# kr-stm-os

面向 STM32H743VGT6 的 Rust 裸机项目，为后续编写操作系统建立启动和诊断基础。当前固件初始化板级外设，通过 USART1 输出启动消息，切换 PC13 电平，然后主动触发一次 panic 来演示 DMA 输出。

## 构建与烧录

安装 Rust stable、目标组件和 probe-rs。仅编译时需要 Rust 工具链和目标组件。

```sh
rustup target add thumbv7em-none-eabihf
cargo build --locked
cargo build --release --locked
```

调试版 ELF 默认生成在 `target/thumbv7em-none-eabihf/debug/kr-stm-os`。连接 ST-Link 等 SWD 探针后，用以下命令确认 probe-rs 能访问探针，再构建、烧录并运行固件。

```sh
probe-rs list
cargo run --locked
```

Cargo runner 使用 `probe-rs run --chip STM32H743VGTx`。VS Code 可以安装 probe-rs 调试扩展，通过 **Debug STM32H743VGT6** 配置构建和启动调试。配置使用仓库内的 STM32H743 SVD 文件显示外设寄存器。Linux 上若探针显示 `inaccessible`，先检查 USB 设备权限及 udev 规则。[probe-rs 安装](https://probe.rs/docs/getting-started/installation/) · [探针配置](https://probe.rs/docs/getting-started/probe-setup/)

## 查看串口输出

将 MCU 的 PA9 接到 3.3 V USB 转串口的 RX，两侧 GND 相连。只观察输出时，PA10 可以留空。串口终端设为 **19 200 baud、8N1、无流控**。打开串口后复位，预期看到：

```text
kr-stm-os booted

[kernel panic]
Panicked at src/main.rs:<行号> Done!
```

panic 段带 ANSI 红色序列。启动期间 PC13 先被拉低，在启动消息的最后一帧发完后被拉高。LED 亮灭取决于板载电路的极性。panic handler 最后进入自旋，复位可再次运行此演示。

## 开发文档

- [代码结构与启动流程](docs/architecture.md) 说明模块职责、链接布局、控制台接口和验证方式。
- [DMA panic 输出](docs/dma-panic.md) 说明缓冲区、缓存维护、寄存器顺序和板上观察点。
