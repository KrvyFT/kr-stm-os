# STM32H743 上的 DMA panic 输出

在 `kr-stm-os` 中，正常日志先进入 `TX_QUEUE`，再由启动代码调用 `ConsoleDriver::wait_for_write()`，在等待期间通过 `poll()` 发送。panic 发生后，执行流进入 panic handler，因此 panic handler 直接用 **DMA1 Stream 0 将 AXI SRAM 中的错误消息送到 USART1**。CPU 负责准备消息、配置传输和检查结果，DMA 负责逐字节搬运。

```text
控制路径   USART1_TX 请求 → DMAMUX1 通道 0 → DMA1 Stream 0
数据路径   AXI SRAM 缓冲区 → DMA1 Stream 0 → USART1.TDR → PA9
```

USART1 当前配置为 19 200 baud，PA9 是 TX，PA10 是 RX，两个引脚都使用 AF7。这里的 DMA 传输只用发送方向。

## 1. 启动时预留 DMA Stream 0

在 `Board::init()` 中，先配置 PWR 和 RCC，再配置 PA9、PA10 与 USART1。随后把串口拆成 TX、RX，并用 HAL 打开 DMA1 的时钟。 [板级初始化](../kernel/src/board.rs)

```rust
let (tx, _rx) = serial.split();
let dma = StreamsTuple::new(dp.DMA1, ccdr.peripheral.DMA1);
let driver = ConsoleDriver::new(tx, dma.0);
```

`dma.0` 的类型是 `Stream0<DMA1>`。驱动把它保存在 `_panic_dma` 字段里，表示这条 Stream 已经由控制台保留。正常的 `poll()` 使用 `Tx<USART1>` 逐字节发送。只有 panic 路径会用到这条 DMA Stream。

`ConsoleDriver::new()` 调用 `panic_dma::arm()`，后者通过 `PANIC_DMA_READY.store(true, Ordering::Release)` 发布就绪状态。这时 USART1 已经配置好，DMA1 的时钟也已经打开。 [控制台驱动](../kernel/src/console.rs)

## 2. 进入 panic handler 并取得使用权

Rust 发生 panic 时会进入 `#[panic_handler]`。它通过 `format_args!` 传入消息、有条件地附上源码文件和行号，并添加 ANSI 红色序列。随后由 `console::write_panic()` 进入 DMA 输出，返回后在 `spin_loop()` 中停留。 [panic handler](../kernel/src/panic_handler.rs)

`panic_dma::write()` 先屏蔽普通中断，然后原子地把就绪标志改为 `false`，并取出旧值：

```rust
cortex_m::interrupt::disable();
if !PANIC_DMA_READY.swap(false, Ordering::AcqRel) {
    return;
}
```

- 旧值为 `true` 时，本次 panic 取得缓冲区和 DMA Stream 的使用权。
- 旧值为 `false` 时，初始化尚未完成，或者前一次 panic 已经取得使用权。函数直接返回，handler 随后停机。

原子交换也处理了重入。例如格式化错误信息时再次 panic，第二次调用会看到 `false`，因此会跳过 DMA 和缓冲区访问。交换中的 `Acquire` 与初始化时的 `Release` 配对。 [就绪检查](../kernel/src/console/panic_dma.rs)

## 3. 准备 DMA 能读到的消息

`PANIC_BUFFER` 是静态缓冲区，容纳最多 512 字节，并按 32 字节对齐。它属于 `.bss`，链接脚本把它放在从 `0x24000000` 开始的 AXI SRAM，DMA1 可以访问这里。 [缓冲区实现](../kernel/src/console/panic_dma.rs) · [内存布局](../kernel/memory.x) · [STM32H743 总线与内存映射](https://www.st.com/resource/en/reference_manual/rm0433-stm32h742-stm32h743-753-and-stm32h750-value-line-advanced-armbased-32bit-mcus-stmicroelectronics.pdf)

`buffer.write_fmt(args)` 会调用 `PanicBuffer::write_str()`，把 UTF-8 字节复制进缓冲区。空间不足时，代码按字符边界截断，保留已写入的部分。之后用 `as_ptr()` 取得地址，用 `buffer.len` 取得实际长度。长度为零时直接返回。

如果 Cortex-M7 的 D-Cache 开着，刚格式化的字节可能还在 CPU 缓存中。代码调用 `SCB::clean_dcache_by_address(address, len)`，把覆盖缓冲区的脏缓存行写回内存。最后执行 `dsb()`，保证前面的内存操作完成，再打开 DMA。D-Cache 关闭时，代码仍执行这条屏障。 [缓存处理](../kernel/src/console/panic_dma.rs) · [ST AN4839 缓存维护说明](https://www.st.com/resource/en/application_note/an4839-level-1-cache-on-stm32f7-series-and-stm32h7-series-microcontrollers-stmicroelectronics.pdf)

## 4. 配置寄存器并启动传输

`USART1::ptr()`、`DMA1::ptr()` 和 `DMAMUX1::ptr()` 返回外设寄存器块的裸指针。panic handler 已经屏蔽普通中断并取得使用权，因此可以通过 PAC 直接访问这些寄存器，即使 HAL 的 `Tx<USART1>` 仍由驱动持有。 [寄存器配置](../kernel/src/console/panic_dma.rs)

| 操作 | 写入内容 | 作用 |
| --- | --- | --- |
| 写 `DMA1.LIFCR` | Stream 0 的五个清除位 | 清除上一次的传输完成、过半和错误标志。清除位采用 W1C，写 1 才清除对应标志 |
| 写 `DMAMUX1.CCR[0].DMAREQ_ID` | `USART1_TX` 请求编号 42 | 把 USART1 的发送请求接到 DMA1 Stream 0 |
| 写 Stream 0 的 `PAR` | `USART1.TDR` 的地址 | 指定每个字节写到哪里 |
| 写 Stream 0 的 `M0AR` | 缓冲区起始地址 | 指定第一个字节从哪里读 |
| 写 Stream 0 的 `NDTR` | 消息实际长度 | 设定传输次数。本次每次搬 1 字节 |
| 复位 Stream 0 的 `FCR` | PAC 定义的复位值 `0x21` | 选择 Direct Mode，绕过 DMA 自己的 FIFO |
| 写 Stream 0 的 `CR` | `DIR = memory-to-peripheral`、`MINC = 1` | 源地址每次加 1，外设地址固定，使用 8 位读写 |
| 写 `USART1.ICR.TCCF` | 1 | 清除 USART 的旧 `TC` 标志 |
| 设置 Stream 0 的 `CR.EN` | 1 | 让 DMA Stream 接收请求 |
| 设置 `USART1.CR3.DMAT` | 1 | 让 USART1 在发送端有空位时发出 DMA 请求 |

配置地址的三行代码如下。注意 `&raw const usart.tdr` 取得的是寄存器地址，DMA 要使用这个地址才能写入 `TDR`。

```rust
stream.par.write(|w| unsafe { w.bits(&raw const usart.tdr as u32) });
stream.m0ar.write(|w| unsafe { w.bits(address as u32) });
stream.ndtr.write(|w| w.ndt().bits(len as u16));
```

假设缓冲区里有 `ABC`。USART1 第一次请求时，DMA 从 `address + 0` 读取 `A`，写进 `TDR`。第二、第三次分别从 `address + 1` 和 `address + 2` 读取 `B` 和 `C`，目标始终是同一个 `TDR`。传输次数逐步减至零。USART1 自己负责将字节变成 PA9 上的串行信号。 [STM32H743 USART DMA 发送时序](https://www.st.com/resource/en/reference_manual/dm00314099-stm32h742-stm32h743753-and-stm32h750-value-line-advanced-armbased-32bit-mcus-stmicroelectronics.pdf)

## 5. 等待 DMA 和 USART 完成

普通中断已被屏蔽，但 DMA 和 USART 仍由硬件驱动。CPU 通过轮询读取两个外设的状态：

1. **等待 DMA 传输。** 轮询中发现 `FEIF0` 时清除标志并继续。`TCIF0` 置位且 `NDTR` 归零时结束等待。出现 `TEIF0`、`DMEIF0` 或超时时也结束等待。
2. **关闭请求和 Stream。** 先保存关闭前的状态。完成条件为 `TCIF0` 置位、`NDTR` 为零且 `TEIF0`、`DMEIF0` 均未置位。软件关闭 Stream 也可能置位 `TCIF0`，因此要先判断，再清 `USART1.CR3.DMAT` 和 DMA Stream 0 的 `CR.EN`。
3. **等串口发完。** 当 DMA 已将最后一个字节交给 USART1 时，继续轮询 `USART1.ISR.TC`，等待最后一帧发完。

`wait_until()` 每次调用最多检查 1600 万次。这个上限按轮询次数计算，时间取决于 CPU 频率与构建配置。DMA 出错或超时时仍会执行关闭步骤，串口可能只收到部分消息。 [轮询和收尾代码](../kernel/src/console/panic_dma.rs) · [STM32H743 RM0433](https://www.st.com/resource/en/reference_manual/rm0433-stm32h742-stm32h743-753-and-stm32h750-value-line-advanced-armbased-32bit-mcus-stmicroelectronics.pdf)

## 6. 在板上观察

当前入口在发完启动消息并拉高 PC13 后主动触发 `panic!("Done!")`。用 3.3 V USB 转串口连接 MCU 的 PA9（接转接器 RX）和 GND，终端设为 **19 200 baud、8N1**。复位后应看到带 `[kernel panic]` 前缀的消息。若在驱动初始化前触发 panic，程序会停在 handler，串口跳过发送。

用 VS Code 调试时，可在设置 `DMAT` 前打断点，查看 DMA1 Stream 0 的 `M0AR`、`PAR`、`NDTR` 和 DMAMUX1 的 `CCR[0]`。然后继续运行，让 DMA 和 USART 完成发送。用 `NDTR` 对照还剩多少字节，再用 `TCIF0` 和 `TC` 区分 DMA 搬运结束与串口线上发送结束。
