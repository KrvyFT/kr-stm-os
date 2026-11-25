# kr-stm-os 用 SVC 启动应用并接收退出码

这一阶段完成一条能在板上验证的路径。内核从 QSPI 打开 KAPP 镜像，将测试应用搬到 AXI SRAM，切到非特权态执行，再通过 `svc #1` 回到内核。串口应由内核打印 `app[0] exited: 42` 和 `Batch complete`。

当前的 [镜像解析器](../kernel/src/batch/image.rs) 已在 `Image::open()` 中校验 CRC，[加载器](../kernel/src/batch/loader.rs) 已按段推进 QSPI 读取偏移，并清零内存中多出来的部分。[内核入口](../kernel/src/main.rs) 通过 `batch::run()` 复用 `loader::load()`。下面按文件说明怎么把这条路径接起来。

本阶段只运行自己构建的整数测试应用。`SVCall` 识别基本异常帧，处理启动、写入和退出。应用使用 PSP 和非特权 Thread mode，MPU 将非特权应用限制在自己的 SRAM 区。现有 `00hello` 和 `01store_fault` 会调用 `println!`，请用 `APPS=00exit42` 打包本轮镜像。

## 1. 先看清两条栈怎么交接

| 时刻 | CPU 模式 | 当前使用的栈 | 下一步 |
| --- | --- | --- | --- |
| 内核调用 `run_app()` | 特权 Thread | MSP | 在用户栈上造出初始异常帧，执行 `svc #0` |
| 启动 `SVCall` | Handler | MSP | 留下内核现场，设置 PSP 和 `CONTROL.nPRIV` |
| 应用执行 `_start` 和 `main` | 非特权 Thread | PSP | `main()` 返回 `42`，公共入口执行 `svc #1` |
| 退出 `SVCall` | Handler | MSP | 从 PSP 帧读取退出码，将其写入内核帧 |
| 内核从 `run_app()` 返回 | 特权 Thread | MSP | 打印退出码，继续处理镜像里的应用 |

MSP 是内核和异常处理程序用的栈，PSP 是应用 Thread 用的栈。**异常发生时，硬件先向被打断一方的栈压入现场，然后 handler 才在 MSP 上执行。** 因此，应用执行 `svc #1` 时，handler 要从 PSP 读取应用的寄存器。完整背景可参阅 [现有批处理指南第 5 节](batch-implementation.md#5-cortex-m7-的陷入中断和特权级切换) 和 [Arm Cortex-M7 异常进入与返回](https://support.arm.com/documentation/dui0646/a/The-Cortex-M7-Processor/Exception-model/Exception-entry-and-return)。

## 2. 编写 Rust 入口和初始异常帧

以下是内核构造应用初始帧的代码。 [异常处理源码](../kernel/src/trap.rs)

```rust
#[repr(C)]
struct BasicExceptionFrame {
    r0: u32,
    r1: u32,
    r2: u32,
    r3: u32,
    r12: u32,
    lr: u32,
    pc: u32,
    xpsr: u32,
}

/// # Safety
/// The entry and stack must belong to the loaded, trusted application.
/// Call in privileged Thread mode on MSP, with SVC available and no active app.
/// The kernel and the app must use basic exception frames.
pub(crate) unsafe fn run_app(entry: u32, stack_top: u32) -> i32 {
    debug_assert_eq!(stack_top & 7, 0);
    debug_assert_ne!(entry & 1, 0);

    let frame = (stack_top as usize - size_of::<BasicExceptionFrame>()) as *mut BasicExceptionFrame;

    unsafe {
        frame.write(BasicExceptionFrame {
            r0: 0,
            r1: 0,
            r2: 0,
            r3: 0,
            r12: 0,
            lr: 0,
            pc: entry & !1,
            xpsr: 1 << 24,
        });
        APP_ACTIVE.store(true, Ordering::Release);
        let code = __kr_run_app(frame);
        APP_ACTIVE.store(false, Ordering::Release);
        code
    }
}
```

- `#[repr(C)]` 固定字段顺序。基本异常帧是八个 32 位字，硬件按这个顺序恢复寄存器。`pc` 位于帧起点 `+0x18`，`xpsr` 位于 `+0x1c`，Rust 通过结构字段访问这两个位置。
- 当前 `stack_top` 是 `0x2408_0000`。栈向低地址增长，所以初始帧写在 `0x2407_ffe0`，满足 8 字节对齐。异常返回弹出 32 字节之后，应用的 PSP 回到 `0x2408_0000`。
- 打包器记录的 Thumb 入口示例是 `0x2406_0001`。`entry & !1` 得到帧中所需的指令地址 `0x2406_0000`，把 `xPSR.T` 设为 1 来指定 Thumb 状态。
- 写入的初始 `lr` 是 0。`_start()` 调用 `main()` 时会建立函数返回地址。等 `main()` 返回后，`_start()` 通过退出系统调用结束应用。
- `__kr_run_app(frame)` 按 C ABI 把帧指针放进 `r0`，并从 `r0` 取回 `i32` 返回值。这个调用跨越汇编边界。`run_app()` 的调用方必须兑现注释里的安全前提。

对应的内存划分见 [内核链接脚本](../kernel/memory.x) 和 [应用链接脚本](../user/link.x)。

## 3. 异常入口与 Rust 分发

`SVCall`、`UsageFault` 和 `MemoryManagement` 共用一段汇编入口。Rust 用 `global_asm!` 和 `include_str!` 引入 `svc.s`，并传入分发函数符号和动作常量。入口在压栈前读取 MSP，用 `{r3, lr}` 暂存 `EXC_RETURN` 并保持 MSP 的八字节对齐，再把原 MSP、`EXC_RETURN` 和 IPSR 传给 Rust。直接从汇编进入确保这些操作发生在 Rust 函数建立自己的栈帧之前。 [汇编入口](../kernel/src/trap/svc.s) · [Rust 分发](../kernel/src/trap.rs)

### 3.1 内核进入 `svc #0`

1. `__kr_run_app` 执行 `dsb` 和 `svc #0`。硬件在 MSP 保存内核帧，帧里的 PC 指向紧随 SVC 的 `__kr_resume_kernel`。
2. Rust 确认 `EXC_RETURN = 0xffff_fff9` 且内核帧的 PC 等于该标签的地址，再用帧内 `r0` 设置初始 PSP。Handler 仍在 MSP 上运行。
3. 汇编从 Rust 返回后，用 `stmdb sp!, {r3-r11, lr}` 保存 40 字节的内核软件区。额外保存 `r3` 使栈保持对齐。寄存器 `r4–r11` 要留给以后恢复内核，并在进入应用前清零。
4. 设置 `CONTROL.nPRIV` 后执行 `isb`，用 `0xffff_fffd` 执行异常返回。CPU 从 PSP 弹出初始帧，开始执行非特权应用。

### 3.2 应用通过 `svc #1` 退出

1. `sys_exit()` 把退出码放入 `r0` 后执行 `svc #1`。硬件把应用现场压入 PSP，Handler 入门 LR 为 `0xffff_fffd`。 [用户态入口](../user/src/runtime.rs) · [系统调用](../user/src/runtime/syscall.rs)
2. Rust 校验 PSP 帧和读取地址，从 `frame.pc - 2` 读取 Thumb 半字。编码 `0xdf01` 表示退出，`frame.r0` 是退出码。
3. 当前 MSP 指向内核软件保存区。Rust 将退出码写入 `MSP + 40` 处的内核帧首字，随后汇编恢复软件区和特权级。
4. 汇编用内核的 `EXC_RETURN` 执行 `bx lr`。CPU 从 MSP 恢复硬件帧，在 `__kr_resume_kernel` 继续，此时 `r0 = 42`。恢复点的第二个 `bx lr` 使用硬件弹出的普通 LR，回到调用 `__kr_run_app()` 的 Rust 函数。

### 3.3 应用通过 `svc #2` 写入并继续运行

Rust 解码出 `0xdf02` 后，从 PSP 帧取缓冲区地址和长度，调用 `sys_write`，再把结果写回同一帧的 `r0`。汇编恢复进入 SVC 时的 `EXC_RETURN`，应用在原来的 PSP 上从 SVC 下一条指令继续。MSP 上暂停的内核现场保留到应用退出。未知 SVC 和扩展浮点帧会进入不支持的异常路径。

```rust
fn sys_write(addr: u32, len: u32) -> i32 {
    const EAGAIN: i32 = -11;
    const EFAULT: i32 = -14;
    const EMSGSIZE: i32 = -90;

    if len == 0 {
        return 0;
    }
    if len as usize > MAX_MESSAGE_BYTES {
        return EMSGSIZE;
    }
    if addr < APP_START || addr.checked_add(len).is_none_or(|end| end > APP_STACK_TOP) {
        return EFAULT;
    }

    // SAFETY: The current batch runner admits trusted apps whose sys_write
    // passes an initialized byte slice. The range check covers code/data and PSP.
    let bytes = unsafe { slice::from_raw_parts(addr as *const u8, len as usize) };
    match console::try_write(bytes) {
        Ok(()) => len as i32,
        Err(WriteError::TooLong) => EMSGSIZE,
        Err(WriteError::Full) => EAGAIN,
    }
}
```

检查按固定顺序进行。零长度直接返回 0。超过 128 字节返回 `-90`。其余请求用 `checked_add` 计算排他末地址，区间必须落在 `[APP_START, APP_STACK_TOP)`，溢出或越界返回 `-14`。`try_write(&[u8])` 在短临界区内把合法数据整段复制到 1024 字节队列，返回入队字节数。队列装不下时返回 `-11`，原队列保持不变。`try_print(&str)` 也复用该字节接口，因此内核文本和应用字节按实际入队顺序发送。

范围检查覆盖整块应用 SRAM，包含 PSP 栈和段间空洞。代码依赖受信任的应用传入已初始化字节，当前 MPU 将非特权访问限制在该 SRAM 区，区内权限统一。handler 使用独立的全局队列接口，因此无需取得第二份控制台驱动的可变借用。外层批处理等到应用结束后追加结果日志，再轮询发送。写入返回 5 表示已接收 5 字节，串口此时仍可能没有输出。 [写入实现](../kernel/src/trap.rs) · [控制台](../kernel/src/drivers/console.rs) · [批处理](../kernel/src/batch.rs)

## 4. 接进批处理循环和内核入口

在 [batch.rs](../kernel/src/batch.rs) 中定义以下函数。它借用已经初始化的 Flash 和控制台驱动，格式化和发送由控制台自己的 `print_fmt()` 完成。

```rust
pub(crate) unsafe fn run(    flash: &mut W25q64jv,
    console: &mut ConsoleDriver,
) -> Result<(), ImageError> {
    let mut image = Image::open(flash)?;
    for index in 0..image.app_count() {
        // SAFETY: The app area is idle until run_app, which returns only
        // after the app has released it and restored the kernel context.
        let code = unsafe {
            let app = loader::load(&mut image, index)?;
            trap::run_app(app.entry, app.stack_top)
        };
        match trap::take_app_fault() {
            Some((pc, cfsr)) => {
                const DACCVIOL: u32 = 1 << 1;
                const MMARVALID: u32 = 1 << 7;
                const UNDEFINSTR: u32 = 1 << 16;

                // MMARVALID 只表示故障地址有效，去掉它再匹配故障原因。
                let reason = match cfsr & !MMARVALID {
                    DACCVIOL => "MPU denied data access",
                    UNDEFINSTR => "undefined instruction",
                    _ => "unknown fault",
                };

                console.print_fmt(format_args!(
                    "app[{index}] faulted: {reason} (pc={pc:#010x})\r\n"
                ));
            }
            None => {
                console.print_fmt(format_args!("app[{index}] exited: {code}\r\n"));
            }
        }
    }
    console.print_fmt(format_args!("Batch complete\r\n"));
    Ok(())
}
```

`Image::open()` 放在循环外，整份镜像只校验一次。应用都链接到 `0x2406_0000`，因此这一轮必须完成「装载 → 执行 → 退出」之后，下一轮才能覆盖 SRAM。镜像或装载错误由 `?` 返回给内核入口。

完整的 MPU 初始化与 Fault 恢复见[第二章](chapter-2-batch-system.md)。检查 [main.rs](../kernel/src/main.rs) 模块声明区有 `mod trap;`。在板级初始化和启动日志之后，把 `Board` 持有的 Flash 和控制台句柄交给批处理，并在入口统一报告失败。

```rust
    // SAFETY: Boot is in privileged Thread mode on MSP. The batch image
    // is provisioned with trusted apps that use the basic exception frame.
    let result = unsafe { batch::run(&mut board.flash, &mut board.console) };
    if let Err(error) = result {
        board
            .console
            .print_fmt(format_args!("KAPP error: {error:?}\r\n"));
    }

    panic!("Done!");
```

`Board::init()` 已构造 `W25q64jv`，所以这里可以直接借用 `board.flash`。现有 `inspect()` 可以在需要查看镜像目录时单独调用。当前启动入口使用 `run()` 时，编译器可能提示保留下来的诊断函数未使用。

## 5. 用退出码 `42` 做第一次验收

创建 `user/src/bin/00exit42.rs`。

```rust
#![no_std]
#![no_main]

use user_lib as _;

#[unsafe(no_mangle)]
fn main() -> i32 {
    42
}
```

`use user_lib as _;` 把现有的 `_start` 和 panic handler 链接进应用。`main()` 返回 `42` 后，运行时自动发出 `svc #1`。退出日志由内核打印，这样能单独检查应用启动和退出是否成功。

### 5.1 编译并核对 ELF

从仓库根目录执行。

```bash
make build
make pack-apps APPS=00exit42 QSPI_IMAGE=output/apps-exit42.img
llvm-readelf -sW target/thumbv7em-none-eabihf/debug/kr-stm-os \
    | rg 'SVCall|__kr_run_app'
llvm-readelf -h target/thumbv7em-none-eabihf/debug/00exit42 \
    | rg 'Entry point'
```

检查内核存在 `SVCall` 和 `__kr_run_app` 两个函数符号，应用入口为 `0x24060001`，打包器显示 `1 application(s)`。`APPS` 决定打包范围，`make pack-apps` 的前置目标依然会编译所有应用。

### 5.2 烧录并观察串口

使用板上对应的 QSPI loader 和探针，烧录命令沿用项目现有的 Makefile。若缺少仓库默认的 loader，可以先执行 `make qspi-loader` 获取并校验它。若使用自定义 loader，在 `qspi-write` 的命令行加 `QSPI_LOADER=/实际路径/loader.stldr`。

```bash
make qspi-write QSPI_IMAGE=output/apps-exit42.img
make flash-kernel
```

打开串口后复位。串口设置为 **19200 baud、8N1、无流控**。正常输出如下。 [接线与串口设置](../README.md#查看串口输出)

```text
kr-stm-os booted
app[0] exited: 42
Batch complete
```

当前内核随后执行 `panic!("Done!")`，用于观察 DMA panic 输出。

只出现启动日志时，先看 `SVCall` 有没有命中，再判断卡在装载、进入应用，还是退出返回。

## 6. 在调试器里逐站验证

在 VS Code 的项目调试配置中设断点，或在已连接目标的 GDB 中用 `hbreak SVCall`。这个断点应命中两次。应用代码在运行时才复制进 SRAM，若要在入口预设断点，使用硬件断点 `hbreak *0x24060000`。调试后端公开系统寄存器时，可以用 `info registers lr msp psp control` 查看值。

| 检查点 | 应看到的值 | 这个检查能定位什么 |
| --- | --- | --- |
| 第一次进入 `SVCall` | `LR = 0xfffffff9`。`MSP + 0x18` 处保存内核 `svc #0` 的下一条指令地址 | 确认向量正确，内核从预期的 MSP 路径进来 |
| 以 `0xffff_fffd` 返回的 `bx lr` 之前 | `LR = 0xfffffffd`，`PSP = 0x2407ffe0`。初始帧的 PC 为 `0x24060000`，xPSR 的 Thumb 位为 1 | 确认第一份应用帧符合异常返回要求 |
| 应用入口 `0x24060000` | `PSP = 0x24080000`，`CONTROL` 低两位为 `0b11` | 确认 CPU 真的开始执行应用，且线程使用非特权 PSP |
| 第二次进入 `SVCall` | `LR = 0xfffffffd`。读 `[PSP]` 得到 `42`，`[PSP + 0x18] - 2` 处的半字是 `0xdf01` | 确认退出码进入应用帧，且调用号正确 |
| 内核 `svc #0` 后一条指令 | `r0 = 42`，Thread mode 使用 MSP，`CONTROL.nPRIV = 0` | 确认恢复的是内核现场，退出码到达了返回寄存器 |

GDB 中用 `disassemble __kr_run_app` 找到 `svc #0` 后面那条 `bx lr` 的地址。在那里设断点，可观察内核刚从退出异常返回的状态。查看基本帧时，`x/8wx $msp` 或 `x/8wx $psp` 的第一个字是 `r0`，第七个是 PC，第八个是 xPSR。对 MSP 的这条检查要在首次进 handler 且还未执行临时的 `push` 时做。

如果需要确认装载内容，可先停在 `__kr_run_app`。此时应用已装好，但尚未执行。用 `x/8hx 0x24060000` 查看 SRAM 入口的指令半字，再运行 `llvm-objdump -d target/thumbv7em-none-eabihf/debug/00exit42`，和 ELF 中的 `_start` 指令对照。

| 现象 | 优先检查 |
| --- | --- |
| 打印 `KAPP error: BadCrc` | 确认烧入的是本轮 `apps-exit42.img`。需要时用 Makefile 的 `qspi-read` 读回相同长度，然后用 `cmp` 比较 |
| 启动 SVC 没有命中 `SVCall` | 查看内核 ELF 中的 `SVCall` 和向量表第 11 项。确认运行的是刚构建的内核，且 SVC 没被屏蔽 |
| SVC 分发执行 `udf` | 查看 LR 和堆栈里的 PC。基本 PSP 帧下读 `PC - 2`，检查是否为未知 SVC 编号。若 LR 指示扩展帧，检查内核和应用是否执行了浮点路径 |
| 已到启动分支，首次 `bx lr` 就触发 Fault | 核对 PSP 对齐、初始 PC、`xPSR.T` 和 `EXC_RETURN`。可在 Fault 处读 `CFSR`（`0xe000ed28`）与 `HFSR`（`0xe000ed2c`） |
| 能进应用，但退出后挂住 | 检查退出分支有没有把 `42` 写回内核帧的 `r0`，以及 `ldmia` 后 MSP 是否正好回到内核硬件帧 |

需要确认 QSPI 中的镜像时，按原始镜像长度读回，再比较两个文件。自定义 loader 的配置与烧录时一致。

```bash
make qspi-read QSPI_SIZE="$(wc -c < output/apps-exit42.img)" \
    QSPI_OUTPUT=output/apps-exit42-readback.img
cmp output/apps-exit42.img output/apps-exit42-readback.img
```

## 7. 再验证两个应用顺序交接

单应用通过后，把 `00exit42.rs` 复制为 `02exit7.rs`，将 `main()` 的返回值改为 `7`。按这个顺序打包并烧录新的镜像。

```bash
make pack-apps APPS='00exit42 02exit7' QSPI_IMAGE=output/apps-exit-pair.img
make qspi-write QSPI_IMAGE=output/apps-exit-pair.img
```

复位后应依次看到下面三行。

```text
app[0] exited: 42
app[1] exited: 7
Batch complete
```

两个退出码都对，说明内核第一次退出后能继续迭代、在同一个 SRAM 地址加载下一应用，再完成一次上下文恢复。到这里再实现 `svc #2` 的串口写入，随后用原来的 `00hello` 验证用户态输出。
