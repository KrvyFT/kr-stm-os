# kr-stm-os

面向 STM32H743VGT6 的 Rust 裸机项目，为后续编写操作系统建立启动和诊断基础。当前固件初始化板级外设，通过 USART1 输出启动消息，切换 PC13 电平，读取并打印 QSPI Flash 的 JEDEC ID，然后主动触发一次 panic 来演示 DMA 输出。

## 项目结构

| 路径 | 职责 |
| --- | --- |
| [`kernel/`](kernel/) | 内核源码、板级驱动和内核链接布局 |
| [`user/`](user/) | 用户库、示例应用和应用链接布局 |
| [`tools/`](tools/) | 应用打包器、QSPI loader 和调试器使用的 SVD |
| [`docs/`](docs/) | 架构、DMA 调试和批处理实现文档 |
| `target/` | Cargo 构建缓存和 ELF |
| `output/` | 应用镜像、QSPI 读回文件等导出产物 |

仓库根目录是 Cargo workspace 和 Makefile 的入口，默认构建内核。

## 构建与烧录

安装 Rust stable、目标组件和 GNU Make。编译生成的内核 ELF 位于 `target/thumbv7em-none-eabihf/debug/kr-stm-os`。

```sh
rustup target add thumbv7em-none-eabihf
make                      # 构建调试版内核
make PROFILE=release      # 构建发布版内核
make build-apps           # 构建 user crate 的应用 ELF
make pack-apps            # 构建应用并打包为 output/apps.img（需要 Python 3.9+）
```

使用 STM32CubeProgrammer 烧录时，将 ST-LINK 的 SWD 和 NRST 接到板上。`make flash-kernel` 重新构建内核，并按 ELF 中的地址写入片内 Flash 后校验。默认连接参数是 Under Reset、硬件复位和 100 kHz。可用 `SWD_ARGS='port=SWD mode=UR reset=HWrst freq=950'` 覆盖。CLI 名称不同时，可以传入 `PROGRAMMER=STM32_Programmer_CLI`。

```sh
make flash-kernel
make qspi-loader
make qspi-read
make pack-apps
make qspi-write
```

`qspi-loader` 下载并校验 DevEBox 的 W25Q64 loader，默认保存到 `tools/flash/STM32H743VI_W25Q64.stldr`。其他板卡使用匹配的 `QSPI_LOADER=/绝对路径/board.stldr`。`qspi-read` 默认从 `0x90000000` 读 4096 字节到 `output/qspi-read.bin`，可以用 `QSPI_ADDR`、`QSPI_SIZE`（字节数）和 `QSPI_OUTPUT` 指定读回范围和文件。

`pack-apps` 先构建用户程序，再按 `user/src/bin/*.rs` 的文件名升序打包。目前顺序为 `00hello`、`01store_fault`。可用 `APPS` 显式指定应用列表和顺序，用 `PROFILE=release` 打包发布版，用 `OUTPUT_DIR` 指定镜像和 QSPI 读回文件的目录，或用 `QSPI_IMAGE` 单独指定镜像路径。见[镜像格式](docs/batch-implementation.md#31-先认识-elf-和-appsimg)。

`qspi-write` 先通过 ST-LINK 的 NRST 对 STM32 执行一次硬件复位，再重新连接，从 `QSPI_ADDR` 写入已有镜像并校验。起点须按 4 KiB 扇区对齐，写入前确认覆盖到的整个扇区都可擦除。使用自定义镜像路径时，给打包和烧录命令传入相同的 `QSPI_IMAGE`。

也可以用 probe-rs 烧录并运行内核。

```sh
probe-rs list
cargo run --locked
```

Cargo runner 使用 `probe-rs run --chip STM32H743VGTx`。VS Code 可以安装 probe-rs 调试扩展，通过 **Debug STM32H743VGT6** 配置构建和启动调试。配置使用仓库内的 STM32H743 SVD 文件显示外设寄存器。Linux 上若探针显示 `inaccessible`，先检查 USB 设备权限及 udev 规则。[probe-rs 安装](https://probe.rs/docs/getting-started/installation/) · [探针配置](https://probe.rs/docs/getting-started/probe-setup/)

## 查看串口输出

将 MCU 的 PA9 接到 3.3 V USB 转串口的 RX，两侧 GND 相连。只观察输出时，PA10 可以留空。串口终端设为 **19 200 baud、8N1、无流控**。打开串口后复位，预期看到：

```text
kr-stm-os booted
<三字节 JEDEC ID>

[kernel panic]
Panicked at kernel/src/main.rs:<行号> Done!
```

panic 段带 ANSI 红色序列。启动期间 PC13 先被拉低，在启动消息的最后一帧发完后被拉高。LED 亮灭取决于板载电路的极性。panic handler 最后进入自旋，复位可再次运行此演示。

## 开发文档

- [代码结构与启动流程](docs/architecture.md) 说明模块职责、链接布局、控制台接口和验证方式。
- [DMA panic 输出](docs/dma-panic.md) 说明缓冲区、缓存维护、寄存器顺序和板上观察点。
- [批处理系统实施指南](docs/batch-implementation.md) 从 W25Q64JV 接线、烧录和装载讲到 Cortex-M7 应用切换及串口共享。
