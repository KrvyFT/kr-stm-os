# kr-stm-os 批处理系统实施指南

目标是在 STM32H743VGT6 上从片内 Flash 启动内核，从板载 W25Q64JV 读取应用镜像，把每个应用装入 AXI SRAM，逐个在非特权态运行。第一版验收时，串口应显示两个应用按顺序运行并退出，最后显示批处理完成。

当前内核启动时读取 W25Q64JV 的 JEDEC ID，打印后执行 `panic!("Done!")` 演示。用户 crate 提供 `hello`、应用链接脚本和 Thumb 入口，主机打包器生成 `output/apps.img`。内核侧的应用装载、SVC 分派和 MPU 配置是接下来要实现的内容。板级接线来自你提供的 W25Q64JV 原理图片段，本文没有做实板读写或烧录验证。相关代码见 [内核入口](../kernel/src/main.rs)、[板级初始化](../kernel/src/board.rs)、[应用链接脚本](../user/link.x) 和 [应用入口](../user/src/runtime.rs)。

## 1. 先分清三种地址

| 对象 | 地址或范围 | 用途 |
| --- | --- | --- |
| 内核片内 Flash | `0x0800_0000`–`0x0807_FFFF` | 当前链接脚本使用的 Flash Bank 1，放向量表、内核代码和初始化数据 |
| 内核 AXI SRAM | `0x2400_0000`–`0x2405_FFFF` | 内核变量、MSP 栈和 panic DMA 缓冲区 |
| 应用 AXI SRAM | `0x2406_0000`–`0x2407_FFFF` | 同一时间只装一个应用。栈占最后 4 KiB，栈顶是结束地址 `0x2408_0000` |
| W25Q64JV 芯片偏移 | `0x000000`–`0x7FFFFF` | 8 MiB 掉电保留空间。本方案先把 `apps.img` 放在偏移 0 |
| QUADSPI 映射窗口的已装芯片范围 | `0x9000_0000`–`0x907F_FFFF` | 启用 memory-mapped 读后，CPU 可以用窗口基址加 NOR 偏移读取。烧录工具也可用这个基址为外部存储定址 |

例如，镜像里的文件字节位于 NOR 偏移 `0x1000`。间接读传 `0x001000` 给控制器。映射读时访问 `0x9000_1000`。应用真正的运行地址由 ELF 决定，可能是 `0x2406_0000`。这三个数字分别回答“芯片里放哪”“CPU 用哪个窗口读”“程序从哪执行”。链接脚本和烧录地址各自解决其中一个问题。见 [内核内存布局](../kernel/memory.x)、[应用内存布局](../user/link.x) 和 [STM32H743 RM0433，Memory map 与 QUADSPI](https://www.st.com/resource/en/reference_manual/rm0433-stm32h7-series-and-stm32h7rs-lines-armbased-32bit-mcus-stmicroelectronics.pdf)。

```text
主机编译内核 ELF ── SWD / probe-rs ──→ 片内 Flash ── 复位启动内核
主机编译应用 ELF ── 打包 output/apps.img ── SWD / external loader ──→ W25Q64JV
                                                            │
内核运行批处理循环 ←── SVC exit ←── 应用在 SRAM 运行 ←── 内核读取并装载
```

## 2. QSPI：从原理图到读出第一个字节

### 2.1 板上的引脚和三个连接路径

下面的引脚号是**原理图中 IC2 的脚号**。MCU 栏填的是 GPIO 名和复用功能 AF。AF 决定由哪一个外设控制该 GPIO。Bank 1 指 QUADSPI 的一组片选与数据线，和片内 Flash 的 Bank 1 是两个独立概念。见 [STM32H743 数据手册，管脚和 AF 表](https://www.st.com/resource/en/datasheet/stm32h743bg.pdf)。

| IC2 脚号 | W25Q64JV 信号 | STM32 引脚 | 用途 |
| --- | --- | --- | --- |
| 1 | `/CS` | PB6，AF10，`QUADSPI_BK1_NCS` | 低电平选中芯片。一次读写事务通常由 CS 拉低开始、拉高结束 |
| 2 | DO / IO1 | PD12，AF9 | 单线模式下 Flash 到 MCU。四线数据阶段作为双向 IO1 |
| 3 | `/WP` / IO2 | PE2，AF9 | 单线模式下具有写保护相关功能，受状态位配置影响。Quad 模式为 IO2 |
| 4 | GND | 板上地 | 信号和电源共用的参考地 |
| 5 | DI / IO0 | PD11，AF9 | 单线模式下 MCU 到 Flash。四线数据阶段作为双向 IO0 |
| 6 | CLK | PB2，AF9 | STM32 输出时钟 |
| 7 | `/HOLD` / IO3 | PD13，AF9 | 单线模式下可暂停通信，具体功能随型号与配置而定。Quad 模式为 IO3 |
| 8 | VCC | 板上 3.3 V | C24 为 0.1 µF，接在 VCC 与 GND 之间用于片旁去耦 |

- **板载 QSPI** 已由 PCB 连接。调试时按网名找测试点或可访问焊盘，有逻辑分析仪时先看 PB6、PB2、PD11、PD12。原理图片段没有给出这些信号的排针编号。
- **SWD 烧录和调试** 用探针接 MCU 的 SWDIO（PA13）、SWCLK（PA14）和 GND。目标电压参考 VTref、复位 NRST 按探针与板端标识核对。VTref 通常用于电平参考，是否由探针供电要看探针标注。具体插头方向和脚号需要板级接插件图。
- **串口日志** 用 3.3 V TTL USB 转串口。PA9（MCU TX）接转接器 RX，GND 相连。需要接收时再把转接器 TX 接 PA10（MCU RX），当前内核只保留发送句柄。终端选 19200、8N1、无流控。见 [当前板级初始化](../kernel/src/board.rs)。

### 2.2 NOR 为什么要先擦、再写

W25Q64JV 的 64 Mbit 对应 8 MiB。**页**是编程边界，大小 256 B。**扇区**是常用的最小擦除单位，大小 4 KiB。擦除后字节为 `0xFF`，页编程能把位从 1 改成 0，要从 0 回到 1 就需要擦除所在扇区。写一整份镜像时按扇区确定擦除范围，再按页边界切片编程。每次擦除或编程前都要发 Write Enable，之后等待 BUSY 清零，等待必须有超时。跨页写入有页内回卷的风险，所以一次最多写 `256 - (地址 % 256)` 字节。若一个扇区有其他数据，擦除会一起清掉，镜像分区边缘应按扇区管理。见 [Winbond W25Q64JV 数据手册](https://media.digikey.com/pdf/Data%20Sheets/Winbond%20PDFs/W25Q64JV_RevK_3-10-21.pdf)。

| 单线指令 | 用途 |
| --- | --- |
| `0x9F` | 读 JEDEC ID。IQ/JQ 版本预期 `EF 40 17`，先核对实物后缀和读数 |
| `0x03` | 普通读，发指令和三字节地址，接收数据，无 dummy cycles |
| `0x06`、`0x05` | 写使能、读状态寄存器 1。用 WEL 检查写使能，用 BUSY 等待芯片完成 |
| `0x20`、`0x02` | 4 KiB Sector Erase、Page Program。用于外部烧录器或专用测试固件 |

一次典型读事务依次发送指令、地址、可选的等待时钟（dummy cycles），再接收数据。比如 `0x03` 是单线命令、地址和数据。后续 `0x6B` 的数据阶段使用四根线，指令和地址仍走单线，需要按手册配置 dummy cycles 并确认 QE（Quad Enable）状态。原理图所写的 133 MHz 是特定供电和指令条件下的 Flash 规格，`0x03` 有独立时钟上限，而且板级走线和 STM32 芯片版本也会限制速度。首次调试建议从约 1 MHz 或板上分频能得到的相近低速开始。见 [Winbond AC 特性及指令表](https://media.digikey.com/pdf/Data%20Sheets/Winbond%20PDFs/W25Q64JV_RevK_3-10-21.pdf) 和 [STM32H743 勘误 ES0392](https://www.st.com/resource/en/errata_sheet/dm00368411-stm32h742xig-and-stm32h743xig-device-limitations-stmicroelectronics.pdf)。

### 2.3 在当前项目怎么接入

1. 在 `kernel/src/qspi.rs` 封装 Flash。由 `Board::init()` 在现有 PWR/RCC 初始化之后，配置 GPIOB、GPIOD、GPIOE 的复用和 QUADSPI 时钟及复位，把 Flash 句柄交给 `Board`。PB6 片选也要配置成 AF10。HAL 示例单独设置片选 GPIO，照抄 `bank1()` 的时钟和 IO 参数容易漏掉它。
2. 配置单芯片、Bank 1、低速、符合 NOR 要求的时钟模式和片选高电平间隔。间接普通读使用 24 位地址。控制器的设备大小 `FSIZE` 用的是地址位数减一，8 MiB 等于 `2^23` 字节，对应 `FSIZE = 22`。地址阶段仍发送 24 位，容量和传输地址长度各有用途。
3. 先实现 `jedec_id()`，使其只发指令并读三个字节，不添加地址。接着实现 `read_at(offset, &mut [u8])`，用 `checked_add` 检查读区间，空缓冲区直接返回。底层等待应可超时。带长度上限的 HAL 操作按其支持的大小分块，失败就报告偏移和错误。
4. 用间接读先跑通整个批处理。后续需要改善吞吐量时，再尝试 Quad fast read 或 memory-mapped。映射模式由控制器把 CPU 读转换为 NOR 事务。通过 `0x9000_0000` 窗口直接写内存无法完成 NOR 页编程。本文应用链接到 SRAM，切到 QSPI 原地执行还要重新安排应用链接地址。

现有依赖是 `stm32h7xx-hal 0.16.0`。该版本的 `read_extended()` 单次最多读 32 B，长度为零也会触发 panic，并且内部轮询没有超时。先用它验证短事务。实现有界等待时，应使用分阶段接口加有界的 PAC 状态轮询，或把完整事务封装进自己的 PAC 驱动，在错误时清状态并恢复外设。HAL 初始化把 `FSIZE` 写成 `0x1F`，所以在间接读的 Rust 封装里仍要明确检查 8 MiB 边界。切换到映射读时，按 RM0433 和勘误设置实际大小。首次读出 ID 并确认已有扇区内容后，再做擦写试验。见 [HAL 0.16.0 QSPI 示例](https://docs.rs/crate/stm32h7xx-hal/0.16.0/source/examples/qspi_flash_memory.rs)、[STM32 RM0433](https://www.st.com/resource/en/reference_manual/rm0433-stm32h7-series-and-stm32h7rs-lines-armbased-32bit-mcus-stmicroelectronics.pdf) 和 [ST AN4760，Quad-SPI 工作模式](https://www.st.com/resource/en/application_note/an4760-introduction-to-quadspi-interface-for-stm32-mcus-and-mpus-stmicroelectronics.pdf)。

## 3. 构建、打包和烧录

### 3.1 先认识 ELF 和 apps.img

```bash
cargo build --locked -p user_lib --bin 00hello
cargo build --locked --release -p user_lib --bins
readelf -h -lW target/thumbv7em-none-eabihf/release/00hello
llvm-objdump -d target/thumbv7em-none-eabihf/release/00hello
```

ELF 带有入口、段表、符号和调试信息，可用于检查和调试。当前 `hello` 的入口值是 `0x2406_0001`。最低位标记 Thumb，指令实际位于 `0x2406_0000`。应用链接脚本为代码与数据定义不同的 `PT_LOAD`。对每个段，`p_filesz` 是 ELF 里已有的初始化字节数，`p_memsz` 是运行时要占用的内存长度。两者的差额需要在装载时清零。直接把 ELF 文件从开头复制进 SRAM 会把文件头也写到应用入口，CPU 就无法按预期取指。见 [应用链接脚本](../user/link.x) 和 [Arm ELF 规范](https://github.com/ARM-software/abi-aa/blob/main/aaelf32/aaelf32.rst)。

运行 `make pack-apps` 构建应用并生成 `output/apps.img`。默认按 `user/src/bin/*.rs` 的文件名升序打包，目前是 `00hello`、`01store_fault`。用 `PROFILE=release` 选择发布版，或者用 `APPS` 显式指定应用列表和顺序。也可以直接运行 `python3 tools/pack-apps -o output/apps.img <应用ELF路径>` 打包已有 ELF。

打包器按指定顺序读取应用 ELF，检查为小端 ARM 32 位可执行文件，提取入口和 `PT_LOAD`。Makefile 从当前源码推导应用列表，避免把残留构建产物误打包。应用仍在同一个固定 RAM 区轮流运行，应用目录存放在 NOR，更新镜像时无需重编内核。本机的 `/home/krvy/Projects/krunix/os/build.rs` 则通过 `.incbin` 把应用并入内核。

KAPP v1 的整数统一用小端 `u32` 并逐字段编码，镜像偏移一律从镜像首字节算起。文件从前往后依次是镜像头、应用表、所有段表、段负载。解析时检查这四块的边界和顺序，段的目标地址独立验证。

| 内容 | 字段 |
| --- | --- |
| 24 B 镜像头 | `magic[4] = "KAPP"`、`version = 1`、`total_len`、`app_count`、`app_table_offset`、`image_crc32` |
| 28 B 应用表项 | `name[16]`（短 ASCII 名，零填充）、`entry`（保留 Thumb 位）、`segment_count`、`segment_table_offset` |
| 20 B 段表项 | `dst_addr`、`payload_offset`、`file_len`、`mem_len`、`flags`（沿用 ELF 的 R/W/X 位） |
| 负载 | 各段的 `file_len` 字节，`mem_len - file_len` 由内核清零 |

CRC-32/ISO-HDLC 覆盖 `[0, total_len)`，计算时将 CRC 字段本身的 4 字节按零处理，用于发现写入或传输损坏。打包器限制镜像为 8 MiB、最多 16 个应用、每个应用最多 16 个装载段，也支持零应用镜像。应用名取自 ELF 文件名（去掉可选的 `.elf` 后缀），长度为 1–16 字节，可使用 ASCII 字母、数字、下划线、点和连字符。裸 NOR 的开头通常读到 `FF FF FF FF`，内核应报告未装镜像，停在诊断路径。

### 3.2 烧录时谁负责写 QSPI

| 操作 | 结果 |
| --- | --- |
| `cargo build` | 在电脑上生成 ELF |
| 当前 `cargo run --locked` | 选中内核，通过 probe-rs 烧录内核 ELF 对应的片内 Flash |
| 用 external loader 烧录 `apps.img` | 通过 SWD 把临时烧写程序放进 MCU RAM，由它初始化 QUADSPI、擦扇区、写页并校验 |
| 复位或重新上电 | 临时 loader 的代码失去运行状态，内核重新初始化 QUADSPI 读取已写好的镜像 |

先从板卡供应商找与这六根引脚和 W25Q64JV 同时匹配的 `.stldr`。如果没有，经验证后可用 ST 的 external loader 工程模板改造，设置 `0x9000_0000`、8 MiB、256 B 页和 4 KiB 扇区，按本板初始化 Bank 1。STM32H743I-EVAL 的官方样例适合作为移植参考，它的外部 Flash 型号与本板不同。也可以为 probe-rs 编写匹配的 Flash algorithm 和 target 描述。当前本机 `probe-rs chip info STM32H743VGTx` 即使列出外部 NVM 地址范围，也只凭这个输出无法确认算法、板级引脚和器件一致。见 [ST external loader 工程](https://github.com/STMicroelectronics/stm32-memory-loaders) 和 [probe-rs Flash algorithm 说明](https://probe.rs/docs/knowledge-base/cmsis-packs/)。

匹配的 loader 和 `output/apps.img` 准备好后，在仓库根目录用 Makefile 调用 CubeProgrammer。默认 loader 可用 `make qspi-loader` 获取。使用其他 loader 时，传入实际路径。连接需要 ST-LINK 和已接好的 NRST。

```bash
make qspi-read QSPI_LOADER=/path/to/board-w25q64jv.stldr
make qspi-write QSPI_LOADER=/path/to/board-w25q64jv.stldr
```

默认写入地址 `0x90000000` 是 loader 对外公布的基址，对应 NOR 内部偏移 0。先确认镜像涉及的完整扇区都可擦除。首次使用 loader 时，用 `QSPI_ADDR` 指向已确认空闲的扇区，写短样本并在断电重启后核对回读。若没有合适的 loader，可写单独的板上测试固件，经 SWD 把样本放进 SRAM，调用 NOR 擦写函数并回读。正式内核第一版保留读取能力就够了。见 [Makefile 用法](../README.md#构建与烧录)和 [STM32CubeProgrammer CLI 的 `-w`、`-v` 和 `-el`](https://dev.st.com/stm32cube-docs/prog/2.23.0/en/docs/markup/CubeProg_Command_Lines.html)。

## 4. 启动后怎么把 QSPI 里的程序搬进内存

建议把镜像解析和 SRAM 写入分开。先让只读解析器输出目录，然后再用经过验证的描述写内存。这样损坏的镜像头可以被拒绝，而不会先改掉 RAM。

1. 从 NOR 偏移 0 读固定长度的头，检查魔数、版本、总长和应用数。逐字段读为小端整数，检查 `offset + length` 和 `count × record_size` 有无溢出。确认各表和负载都落在声明的镜像范围中，最后流式核对 CRC。
2. 为当前应用验证每个段。要求 `file_len <= mem_len`、目标范围落在 `[0x2406_0000, 0x2407_F000)`、本应用的段互不重叠、文件字节落在负载区。检查入口有 Thumb 位，且清掉最低位后落在可执行段的已初始化字节中。传给 CPU 的目标地址因此来自验证通过的数据。
3. 应用停止后，用固定大小缓冲区按块从 `payload_offset` 读 `file_len` 字节，写到 `dst_addr`。再把 `[dst_addr + file_len, dst_addr + mem_len)` 清零。第一版可以在装载前清理整个应用区，避免下一个应用读到上一轮的栈和未用空隙。用原始指针写计划好的范围，整个装载期间内核独占应用区。任一读失败都跳过执行，报告应用和段号。
4. 如果 D-Cache 已开启，CPU 写入的指令字节可能还留在 D-Cache。完成复制后清理涉及的 D-Cache 地址范围并同步，若 I-Cache 已启用，失效相关取指缓存，再执行所需屏障。初版可沿用内核的缓存配置，单独验证有缓存时的代码重载。维护范围要按缓存行要求对齐。后续若改用 DMA，DMA 写入与 CPU 缓存的同步方向要另行设计。见 [ST AN4839，Cache maintenance](https://www.st.com/resource/en/application_note/an4839-level-1-cache-on-stm32f7-series-and-stm32h7-series-microcontrollers.pdf) 和 [当前 panic DMA 文档](dma-panic.md)。
5. 在调试器查看 `0x2406_0000` 开始的内存，与 ELF 的反汇编比较。检查 `.data` 初值、`.bss` 和预留栈范围，最后才进入应用。建议第二个应用包含一个有初值的全局变量和一个零初值变量，运行时实际读取它们，验证装载和复用。

## 5. Cortex-M7 的陷入、中断和特权级切换

### 5.1 先把四个概念分开

- **Thread mode** 运行常规代码。内核的 Rust `main` 在这里开始执行，用户程序也在这里执行。Thread mode 可以是特权或非特权。
- **Handler mode** 运行异常处理函数。SVC、设备 IRQ、SysTick 和 Fault 都会进入此模式。Handler 始终是特权态，使用 MSP。
- **MSP / PSP** 是两套栈指针。本方案让内核 Thread 和所有 handler 使用 MSP，应用 Thread 使用 PSP。栈从高地址向低地址增长，两条栈都要求为 C ABI 调用保持正确对齐。
- **特权级** 和 **内存隔离** 是两件事。特权代码可以配置诸如中断控制和 MPU 的系统寄存器。应用降权以后，还要配置 MPU 权限，才能把内核 RAM、片内 Flash、QSPI 和设备寄存器挡在应用可访问范围之外。

SVC 是程序主动执行的指令，行为类似 `krunix` 的 `ecall`。IRQ 由硬件事件触发，可能在应用任意两条指令之间到达。Fault 是 CPU 检测到异常条件时产生的异常，例如访问权限错误或非法指令。异常向量表给每种异常对应的入口，内核继续使用 `cortex-m-rt` 生成的向量表，并提供自己的 `SVCall` 等符号。见 [Arm Cortex-M7 手册，Programmers model / Exception model](https://documentation-service.arm.com/static/61efd8352dd99944d05142d1) 和 [cortex-m-rt 链接脚本](https://github.com/rust-embedded/cortex-m/blob/main/cortex-m-rt/link.x.in)。

### 5.2 CPU 自动保存什么，汇编又要做什么

异常发生时，CPU 先在**被打断代码正在用的栈**上压入异常帧，然后进入 Handler mode，用 MSP 执行。应用执行 `svc` 时，现场在 PSP 上，handler 的局部变量在 MSP 上。内核执行启动用的 `svc` 时，内核现场和 handler 的调用都用 MSP。两者的帧内容相同，位置不同。

下面是**基本帧**。表中偏移相对通用寄存器帧起点，单位是字节。

| 偏移 | 内容 | 在本项目的用途 |
| --- | --- | --- |
| `0x00`、`0x04`、`0x08`、`0x0C` | `r0`–`r3` | 传系统调用参数。修改帧里的 `r0` 可以把结果交回被打断的代码 |
| `0x10` | `r12` | 一般临时寄存器，也可由项目约定给系统调用使用 |
| `0x14` | 被打断代码的 `LR` | 原代码的函数返回位置 |
| `0x18` | `PC` | 异常返回时继续执行的位置。SVC 保存的是下一条指令地址 |
| `0x1C` | `xPSR` | 包含 Thumb 状态。硬件生成的帧还可能附带栈对齐填充 |

**栈里的 LR 和 handler 一进门的 LR 不是同一个值。** 栈里的 LR 属于原来的代码。handler 的 LR 是 CPU 写入的 `EXC_RETURN`，低位告诉 CPU 异常返回要用哪条栈、回到哪种模式，以及帧是否包含浮点状态。汇编如果用 `bl` 调 Rust，`bl` 会覆盖当前 LR，所以在调用前要保留 `EXC_RETURN`。普通 Rust handler 按调用约定保护自己用到的被调用者保存寄存器。真正切换到另一套现场时，软件要负责 `r4–r11` 等硬件未保存的状态。当前目标是 `eabihf`。初期可以用经反汇编确认的整数路径验证基本帧，但通用实现仍要检查实际 `EXC_RETURN` 以及浮点和 lazy stacking 状态，逐步支持扩展帧。遇到尚未支持的扩展帧时应沿明确的诊断路径处理，避免按固定偏移误读。见 [Arm Cortex-M7 手册，Exception entry and return / FPU](https://documentation-service.arm.com/static/61efd8352dd99944d05142d1)。

### 5.3 跟着 hello 走一遍

#### 第一步：内核主动进入启动 handler

顶层批处理循环校验镜像、装好第一个应用，调用未来的 `run_app()`。这个函数在**特权 Thread mode + MSP** 中执行预留给内核的启动 SVC。CPU 把内核基本帧压到 MSP，保存的 PC 指向 SVC 后面。内核之后要从这里继续，所以切换路径还要保留内核的 `r4–r11`、帧位置和原 `EXC_RETURN`，并记录当前应用处于运行状态。保存区放在内核所拥有的内存里，后续嵌套 IRQ 在 MSP 上生长时不能覆盖它。

启动 SVC 和用户系统调用共用 `SVCall` 向量。可按来源栈、运行状态和调用号一起分派。调度用的内部操作只接受预期的内核入口。第一版不需要调用链嵌套的应用执行，保留一份内核续执行现场即可。**切换应从明确的汇编边界发生**。如果先调用 Rust 函数做检查或分派，要让它按 ABI 返回到汇编，再执行异常返回。这样普通 Rust 函数的栈帧和寄存器保存仍由编译器成对管理。

#### 第二步：构造应用现场并降权

应用首次运行时还没有自然产生的异常帧，所以从 `__user_stack_top = 0x2408_0000` 往下预留基本帧。用 8 字节对齐的栈顶，基本帧正好 32 B，初始 PSP 例子为 `0x2407_FFE0`。

- 填 `PC = ELF entry & !1`。ELF 的低位标记 Thumb，异常帧里用对齐后的指令地址。
- 填 `xPSR = 1 << 24`，显式启用 Thumb 状态。初始帧按 8 字节对齐构造，用不到额外对齐填充。
- 应用的 Rust 入口 `_start()` 负责调用 `main()`，再把返回值交给退出系统调用。因此 `main()` 的返回地址由入口里的函数调用设置，初始帧的 LR 不参与这条返回路径。
- 初始化基本帧里的通用寄存器。为了避免泄露内核寄存器内容，切换时也要给应用的 `r4–r11` 确定初值。版本一只允许已验证的基本帧路径。
- 写好 PSP 和 `CONTROL.nPRIV = 1`，按要求执行指令同步。Handler 通过 `EXC_RETURN = 0xFFFF_FFFD` 发起基本帧异常返回，CPU 从 PSP 弹出应用帧，进入**非特权 Thread mode + PSP**。这个返回码选择 Thread 和 PSP，`CONTROL.nPRIV` 决定返回后的特权级。

至此应用的第一条指令才开始执行。与 `krunix` 构造 `sepc`、用户 SP 和 `sstatus` 后 `sret` 的目标一致。具体的寄存器、栈和返回指令按 Cortex-M7 的规则设置。见 [Rust 应用入口](../user/src/runtime.rs) 和 [Arm Cortex-M7 手册，Exception return 表](https://documentation-service.arm.com/static/61efd8352dd99944d05142d1)。

#### 第三步：应用请求 write，内核办完后返回应用

建议用 `svc #imm` 区分调用，例如内核启动用 `#0`，用户退出用 `#1`，用户写入用 `#2`。对写入，`r0` 放用户缓冲区指针，`r1` 放字节长度。裸汇编入口用 `EXC_RETURN` 判定硬件帧来自哪条栈，必要时解析扩展帧，Rust 分派函数从基本帧读参数。若按立即数派发，在验证返回 PC 位于对应的可执行范围后，读 `stacked_pc - 2` 处的 Thumb 半字并检查它确实是 SVC 指令。保存的 PC 已经指向 SVC 后面，返回时原值即可。

`write` 先对长度设上限，再用 checked arithmetic 验证整个切片在应用可读范围。空写入可直接返回 0，避免为任意空指针构造切片。把字节复制到内核拥有的队列，然后把写入结果放回异常帧的 `r0`。应用继续运行后可能马上复用原缓冲区，所以串口驱动应持有复制好的数据。用能够从 PSP 恢复该帧的实际 `EXC_RETURN` 返回。这个过程不需要另存一份应用的完整上下文。见 [当前串口队列](../kernel/src/console.rs) 和 [Arm Cortex-M7 手册，SVCall 与异常返回](https://documentation-service.arm.com/static/61efd8352dd99944d05142d1)。

#### 第四步：应用退出，批处理循环拿回控制权

`hello` 的 `main()` 先返回整数 `0`，共用入口再触发 `SVC exit`。handler 取退出码，标记应用执行结束，把结果写到**内核**启动 SVC 的保存帧或经明确约定的内核状态。恢复内核保存的 `r4–r11` 和 MSP 现场，令 Thread mode 回到特权级，再走保存的内核 `EXC_RETURN`。若内核现场是基本帧，其返回码通常为 `0xFFFF_FFF9`。应用现场在此结束，无需为退出的应用补存 `r4–r11`。恢复结果是内核在启动 SVC 后面继续，批处理循环可以冲洗串口、打印退出码和加载下一应用。

注意工程实现里的两条边界。第一，普通 SVC 返回原代码时继续用本次进门的 `EXC_RETURN`，退出应用才恢复之前保存的内核令牌。第二，MSP 上当前 exit handler 的调用帧必须通过受控汇编路径正确收尾，才把 MSP 恢复到内核硬件帧。直接在普通 Rust 函数的深层调用里改 SP 会破坏其栈帧。

### 5.4 IRQ、Fault、MPU 和抢占的先后

| 情况 | 第一版怎么做 | 为什么 |
| --- | --- | --- |
| 普通 IRQ | 使用现有向量机制处理外设事件，清对应中断条件，按原现场返回 | IRQ 只需打断应用并处理事件。正常返回时编译器和硬件已经承担各自的保存工作 |
| 进程隔离 | MPU 给应用区非特权访问权限，内核和外设保持特权访问。可先用一个 128 KiB 对齐的应用区，待段边界按 MPU 要求对齐后，再分代码只读可执行和数据、栈可写不可执行 | 基础配置保护内核。ELF 的 R/W/X 段标记只供加载器理解，真正限制 CPU 访问的是 MPU。边界也不能把整个应用区连同内核一起放开 |
| 应用 Fault | 先用已知的应用违规访问验证 MemManage。检查当前应用状态、异常来源、帧范围和故障位。记录 `CFSR`、`HFSR`、有效时的 `MMFAR/BFAR`，可恢复时回到内核并继续下一个 | 栈入栈或退栈失败时，PSP 帧可能不完整。内核来源或无法可靠归属的故障进入内核诊断 |
| 永久循环 | 初版用调试器中止。后续加 SysTick 超时，再决定是否需要 PendSV | 一次只运行一个应用，正常退出足以验证批处理。增加可恢复的抢占时才需要保存可再次调度的应用上下文，包括适用的浮点状态 |

配置可恢复 Fault 前要使能相应异常，避免它们都升级为 HardFault。调异常优先级时注意 Cortex-M 上通常是优先级**数值越小越紧急**，同一个中断要同时看 NVIC 以及外设的挂起条件。见 [Arm Cortex-M7 手册，Fault handling / NVIC / MPU](https://documentation-service.arm.com/static/61efd8352dd99944d05142d1)。

### 5.5 调试器要看什么

| 断点 | 核对内容 |
| --- | --- |
| 内核执行启动 SVC 前 | 特权 Thread、当前 SP 属于 MSP，应用 SRAM 的入口指令与 ELF 相符 |
| 启动 `SVCall` 汇编第一条指令 | 此时 handler 的 LR 仍是 `EXC_RETURN`。它指示内核帧来源。查看帧里 PC 是否等于启动 SVC 的下一条指令 |
| 异常返回之前 | PSP 指向构造的帧，8 字节对齐。检查 `[PSP + 0x18]` 的 PC 和 `[PSP + 0x1C]` 的 Thumb 位。确认 PSP 在预留应用栈内 |
| 应用 `_start` | PC 指令地址为 `0x2406_0000`，线程使用 PSP，`CONTROL.nPRIV` 为 1。若调试器支持，可为装入 SRAM 的应用单独加载 ELF 符号 |
| 应用退出 SVC | 对比 handler 的 LR、应用帧的 LR 和内核先前保存的 `EXC_RETURN`。看应用帧的 `r0` 是否为 0 |
| 内核启动 SVC 之后 | 回到特权 Thread、使用 MSP，读到退出码。循环能够进入下一个应用，最后输出完成 |

一进应用就触发 `INVSTATE` 或 `INVPC`，先看初始 PC、`xPSR.T`、PSP 对齐、异常返回值和浮点帧类型。若只看到 `DefaultHandler`，用 ELF 符号和向量表核对 `SVCall` 是否确实指向自定义的汇编入口。输出只显示“已装载”时，先用断点证明应用执行到了 `_start`，再定位 SVC 或串口。

## 6. 共享 USART1：谁写、何时写、满了怎么办

### 6.1 现有代码已经提供的语义

当前 `Board` 持有 `ConsoleDriver`。它独占 USART1 TX，另留 DMA1 Stream 0 给内核 panic。普通调用通过 `try_print(&str)` 把一条消息完整复制到 1024 B 队列。最多 128 B，剩余空间不足就返回 `Full`，队列不变。入队在短临界区里完成，所以**一次调用内的字节不会被另一个生产者插进去**。`poll()` 从队列取一个字节，USART 暂时忙时用 `pending` 保留它。只有主程序调用 `poll()` 或 `wait_for_write()`，普通输出才前进。UART 一条 TX 线一次发送一位，所谓同时发送实际是多个代码路径同时提交消息。见 [控制台实现](../kernel/src/console.rs) 和 [panic DMA 实现](../kernel/src/console/panic_dma.rs)。

在 19200、8N1 下，一个数据字节通常占 10 个线路位。128 B 连续发完约 67 ms，1024 B 约 0.53 s。把一整条发送放进高优先级 handler 轮询，会让低优先级工作等很久。临界区只保护队列和发送状态的短操作，等待 UART 就绪应在临界区外。

### 6.2 给生产者一个共同的提交接口

```text
内核日志 ───────┐
用户 SVC write ─┼─→ 内核队列 ─→ 单个 TX 消费者 ─→ USART1 TDR ─→ 终端
IRQ 事件摘要 ───┘
```

用户程序通过 `SVC write(ptr, len)` 提交字节，USART 和 DMA 继续由内核拥有。可把当前 `try_print(&str)` 改为复用内部的 `try_enqueue(&[u8])`，让内核日志保留文本接口，系统调用直接使用字节接口。建议第一版每次提交最多 128 B，成功返回接受的字节数，过长返回明确错误，满了返回 `WouldBlock`。生产者提交时只复制数据，队列里不会留下用户指针。若要保证整行在抢占和多生产者时连续，先用固定容量栈缓冲把整行格式化，再一次提交。一次 Rust 格式化可能调用多次 `write_str`，分段提交只能保证各次提交的原子性。

如果两条生产路径分别提交 `[kernel] ready\n` 和 `[app] hello\n`，处理器先让哪条成功入队，哪条先出现在串口。可以保证两条各自完整，不能依赖同一时刻的生产者之间固定先后。想复查先后可由内核在入队时加序号。IRQ 日志要短，队列满时记录丢弃计数或延迟生成诊断。用户大量输出时，可以按应用限制提交频率或预留内核日志容量，避免把关键消息挤掉。

### 6.3 第一版轮询和后续 IRQ 如何取舍

| 方案 | 发送进度来自哪里 | 限制与适用阶段 |
| --- | --- | --- |
| **最小验证：入队，返回内核后冲洗** | 应用退出后，批处理循环用现有 `ConsoleDriver` 冲洗，再打印退出状态 | 修改量最小，适用于短应用。应用持续运行时内容可能积在队列里。满了就返回错误，应用此时忙等重试没有用，因为消费者还在等应用退出 |
| **持续输出：TX IRQ 消费队列** | TX 就绪触发 USART1 ISR。ISR 在硬件可收时写入字节，队列空时关 TX 就绪中断。生产者重新入队后启动发送 | 适合要边运行边看日志的应用，不需要把驱动从主函数临时借给 SVC。需要用短临界区把“入队并启动”与“判空并关闭”协调起来 |
| **后续扩展：DMA 分块** | DMA 从内核的发送缓冲区往 TDR 写字节，完成 IRQ 安排下一块 | 吞吐量更高。要管理 DMA 可访问的缓冲区、缓存一致性、块生命周期以及 panic 接管协议。在目前的 19200 波特率下优先把队列和 IRQ 做正确 |

**推荐先用轮询验证退出，再切 TX IRQ。** 切换时只保留一个普通 TX 消费者。主程序和 SVC 入队，ISR 消费，UART TX 句柄由控制台驱动持有。不要在主程序持有队列借用或驱动可变借用时进入可能访问同一对象的 SVC，否则容易触发 `RefCell` 借用错误或违反独占访问。用 IRQ 版本时，等待冲洗应等待“队列空、无暂存字节、USART TC 为 1”，并让消费者中断能执行。TX 就绪表示 TDR 或 FIFO 能收下一个字节，TC 才表示最后一帧已从 TX 引脚发完。见 [STM32 RM0433，USART ISR/CR1](https://www.st.com/resource/en/reference_manual/rm0433-stm32h7-series-and-stm32h7rs-lines-armbased-32bit-mcus-stmicroelectronics.pdf) 和 [当前 wait_for_write](../kernel/src/console.rs)。

最容易漏掉的是**丢唤醒**。假设 ISR 看到队列空，线程随后入队并开中断，然后 ISR 又关了它，新数据就会一直停在队列。把队列状态和 TX 中断启停纳入同一同步协议。让生产者在提交成功后启用发送，让消费者在同一个临界区内确认队列仍空后关闭发送中断。应实际测试“队列从空变非空”和“最后一个字节送出时恰好新提交”两种情况。

### 6.4 应用 panic 和内核 panic 的边界

应用发生 panic 时，用户态 handler 尝试输出一条日志，再用 `SVC exit` 提交退出码 `1`。内核接入退出处理后，可以记录该状态并继续批处理。如果退出调用意外返回，用户程序进入自旋，避免再次触发 panic。`panic_dma` 留给内核无法继续执行的情形。它目前禁用可屏蔽中断，使用独立缓冲区和保留的 DMA Stream 直接写 USART1，避免依赖普通队列的借用或锁。引脚上正在发送的旧字节可能先出现，未排出的普通日志可能丢失。如果将来普通发送也用 DMA，panic 接管前需要停掉普通 TX IRQ / DMA 请求，并在有界等待内处理正在发送的字节；同一 TDR 只能有一个活跃的发送方。见 [DMA Panic 与 USART 完成条件](dma-panic.md)。

## 7. 按阶段实现和验收

| 阶段 | 建议位置与工作 | 完成时应观察到什么 |
| --- | --- | --- |
| 1. QSPI 连通 | `kernel/src/board.rs` 接 GPIO、时钟与 QUADSPI，`kernel/src/qspi.rs` 实现 JEDEC ID 与 `read_at` | 稳定读到符合实物版本的 JEDEC ID。逻辑分析仪能区分 CS、CLK、命令和读出字节 |
| 2. 独立镜像 | 主机 `tools/pack-apps` 生成镜像，内核解析只读目录，先用匹配 loader 烧入独立样本 | 烧录工具校验通过。断电重启后内核按顺序列出 `hello`、第二个应用与段信息。空白或损坏镜像有明确诊断 |
| 3. 复制到 SRAM | 在 `kernel/src/batch/` 先实现镜像解析和装载，保持顶层入口只负责编排 | 在调试器里对比目标内存、入口机器码和段内容。长度溢出、越过应用区、无效 Thumb 入口和错误 CRC 都被拒绝 |
| 4. 进入和退出 | `kernel/src/trap/` 放 ARM 异常入口、帧定义和 SVC 分派，应用入口接退出调用 | 先让整数版 `hello` 返回 0，再跑两个不同返回码。验证 PSP/MSP、特权级以及最后一个应用完成后返回内核 |
| 5. 应用打印 | `kernel/src/console.rs` 统一提交接口，先在应用退出后排队冲洗 | 内核日志和应用日志按提交顺序出现。坏指针、超过 128 B 和队列满均有确定返回，无半条入队 |
| 6. 权限与持续输出 | 接 MPU、应用 Fault 处理、USART1 TX IRQ。超时和抢占可另外验证 | 测试应用尝试访问被禁止的地址时记录原因并继续下一个应用。运行中的应用能持续输出，压力测试无交错、无丢唤醒 |

每次只新增一个可观测现象。先证明 QSPI 能读，再证明 RAM 写对，最后才看应用能否执行。这三个检查点分属外设、加载器和 CPU 异常机制，能大幅缩小排查范围。

## 8. 查手册的入口

- 引脚复用看 [STM32H743 DS12110](https://www.st.com/resource/en/datasheet/stm32h743bg.pdf)。实际连线按你提供的 IC2 原理图，板端排针另查板卡连接器图。
- QUADSPI 寄存器、USART 状态位和 MCU 地址图看 [RM0433](https://www.st.com/resource/en/reference_manual/rm0433-stm32h7-series-and-stm32h7rs-lines-armbased-32bit-mcus-stmicroelectronics.pdf)。时序和已知限制一并看 [ES0392](https://www.st.com/resource/en/errata_sheet/dm00368411-stm32h742xig-and-stm32h743xig-device-limitations-stmicroelectronics.pdf)。
- Flash 命令、页和扇区看 [Winbond W25Q64JV 数据手册入口](https://www.winbond.com/hq/support/documentation/?__locale=en&category=%2F.categories%2Fresources%2Fdatasheet%2F&family=%2Fproduct%2Fcode-storage-flash%2Fqspi-nor%2Findex.html&line=%2Fproduct%2Fcode-storage-flash%2Findex.html&pno=W25Q64JVBYIQ)。理解 STM32 外部烧录和映射读看 [AN4760](https://www.st.com/resource/en/application_note/an4760-introduction-to-quadspi-interface-for-stm32-mcus-and-mpus-stmicroelectronics.pdf)。
- 异常、特权级和寄存器语义看 [Arm Cortex-M7 Devices Generic User Guide](https://documentation-service.arm.com/static/61efd8352dd99944d05142d1)。栈对齐与寄存器保存约定看 [AAPCS32](https://github.com/ARM-software/abi-aa/blob/main/aapcs32/aapcs32.rst)。
