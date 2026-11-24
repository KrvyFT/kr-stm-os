# 代码结构与启动流程

## 源码入口

| 文件 | 职责 |
| --- | --- |
| [`kernel/src/main.rs`](../kernel/src/main.rs) | 定义 `#[entry]`，串联板级初始化、启动输出、批处理和结束后的 panic 演示 |
| [`kernel/src/board.rs`](../kernel/src/board.rs) | 取得 STM32 外设所有权、配置时钟和引脚，构造板上实际使用的 Flash 和串口驱动 |
| [`kernel/src/drivers.rs`](../kernel/src/drivers.rs) | 组织可供板级代码和内核逻辑使用的设备驱动 |
| [`kernel/src/drivers/qspi.rs`](../kernel/src/drivers/qspi.rs) | 封装 W25Q64JV 的 JEDEC ID 和按偏移读取操作 |
| [`kernel/src/drivers/console.rs`](../kernel/src/drivers/console.rs) | 管理普通输出队列，通过 HAL 轮询发送，保存 DMA1 Stream 0 的所有权 |
| [`kernel/src/drivers/console/panic_dma.rs`](../kernel/src/drivers/console/panic_dma.rs) | 在 panic 路径中格式化消息、维护 D-Cache 并直接操作 USART1 和 DMA 寄存器 |
| [`kernel/src/panic_handler.rs`](../kernel/src/panic_handler.rs) | 从 `PanicInfo` 提取消息与源码位置，调用 panic 输出后自旋 |
| [`kernel/src/batch.rs`](../kernel/src/batch.rs) | 打开 KAPP 镜像，逐个装载、运行应用并报告退出码 |
| [镜像解析](../kernel/src/batch/image.rs)与[段装载](../kernel/src/batch/loader.rs) | 检查 CRC 和应用地址范围，将段从 QSPI 复制到 AXI SRAM 并清零段尾 |
| [`kernel/src/trap.rs`](../kernel/src/trap.rs) 与 [`svc.s`](../kernel/src/trap/svc.s) | Rust 构造初始异常帧，汇编处理 SVC、MSP/PSP 切换和退出码返回 |
| [`memory.x`](../kernel/memory.x)、[`build.rs`](../kernel/build.rs) 和 [`.cargo/config.toml`](../.cargo/config.toml) | 定义内核的内存区域和链接参数，以及项目的交叉编译目标和烧录 runner |

`main.rs` 使用 `#![no_std]` 和 `#![no_main]`。复位后，`cortex-m-rt` 提供的启动代码准备 RAM 中的静态数据，包括初始化 `.data` 和清零 `.bss`，随后调用 `#[entry]` 指定的 `main()`。

`Board::init()` 使用 `pac::Peripherals::take()` 取得外设实例。它先配置 PWR 和 RCC，再初始化 PA9、PA10 和 PC13。PA9 和 PA10 使用 AF7，由 USART1 分别作为 TX 和 RX。PC13 配置为推挽输出。USART1 初始化后只保留发送句柄。DMA1 开启时钟后，Stream 0 随 `ConsoleDriver` 一起交给入口函数，控制台此时才允许 panic DMA 接管发送。

入口函数把启动消息放入队列，把 PC13 拉低，调用 `wait_for_write()` 等待串口发完，再把 PC13 拉高。批处理从 `Board` 借用 Flash 和控制台，先校验镜像，再顺序装载和执行应用。加载或校验错误沿 `Result` 返回给入口，统一打印诊断。日志发送完毕后，入口保留 `panic!("Done!")` 的 DMA 演示。

板级代码负责构造驱动，所以批处理接收的是已经初始化的 W25Q64JV 和控制台句柄。应用始终复用同一段 SRAM，`batch` 要等到 `trap::run_app()` 返回后才能装载下一个。 [应用切换和实板验证](svc-app-start-exit.md)

## 链接与内存

Cargo 默认使用 `thumbv7em-none-eabihf`，对应当前 Cortex-M7 固件的指令集和硬浮点 ABI。内核的 `build.rs` 只给 `kr-stm-os` 二进制传入 `-Tlink.x`，因此 workspace 中的 `user` 可以定义自己的链接布局。内核的构建脚本把 `kernel/` 加入链接器搜索路径，让 `cortex-m-rt` 的 `link.x` 找到定义以下区域的 `kernel/memory.x`。

| 区域 | 起始地址 | 项目使用范围 | 用途 |
| --- | --- | --- | --- |
| FLASH | `0x0800_0000` | 512 KiB | 向量表、程序代码、只读数据和 `.data` 的初始值 |
| 内核 RAM | `0x2400_0000` | 384 KiB | AXI SRAM，存放内核 `.data`、`.bss` 和栈 |
| 应用 RAM | `0x2406_0000` | 128 KiB | AXI SRAM，由 [`user/link.x`](../user/link.x) 定义，各应用分时使用，顶部预留 4 KiB 栈 |

panic 缓冲区位于 AXI SRAM 的 `.bss`。如果将来调整链接布局，需要继续把它放在 DMA1 可访问的内存，并保留 32 字节对齐。[DMA 缓冲区与缓存维护](dma-panic.md#3-准备-dma-能读到的消息)

`cargo build --locked -p user_lib --bin 00hello` 生成应用 ELF。内核装载可加载段并设置 PSP，应用从 Rust 的 [`_start`](../user/src/runtime.rs) 进入，调用 `main()`，再把返回值交给 `exit()`。`00hello` 在返回前会请求用户态写入，当前内核的 SVC 处理只支持进入和退出。

用户库的 [`lib.rs`](../user/src/lib.rs) 从运行时导出应用使用的 `write` 和 `exit` 接口。[用户态控制台](../user/src/console.rs) 将格式化结果交给 `write`，运行时内部的[系统调用模块](../user/src/runtime/syscall.rs) 处理 SVC 编号和传参寄存器。

## 主机打包器

[命令行模块](../tools/pack/src/cli.rs) 只解析参数，[生成流程](../tools/pack/src/pack.rs) 负责读取输入、检查文件冲突并写出完整镜像。[ELF 解析](../tools/pack/src/elf.rs) 和 [KAPP 编码](../tools/pack/src/image.rs) 分别处理两种文件格式。入口统一打印结果，因此可从参数、输入文件和镜像编码三个层次定位问题。

## 普通串口输出

- `console::try_print(&str)` 尝试把一条消息完整追加到 1 024 字节的队列。单条上限为 128 字节，超过时返回 `WriteError::TooLong`。剩余空间不足时返回 `WriteError::Full`。两种失败均发生在写入队列之前，长度按 UTF-8 字节计算。
- `ConsoleDriver::poll()` 每次尝试发送一个字节。USART1 暂时无法接收下一个字节时，驱动将该字节留在 `pending`，下一次轮询先重试它。
- `ConsoleDriver::wait_for_write()` 反复调用 `poll()`，直到队列为空、没有待重试字节，且 HAL 的 `flush()` 确认 USART 的 `TC` 置位。这是阻塞等待，依赖 USART 正常推进，当前用于启动演示的输出顺序。

访问队列时用 Cortex-M 临界区屏蔽普通中断，USART 写入由持有驱动的入口代码执行。普通输出的进度由调用方轮询推动。panic handler 可能在队列操作尚未结束时接管，因此使用独立缓冲区，直接访问 USART1 和已保留的 DMA Stream。

## 验证

```sh
cargo fmt --all -- --check
cargo fmt --manifest-path tools/pack/Cargo.toml -- --check
cargo build --locked
cargo build --release --locked
```

这组命令覆盖格式和两种构建配置的链接。板上可以分别观察应用退出码、PC13 电平顺序和随后 DMA 发送的 panic 文本。 [接线与预期输出](../README.md#查看串口输出) · [DMA 调试步骤](dma-panic.md#6-在板上观察)
