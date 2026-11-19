# kr-stm-os 批处理系统实施指南

目标是在 STM32H743VGT6 上从片内 Flash 启动内核，从板载 W25Q64JV 读取应用镜像，把每个应用装入 AXI SRAM，逐个在非特权态运行。第一版验收时，串口应显示两个应用按顺序运行并退出，最后显示批处理完成。

当前代码已经有独立的用户 crate、`hello` ELF、应用链接脚本和 Thumb 入口。内核仍然执行启动演示和 `panic!("Done!")`。本文中的 QSPI 驱动、打包器、应用加载器、系统调用和 MPU 配置是**接下来要实现的内容**。板级接线来自你提供的 W25Q64JV 原理图片段，本文没有做实板读写或烧录验证。相关代码见 [内核入口](../src/main.rs)、[板级初始化](../src/board.rs)、[应用链接脚本](../user/link.x) 和 [应用入口](../user/src/entry.s)。

## 1. 先分清三种地址

| 对象 | 地址或范围 | 用途 |
| --- | --- | --- |
| 内核片内 Flash | `0x0800_0000`–`0x0807_FFFF` | 当前链接脚本使用的 Flash Bank 1，放向量表、内核代码和初始化数据 |
| 内核 AXI SRAM | `0x2400_0000`–`0x2405_FFFF` | 内核变量、MSP 栈和 panic DMA 缓冲区 |
| 应用 AXI SRAM | `0x2406_0000`–`0x2407_FFFF` | 同一时间只装一个应用。栈占最后 4 KiB，栈顶是结束地址 `0x2408_0000` |
| W25Q64JV 芯片偏移 | `0x000000`–`0x7FFFFF` | 8 MiB 掉电保留空间。本方案先把 `apps.img` 放在偏移 0 |
| QUADSPI 映射窗口的已装芯片范围 | `0x9000_0000`–`0x907F_FFFF` | 启用 memory-mapped 读后，CPU 可以用窗口基址加 NOR 偏移读取。烧录工具也可用这个基址为外部存储定址 |

例如，镜像里的文件字节位于 NOR 偏移 `0x1000`。间接读传 `0x001000` 给控制器。映射读时访问 `0x9000_1000`。应用真正的运行地址由 ELF 决定，可能是 `0x2406_0000`。这三个数字分别回答“芯片里放哪”“CPU 用哪个窗口读”“程序从哪执行”。链接脚本和烧录地址各自解决其中一个问题。见 [内核内存布局](../memory.x)、[应用内存布局](../user/link.x) 和 [STM32H743 RM0433，Memory map 与 QUADSPI](https://www.st.com/resource/en/reference_manual/rm0433-stm32h7-series-and-stm32h7rs-lines-armbased-32bit-mcus-stmicroelectronics.pdf)。

```text
主机编译内核 ELF ── SWD / probe-rs ──→ 片内 Flash ── 复位启动内核
主机编译应用 ELF ── 打包 apps.img ── SWD / external loader ──→ W25Q64JV
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
- **串口日志** 用 3.3 V TTL USB 转串口。PA9（MCU TX）接转接器 RX，GND 相连。需要接收时再把转接器 TX 接 PA10（MCU RX），当前内核只保留发送句柄。终端选 19200、8N1、无流控。见 [当前板级初始化](../src/board.rs)。

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

1. 在 `src/qspi.rs` 封装 Flash。由 `Board::init()` 在现有 PWR/RCC 初始化之后，配置 GPIOB、GPIOD、GPIOE 的复用和 QUADSPI 时钟及复位，把 Flash 句柄交给 `Board`。PB6 片选也要配置成 AF10。HAL 示例单独设置片选 GPIO，照抄 `bank1()` 的时钟和 IO 参数容易漏掉它。
2. 配置单芯片、Bank 1、低速、符合 NOR 要求的时钟模式和片选高电平间隔。间接普通读使用 24 位地址。控制器的设备大小 `FSIZE` 用的是地址位数减一，8 MiB 等于 `2^23` 字节，对应 `FSIZE = 22`。地址阶段仍发送 24 位，容量和传输地址长度各有用途。
3. 先实现 `jedec_id()`，使其只发指令并读三个字节，不添加地址。接着实现 `read_at(offset, &mut [u8])`，用 `checked_add` 检查读区间，空缓冲区直接返回。底层等待应可超时。带长度上限的 HAL 操作按其支持的大小分块，失败就报告偏移和错误。
4. 用间接读先跑通整个批处理。后续需要改善吞吐量时，再尝试 Quad fast read 或 memory-mapped。映射模式由控制器把 CPU 读转换为 NOR 事务。通过 `0x9000_0000` 窗口直接写内存无法完成 NOR 页编程。本文应用链接到 SRAM，切到 QSPI 原地执行还要重新安排应用链接地址。

现有依赖是 `stm32h7xx-hal 0.16.0`。先看该版本的 QSPI 驱动与 Flash 示例，保留它的传输接口，改成本板接线、单线时序和 W25Q64JV 的命令。首次读出 ID 并确认已有扇区内容后，才做擦写试验。见 [HAL 0.16.0 QSPI 示例](https://docs.rs/crate/stm32h7xx-hal/0.16.0/source/examples/qspi_flash_memory.rs)、[STM32 RM0433](https://www.st.com/resource/en/reference_manual/rm0433-stm32h7-series-and-stm32h7rs-lines-armbased-32bit-mcus-stmicroelectronics.pdf) 和 [ST AN4760，Quad-SPI 工作模式](https://www.st.com/resource/en/application_note/an4760-introduction-to-quadspi-interface-for-stm32-mcus-and-mpus-stmicroelectronics.pdf)。

## 3. 构建、打包和烧录

### 3.1 先认识 ELF 和 apps.img

```bash
cargo build --locked -p user --bin hello
cargo build --locked --release -p user --bins
readelf -h -lW target/thumbv7em-none-eabihf/release/hello
llvm-objdump -d target/thumbv7em-none-eabihf/release/hello
```

ELF 带有入口、段表、符号和调试信息，可用于检查和调试。当前 `hello` 的入口值是 `0x2406_0001`。最低位标记 Thumb，指令实际位于 `0x2406_0000`。应用链接脚本为代码与数据定义不同的 `PT_LOAD`。对每个段，`p_filesz` 是 ELF 里已有的初始化字节数，`p_memsz` 是运行时要占用的内存长度。两者的差额需要在装载时清零。直接把 ELF 文件从开头复制进 SRAM 会把文件头也写到应用入口，CPU 就无法按预期取指。见 [应用链接脚本](../user/link.x) 和 [Arm ELF 规范](https://github.com/ARM-software/abi-aa/blob/main/aaelf32/aaelf32.rst)。

建议单独加主机打包器，例如 `tools/pack-apps`。它按明确列出的顺序读取应用 ELF，检查为小端 ARM 32 位可执行文件，提取入口和 `PT_LOAD`，再生成紧凑的 `apps.img`。显式列出应用可避免把残留构建产物误打包。镜像独立生成，内核也能单独构建和烧录。与 `krunix` 相比，仍然在同一个固定 RAM 区轮流装应用，应用目录改存到 NOR。见 [krunix 的应用目录生成逻辑](https://github.com/KrvyFT/krunix) 的 `os/build.rs` 和 `os/src/batch.rs`（本机仓库 `/home/krvy/Projects/krunix`）。

建议先冻结一版简单格式。整数统一用小端 `u32` 并逐字段编码，镜像偏移一律从镜像首字节算起。

| 内容 | 建议字段 |
| --- | --- |
| 24 B 镜像头 | `magic[4] = "KAPP"`、`version = 1`、`total_len`、`app_count`、`app_table_offset`、`image_crc32` |
| 28 B 应用表项 | `name[16]`（短 ASCII 名，零填充）、`entry`（保留 Thumb 位）、`segment_count`、`segment_table_offset` |
| 20 B 段表项 | `dst_addr`、`payload_offset`、`file_len`、`mem_len`、`flags`（沿用 ELF 的 R/W/X 位） |
| 负载 | 各段的 `file_len` 字节，`mem_len - file_len` 由内核清零 |

打包端和内核使用同一种 CRC-32/ISO-HDLC，计算 `[0, total_len)` 并将 CRC 字段本身的 4 字节按零处理。CRC 用于发现写入或传输损坏。验收时先把容量上限、应用数上限（例如 16）和段数上限写进两端的格式约定，支持空目录正常完成。裸 NOR 的开头通常读到 `FF FF FF FF`，内核应报告未装镜像，停在诊断路径。

### 3.2 烧录时谁负责写 QSPI

| 操作 | 结果 |
| --- | --- |
| `cargo build` | 在电脑上生成 ELF |
| 当前 `cargo run --locked` | 选中内核，通过 probe-rs 烧录内核 ELF 对应的片内 Flash |
| 用 external loader 烧录 `apps.img` | 通过 SWD 把临时烧写程序放进 MCU RAM，由它初始化 QUADSPI、擦扇区、写页并校验 |
| 复位或重新上电 | 临时 loader 的代码失去运行状态，内核重新初始化 QUADSPI 读取已写好的镜像 |

先从板卡供应商找与这六根引脚和 W25Q64JV 同时匹配的 `.stldr`。如果没有，经验证后可用 ST 的 external loader 工程模板改造，设置 `0x9000_0000`、8 MiB、256 B 页和 4 KiB 扇区，按本板初始化 Bank 1。STM32H743I-EVAL 的官方样例适合作为移植参考，它的外部 Flash 型号与本板不同。也可以为 probe-rs 编写匹配的 Flash algorithm 和 target 描述。当前本机 `probe-rs chip info STM32H743VGTx` 即使列出外部 NVM 地址范围，也只凭这个输出无法确认算法、板级引脚和器件一致。见 [ST external loader 工程](https://github.com/STMicroelectronics/stm32-memory-loaders) 和 [probe-rs Flash algorithm 说明](https://probe.rs/docs/knowledge-base/cmsis-packs/)。

匹配的 loader 和 `apps.img` 准备好后，用 CubeProgrammer 的命令行可按下面的形式烧录。需要用支持该工具的探针，例如 ST-LINK。路径是待替换的示例。

```bash
STM32_Programmer_CLI -c port=SWD \
  -el /path/to/board-w25q64jv.stldr \
  -w apps.img 0x90000000 -v
```

这里的 `0x90000000` 是 loader 对外公布的基址，对应 NOR 内部偏移 0。先确认该区域没有需要保留的数据。先用一个已确认空闲的扇区写短样本，核对回读和断电重启后的结果，再烧正式镜像。若没有合适的 loader，可写单独的板上测试固件，经 SWD 把样本放进 SRAM，调用 NOR 擦写函数并回读。正式内核第一版保留读取能力就够了。见 [STM32CubeProgrammer CLI 的 `-w`、`-v` 和 `-el`](https://dev.st.com/stm32cube-docs/prog/2.23.0/en/docs/markup/CubeProg_Command_Lines.html)。

## 4. 启动后怎么把 QSPI 里的程序搬进内存

建议把镜像解析和 SRAM 写入分开。先让只读解析器输出目录，然后再用经过验证的描述写内存。这样损坏的镜像头可以被拒绝，而不会先改掉 RAM。

1. 从 NOR 偏移 0 读固定长度的头，检查魔数、版本、总长和应用数。逐字段读为小端整数，检查 `offset + length` 和 `count × record_size` 有无溢出。确认各表和负载都落在声明的镜像范围中，最后流式核对 CRC。
2. 为当前应用验证每个段。要求 `file_len <= mem_len`、目标范围落在 `[0x2406_0000, 0x2407_F000)`、本应用的段互不重叠、文件字节落在负载区。检查入口有 Thumb 位，且清掉最低位后落在可执行段的已初始化字节中。传给 CPU 的目标地址因此来自验证通过的数据。
3. 应用停止后，用固定大小缓冲区按块从 `payload_offset` 读 `file_len` 字节，写到 `dst_addr`。再把 `[dst_addr + file_len, dst_addr + mem_len)` 清零。第一版可以在装载前清理整个应用区，避免下一个应用读到上一轮的栈和未用空隙。用原始指针写计划好的范围，整个装载期间内核独占应用区。任一读失败都跳过执行，报告应用和段号。
4. 如果 D-Cache 已开启，CPU 写入的指令字节可能还留在 D-Cache。完成复制后清理涉及的 D-Cache 地址范围并同步，若 I-Cache 已启用，失效相关取指缓存，再执行所需屏障。初版可沿用内核的缓存配置，单独验证有缓存时的代码重载。维护范围要按缓存行要求对齐。后续若改用 DMA，DMA 写入与 CPU 缓存的同步方向要另行设计。见 [ST AN4839，Cache maintenance](https://www.st.com/resource/en/application_note/an4839-level-1-cache-on-stm32f7-series-and-stm32h7-series-microcontrollers.pdf) 和 [当前 panic DMA 文档](dma-panic.md)。
5. 在调试器查看 `0x2406_0000` 开始的内存，与 ELF 的反汇编比较。检查 `.data` 初值、`.bss` 和预留栈范围，最后才进入应用。建议第二个应用包含一个有初值的全局变量和一个零初值变量，运行时实际读取它们，验证装载和复用。

