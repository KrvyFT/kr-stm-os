# 代码结构与启动流程

## 源码入口

| 文件 | 职责 |
| --- | --- |
| [`src/main.rs`](../src/main.rs) | 定义 `#[entry]`，串联板级初始化、启动输出、PC13 电平切换和 panic 演示 |
| [`src/board.rs`](../src/board.rs) | 取得 STM32 外设所有权，配置供电和时钟，初始化 GPIO、USART1 和 DMA1 |
| [`src/console.rs`](../src/console.rs) | 管理普通输出队列，通过 HAL 轮询发送，保存 DMA1 Stream 0 的所有权 |
| [`src/console/panic_dma.rs`](../src/console/panic_dma.rs) | 在 panic 路径中格式化消息、维护 D-Cache 并直接操作 USART1 和 DMA 寄存器 |
| [`src/panic_handler.rs`](../src/panic_handler.rs) | 从 `PanicInfo` 提取消息与源码位置，调用 panic 输出后自旋 |
| [`memory.x`](../memory.x)、[`build.rs`](../build.rs) 和 [`.cargo/config.toml`](../.cargo/config.toml) | 定义内核的内存区域和链接参数，以及项目的交叉编译目标和烧录 runner |

`main.rs` 使用 `#![no_std]` 和 `#![no_main]`。复位后，`cortex-m-rt` 提供的启动代码准备 RAM 中的静态数据，包括初始化 `.data` 和清零 `.bss`，随后调用 `#[entry]` 指定的 `main()`。

`Board::init()` 使用 `pac::Peripherals::take()` 取得外设实例。它先配置 PWR 和 RCC，再初始化 PA9、PA10 和 PC13。PA9 和 PA10 使用 AF7，由 USART1 分别作为 TX 和 RX。PC13 配置为推挽输出。USART1 初始化后只保留发送句柄。DMA1 开启时钟后，Stream 0 随 `ConsoleDriver` 一起交给入口函数，控制台此时才允许 panic DMA 接管发送。

入口函数把启动消息放入队列，把 PC13 拉低，调用 `wait_for_write()` 等待串口发完，再把 PC13 拉高并调用 `panic!("Done!")`。这个顺序让同一串口上的启动消息先于 panic 消息完整发送。

## 链接与内存

Cargo 默认使用 `thumbv7em-none-eabihf`，对应当前 Cortex-M7 固件的指令集和硬浮点 ABI。内核的 `build.rs` 只给 `kr-stm-os` 二进制传入 `-Tlink.x`，因此 workspace 中的 `user` 可以定义自己的链接布局。内核使用 `cortex-m-rt` 的链接脚本，从 `memory.x` 读取以下区域：

| 区域 | 起始地址 | 项目使用范围 | 用途 |
| --- | --- | --- | --- |
| FLASH | `0x0800_0000` | 512 KiB | 向量表、程序代码、只读数据和 `.data` 的初始值 |
| 内核 RAM | `0x2400_0000` | 384 KiB | AXI SRAM，存放内核 `.data`、`.bss` 和栈 |
| 应用 RAM | `0x2406_0000` | 128 KiB | AXI SRAM，由 [`user/link.x`](../user/link.x) 定义，各应用分时使用，顶部预留 4 KiB 栈 |

panic 缓冲区位于 AXI SRAM 的 `.bss`。如果将来调整链接布局，需要继续把它放在 DMA1 可访问的内存，并保留 32 字节对齐。[DMA 缓冲区与缓存维护](dma-panic.md#3-准备-dma-能读到的消息)

`cargo build --locked -p user --bin hello` 生成应用 ELF。应用以 [`_start`](../user/src/entry.s) 为入口，尾跳转至 Rust 的 `user_main`，把返回值留在 `r0`。后续加载器负责复制可加载段、清零零填充区域、设置应用栈和安排返回地址。`hello` 目前只返回 `0`，用于验证链接和入口。

## 普通串口输出

- `console::try_print(&str)` 尝试把一条消息完整追加到 1 024 字节的队列。单条上限为 128 字节，超过时返回 `WriteError::TooLong`。剩余空间不足时返回 `WriteError::Full`。两种失败均发生在写入队列之前，长度按 UTF-8 字节计算。
- `ConsoleDriver::poll()` 每次尝试发送一个字节。USART1 暂时无法接收下一个字节时，驱动将该字节留在 `pending`，下一次轮询先重试它。
- `ConsoleDriver::wait_for_write()` 反复调用 `poll()`，直到队列为空、没有待重试字节，且 HAL 的 `flush()` 确认 USART 的 `TC` 置位。这是阻塞等待，依赖 USART 正常推进，当前用于启动演示的输出顺序。

访问队列时用 Cortex-M 临界区屏蔽普通中断，USART 写入由持有驱动的入口代码执行。普通输出的进度由调用方轮询推动。panic handler 可能在队列操作尚未结束时接管，因此使用独立缓冲区，直接访问 USART1 和已保留的 DMA Stream。

## 验证

```sh
cargo fmt --all -- --check
cargo clippy --locked -- -D warnings
cargo build --locked
cargo build --release --locked
```

这组命令覆盖格式、静态检查和两种构建配置的链接。板上验证以复位后的两段串口输出、PC13 电平顺序和 DMA 寄存器状态为准。[接线与预期输出](../README.md#查看串口输出) · [DMA 调试步骤](dma-panic.md#6-在板上观察)
