# 第二章　批处理系统

本章从 STM32H743VGT6 上的一个现象开始。板子复位之后，串口先报告内核启动，然后先后报告两个应用的退出码。两个应用都在同一个 AXI SRAM 地址运行，分别返回 42 和 7。它们与内核分别构建，存在外部 QSPI Flash 的同一份镜像中。只要更换镜像里的应用顺序，内核就会按新的顺序装载和运行。

原书第二章按特权级、应用程序、批处理装载、异常切换以及练习展开。本章沿用这个学习次序，围绕当前 `kr-stm-os` 的源码重新组织实验。阅读时也可以把实现与 `krunix` 的 RISC-V 路径并排放在一起，观察两边如何用不同的硬件接口实现相同的控制流。 [rCore 教程第二章](https://rcore-os.cn/rCore-Tutorial-Book-v3/chapter2/index.html)

## 2.1 引言：先看一个完整的批次

### 2.1.1 一次复位如何运行两个程序

设想手里有两份计算程序。第一份计算结束得到 42，第二份得到 7。最朴素的实验方式是把它们各自编译成一套裸机固件，轮流烧录整个程序，手工重启板子，等待每个程序完成。这样调试一次计算尚可，应用每增加一个，就要多做一次烧录和复位。板级初始化、串口输出和结束处理也会在不同的固件中重复出现。

批处理把这几个动作归入内核。它维护一份应用次序，能找到任意一项的数据，按应用预期的地址完成装载，然后把 CPU 交给这项程序。应用计算完成后通过固定入口把退出码交回。内核观察到本项结束，才有权覆盖原来的应用代码和工作区。这就把上一个应用的退出和下一个应用的装载变成一段可以自动重复的控制流。外部仍需要预先说明有哪些应用，但运行期间无需按应用切换固件。

这里的“批”是一个有顺序的目录。它可以只有一项，也可以有多项。目录顺序是一种简单而可观察的调度决策。应用仍然一项一项执行，处理器在任何时刻最多运行这一批中的一个应用。两项实验的价值在于可以检查它们是否真的经历了完整的交接，而不是内核某处碰巧打印了预定消息。如果第二项从未被装载，肯定拿不到它自己的退出码。如果恢复内核时寄存器没有恢复正确，第一项可能结束，第二项却无法开始。

本章最小实验用 42 和 7 作为退出标记。任选两个不同整数同样能做实验。取非零值有助于把真实的返回值和零值初始化区分开，使用两个不同的值还能发现第二次仍沿用第一次结果的问题。它们是应用通过 `main()` 返回的状态，不参与地址计算、SVC 分派或芯片寄存器配置。即使改用 47 和 57，也只会改变串口日志里的退出数字。能否看到这些数字取决于应用实际是否走到了退出调用。

### 2.1.2 镜像、内存和控制流分三层

在这台板上，一个批次涉及三类对象。第一类是主机上的 ELF。每个应用独立链接，ELF 说明入口、段地址和段长度，并携带调试器可能用到的符号。第二类是烧进板载 Flash 的应用镜像。它按内核方便读取的方式排列目录和段数据，并且可以独立于内核更新。第三类是 CPU 此刻正在执行的内存与寄存器状态。执行时处理器从 AXI SRAM 取应用指令，入口、栈顶和执行权限还要由内核传给处理器。

把三类对象分开以后，能解释为什么“烧录已完成”还不足以推断程序能跑。烧录校验通常负责验证外置芯片里已有正确字节。内核还要能够打开镜像，计算 CRC，找到段源地址，再把每段搬到正确的 SRAM 目的地。即使 SRAM 内容完全正确，若初始 PSP 指错，异常返回也无法恢复入口。最后，即使应用已执行，如果退出 handler 没保留内核现场，批处理仍然不能进入下一项。

文档的验证顺序刻意沿这三层推进。主机先用 ELF 工具核对入口和段，再用打包器生成明确顺序的镜像。上板先看启动日志和 QSPI 读回，然后确认 SRAM 段的位置，最后用两个 SVC 断点看 CPU 切换的前后状态。这样每次排查只需回答眼前一个问题。例如串口只有启动提示时，不必立刻猜测 `r4` 是否恢复错了，先看内核有没有走到启动 SVC。若连启动分支都没进，问题已经在更窄的范围内。

```text
主机   应用源码 -> 链接后的 ELF -> KAPP 镜像
                                    |
板级   W25Q64JV <-------------------+ 烧录
           |
内核   镜像目录 -> 校验 -> AXI SRAM -> 准备应用异常帧
                                             |
应用                       执行 _start <- 异常返回
                                  |
内核             下一项装载 <- 恢复现场 <- SVC exit
```

### 2.1.3 用哪些文件回答哪些问题

第一次读代码可以从入口顺着调用方向走。`kernel/src/main.rs` 先让 `Board::init()` 完成时钟、引脚、串口和 QSPI 的初始化，再把可用的 Flash 和控制台交给 `batch::run()`。如果复位时连启动消息都没有，先回到板级代码和串口连接检查，因为应用装载还没开始。 [内核入口](../kernel/src/main.rs) · [板级初始化](../kernel/src/board.rs)

`batch::run()` 知道什么时候运行下一项。它向 `Image` 询问应用数量，让 `loader::load()` 完成内存准备，再把入口和栈顶交给 `trap::run_app()`。它等待返回，只在拿到退出码后推进迭代。若某项镜像校验或装载失败，错误会沿 `Result` 返回，由内核入口打印。追踪应用顺序时先读这个循环，就能判断“下一项是谁”与“某一项怎么进入 CPU”分别由谁决定。 [批处理入口](../kernel/src/batch.rs)

想弄懂为什么从 Flash 偏移 0 读取、为什么每轮搬运时两个地址都要增长，可以转到镜像解析器和装载器。镜像解析器的职责是约束来源和目的地，装载器有了这些约束才能用裸指针进行复制。两层之间的接口返回应用的入口和栈顶，把磁盘格式、Flash 读操作和 CPU 现场分隔开。 [镜像解析器](../kernel/src/batch/image.rs) · [段加载器](../kernel/src/batch/loader.rs)

想跟踪 `main()` 返回的整数，则从用户运行时进入。应用入口 `_start()` 已经提供通用启动和退出处理，应用自己的 `main()` 只实现要计算的内容。运行时把状态作为系统调用参数交给汇编，异常 handler 接管以后再把它写入内核的续执行现场。从源码看，整数跨过的是用户与内核的边界，从机器状态看，跨的是两份异常帧。 [用户运行时](../user/src/runtime.rs) · [SVC 汇编](../kernel/src/trap/svc.s)

### 2.1.4 本章的验收边界

学完这里，应当能从空白的测试文件开始，构造一个整数应用，编译、打包并找到它的 Thumb 入口。把单应用镜像扩大成两项之后，应能解释为什么二者可以共用起始地址，以及为什么不能把它们同时加载到这个地址。到异常部分，要能指出进入内核时哪条栈保存了现场，handler 的 LR 和栈里的 LR 分别属于什么，`svc #0` 与 `svc #1` 如何共享同一个向量。

当前内核已经支持启动、退出和用户态写入。先用纯整数应用定位启动与退出，再用 `00hello` 验证 `svc #2` 和串口发送顺序。 `01store_fault` 需要单独分析，目前内核尚未实现应用 Fault 恢复。 [SVC 汇编](../kernel/src/trap/svc.s)

测试镜像由自己构建并用已知应用组成。CRC 可以发现内容变化或读取错误，特权状态限制部分硬件操作，MPU 才为具体地址范围提供运行期的访问约束。三者负责不同阶段，合起来才能说明板子运行程序时有怎样的保障。此处先把控制流接通，再逐步加入串口服务、故障归属和内存保护，实验结果要和当前实现区分开。

## 2.2 特权、异常与两条栈

### 2.2.1 为什么普通函数调用解决不了问题

假设两个应用都要发串口。如果直接把 USART 寄存器地址或驱动对象交给应用，那么任何程序都能在自己的循环里写寄存器。第一个应用刚改过串口配置，第二个应用继承什么状态就难以预期。内核也无法统一定义输出长度、队列顺序和设备错误处理。把写串口的服务留在内核中，应用只提出“输出这些数据”的请求，就能让设备操作遵循一个地方的规则。退出也是类似的请求。应用知道计算完成，却不应自己决定下一项镜像存在哪里或怎样覆盖内存。

把一个 Rust 函数写成 private 可以限制其他模块如何调用它，这是编译阶段的边界。处理器执行机器码时看到的是指令、地址和当前状态。应用与内核各自编译，通过链接脚本放进不同区域，是为了把代码与数据摆在约定的位置。运行过程中 PC 执行什么、硬件对敏感操作允许到什么程度，还需要处理器支持。操作系统把已有的硬件机制和自己的调用约定组合起来，才能让应用只经明确的边界请求内核服务。

在 Cortex-M7 上，把内核函数的地址写进某个应用程序，并执行一次普通分支，只会在原有执行模式和权限下改变 PC。处理器执行 `svc` 时，会按异常进入机制保存返回所需的状态，再改用异常 handler。改变的是 CPU 的执行环境，而不仅是代码的位置。这就是应用适合用 SVC 访问内核服务的原因。进入后内核仍要验证调用号和参数，因为获得特权的是 handler，用户传来的值仍由应用控制。 [Arm Cortex-M7 异常类型](https://support.arm.com/documentation/dui0646/c/The-Cortex-M7-Processor/Exception-model/Exception-types?lang=en)

### 2.2.2 Thread、Handler 与特权级

学习 Cortex-M 时容易把“模式”和“特权”合成一个概念。模式回答正在执行哪类控制流。Thread mode 用于通常的程序执行，Handler mode 用于异常处理。特权回答能执行哪些受限操作。Thread mode 可以是特权或非特权，Handler mode 始终有特权。当前项目从复位后进入的内核普通代码属于特权 Thread mode，应用启动后进入非特权 Thread mode，两个 SVC 都会让 CPU 临时进入 Handler mode。

| 所处位置 | 执行模式 | 权限 | 当前栈 |
| --- | --- | --- | --- |
| 板级启动与批处理循环 | Thread | 特权 | MSP |
| 应用入口与应用主函数 | Thread | 非特权 | PSP |
| SVC 处理代码 | Handler | 特权 | MSP |

内核启动应用时，需要同时回答两个问题。第一，如何让 CPU 选定 PSP 取出应用的寄存器。第二，回到 Thread mode 时以什么权限继续运行。异常返回令牌说明返回的模式、栈和帧类型。`CONTROL.nPRIV` 决定 Thread 的权限。应用的入口地址在初始帧的 PC 槽里。三部分分别处理不同的事，所以只把 PC 改到应用地址，或者只设置 PSP，都不足以完成这个交接。反过来退出时，也要恢复内核的 MSP 路径和特权 Thread 状态。

从调试器看，断在异常 handler 里时，当前指令确实正在特权态执行。这个时刻要判断之前应用是不是非特权，可以结合进入前的 `CONTROL`、handler LR 和帧位置。若停在用户入口，`CONTROL` 的 `nPRIV` 和 `SPSEL` 两位在本实现中都应为 1。若停在内核已经从 SVC 返回的续执行点，`nPRIV` 应回到 0，线程栈应为 MSP。把“当前 CPU 在哪里”和“被保存的现场属于谁”区分开，能避免观察一个寄存器就误判状态。 [断点位置与状态](svc-app-start-exit.md#6-在调试器里逐站验证)

### 2.2.3 为什么要让应用使用另一条栈

栈保存函数调用期间需要暂存的内容，例如返回地址、寄存器和局部变量。如果两个程序共用同一条栈，并在彼此不知情时改变其布局，回来以后就不容易知道哪一片空间对应原来的函数。这里给内核和应用各自一套栈指针。应用运行时，内核原来的调用链和暂停现场留在 MSP 所指向的内存里。应用在 PSP 上建立自己的调用链，正常退出就可以丢弃整个调用现场。

Cortex-M7 进入异常时，会先在被打断代码使用的栈上写异常帧，然后才切到 Handler 使用的 MSP。所以“handler 使用 MSP”和“用户现场在 PSP 上”会同时成立。以 `exit(42)` 为例，应用在执行 SVC 前确实把 42 放在了 `r0`。异常进入后，handler 可以自由将当前 `r0` 用作指针，因为原来的 42 已经有一份保存到 PSP。要取回应用传递的 42，就应该读应用的异常帧，而不是把此时调试器里的实时 `r0` 当作参数。

另一种情况是内核主动发出的 `svc #0`。异常前它在特权 Thread mode 使用 MSP，硬件于是也把基本帧压在 MSP 上。进入 handler 时，SP 仍选择 MSP，但当前栈顶已是硬件刚压好的帧。汇编再往更低地址留出软件保存区。把这些数据留在那里之后，通过异常返回从 PSP 恢复应用。进入应用之后 MSP 还是那份暂停现场所在的位置。退出 handler 再从 MSP 恢复内核时，必须恰好把 SP 推回硬件帧起点。只差一个字，接下来恢复到 `PC` 和 `xPSR` 的值就可能来自错误的槽位。

这两次异常的栈变动可以按时间看。第一次 SVC 把内核帧压到 MSP。启动分支给 MSP 再压一份保存区，并把 PSP 指向应用初始帧。进入应用时，CPU 从 PSP 弹出初始帧，然后以普通调用方式使用用户栈。第二次 SVC 把实时应用状态压回 PSP。退出分支取出应用的值，弹出 MSP 的软件保存区，最后让硬件从 MSP 弹出当初的内核帧。动画式地跟着指针移动，比把所有帧想成同一个长数组更容易定位错误。

### 2.2.4 异常向量与调用编号各负责什么

SVC 有固定的异常入口。当内核和应用各执行一条不同的 `svc` 指令时，硬件都会从异常向量表取相同的 `SVCall` 入口地址。单个入口可以服务多种请求，因为进入 handler 以后可以从保存的状态判断来源，再按调用约定选择处理分支。当前代码先看 handler LR 的 `EXC_RETURN`。从 MSP 进入的启动路径还校验返回 PC 对应内核的指定调用点，从 PSP 进入的退出路径则读出 SVC 指令并确认编号为 1。 [向量与分派源码](../kernel/src/trap/svc.s)

这里要区分三个数字。异常向量的位置用于挑选硬件入口，`svc #1` 的 1 是用户调用号，应用的 42 是退出参数。它们都可能以整数形式出现，职责不同。调试时最有效的问法是“这个数从哪来，接下来谁读它”。例如 `0xdf01` 来自应用代码内存，低八位表示退出请求。42 来自应用异常帧的第一个字。handler LR 中的 `0xffff_fffd` 来自硬件，包含返回是一份 PSP 基本帧的信息。三者碰巧出现在同一段汇编中，读取位置和含义各有边界。

异常入口也不是每次都由应用主动触发。应用执行非法指令可能触发 Fault，定时器和串口等外设也可能产生 IRQ。它们都能打断正在执行的代码，但触发原因不同，应用能否继续要分别判断。SVC 有明确的调用约定，普通服务可返回到调用之后。Fault 需要先确认异常的归属与帧是否有效，再考虑终止本应用或进入内核诊断。外设 IRQ 一般应尽快处理事件，并使原来被打断的逻辑继续。本章只依赖 SVC，先把可控路径跑通，之后再用同样的栈和异常知识理解其他入口。

### 2.2.5 链接地址、装载校验和 MPU 的边界

应用的链接脚本提供一个受约束的地址布局。应用代码和数据在自己的区域内，栈在另一段有明确定义的区间，内核链接脚本给它们留出空间。这个安排先保护了工程构建过程。如果全局数据太多或段冲到栈区，链接器或打包器能在程序上板前及时报告。应用段进入镜像以后，内核仍会检查来源和目的地址，因为烧录文件可能损坏，也可能来自不同版本的构建。只有区间检查通过，加载器才能把文件中声明的地址转换为写入 SRAM 的指针。

应用正在执行时，它的任何一次读写都由 CPU 发出。装载器已经完成了任务，无法再拦截每一条指令。要让它的代码只能访问指定 RAM，就要配 MPU 区域和访问权限。将来在板上验证这项能力时，先让有意越界的测试程序从正确的地址执行，再观察是否触发预期的 Fault。没有这些前置检查，任何早于测试语句的错误都可能被误读为“保护生效了”。类似地，CRC 通过只能表明镜像与其记录的校验值一致，出处与信任策略由烧录流程另外决定。

PCB 和片上的存储区域还会对操作行为产生差异。例如向未映射地址访问，可能以总线错误告终。向确实映射、但被 MPU 禁止的地址访问，则需要按相应的内存管理故障路径判断。当前应用测试里的非法写设计要等故障入口和权限设置之后再作为验收项。先前建立的双应用退出实验可以作为对照组，串口日志、内核状态、批处理推进顺序都可复用到故障实验中。

### 2.2.6 对照 RISC-V 时怎样读得更快

rCore 的第二章用 RISC-V 的 U-mode 与 S-mode 描述用户和内核边界。应用发起 `ecall` 后，处理器记录陷入原因与指令地址，并进入设置好的异常入口。现场中的通用寄存器由汇编保存，栈的切换通常借助 `sscratch`。这与 Cortex-M 的自动压基本帧是两套接口。拿两份实现来对照时，先抽取彼此共有的目标，例如保留旧寄存器、切换到安全的栈、恢复入口，再去找各自完成目标的代码。 [rCore 的特权级模型](https://rcore-os.cn/rCore-Tutorial-Book-v3/chapter2/1rv-privilege.html) · [rCore 的 Trap 处理](https://rcore-os.cn/rCore-Tutorial-Book-v3/chapter2/4trap-handling.html)

| 观察的问题 | RISC-V 路径 | Cortex-M7 路径 |
| --- | --- | --- |
| 硬件怎样找 handler | 由 `stvec` 选择入口 | 从异常向量表取 handler 地址 |
| 进入后如何找到用户栈 | 软件通过 `sscratch` 交换 SP | PSP 和 MSP 分别保留，处理器依据模式使用对应栈 |
| 返回地址在哪里 | 保存到异常 PC 寄存器，软件可写回 | 基本帧里有一个 `PC` 槽 |
| 怎么决定下一段权限 | `sstatus` 等状态参与 `sret` | `CONTROL.nPRIV` 指定 Thread 权限 |
| 哪条指令完成交接 | `sret` | Handler 向 PC 装入 `EXC_RETURN`，本项目用 `bx lr` |

这张表也能解释有些操作只在一侧出现。RISC-V 上软件必须安排何处保存通用寄存器，因此先执行的若干条栈相关指令很关键。Cortex-M 在进入 handler 之前已经写好一组寄存器，入口更适合先辨认帧类型。软件仍然需要对未自动保存的寄存器负责，特别是当前实现会从内核切到另一条线程控制流，不能依赖普通 handler 立刻返回原线程的假设。

现在可以回到本章最初的两个应用。我们需要把程序放到 CPU 将要执行的地址上，同时准备出一份可被硬件理解的现场。下一节先完成二者的前半段。让应用能独立链接，弄清 `_start` 属于谁，再用 ELF 的程序头把它变成 KAPP 中的一项。

## 2.3 实现可装载的应用

### 2.3.1 用同一个地址运行不同的程序

一个能由芯片复位直接启动的固件，要给出复位向量、初始栈指针，还要负责它自己的运行时初始化。批处理中的应用由已在运行的内核接手。假设内核刚执行完一次 `exit`，下一份程序此刻仍是一串存放在外部 Flash 的字节。内核必须知道哪些字节要读，放到什么地址，需要额外准备多少内存，以及装好以后从哪条指令开始。应用的构建产物要准确记录这些约定，否则应用源码即使能通过 Rust 的类型检查，进入 CPU 时也可能在一片错误的内存上工作。

本章每次只运行一个应用，因而所有应用都能在编译时约定同一个地址。`kernel/memory.x` 把 D1 AXI SRAM 开始的 384 KiB 分给内核，剩余 128 KiB 由 `user/link.x` 划成 124 KiB 应用装载区和 4 KiB 应用栈。下表区间使用左闭右开记法，`0x2407_F000` 正好既是装载区的末端，又是栈区的起点。装载阶段在 `0x2407_EFFF` 写最后一个字节仍合法，写到 `0x2407_F000` 就越过了装载区。`0x2408_0000` 是空栈的初始边界，入栈时地址向低处移动。

| 用途 | 地址范围 | 大小 |
| --- | --- | ---: |
| 内核实际链接使用的 D1 AXI SRAM | `[0x2400_0000, 0x2406_0000)` | 384 KiB |
| 应用代码、常量和数据 | `[0x2406_0000, 0x2407_F000)` | 124 KiB |
| 应用 PSP 栈 | `[0x2407_F000, 0x2408_0000)` | 4 KiB |

CPU 取指时使用链接所确定的地址，编译器生成的代码也可能按这些地址找静态数据。固定地址使装载器只需恢复链接器安排的内存内容，省去了动态重定位。让两个应用同时留在相同地址会让后装入者覆盖前者的指令和数据。批处理循环必须等当前应用交回控制权才能重用这一区间。链接脚本里的边界能约束生成的 ELF 及装载器接受的目标范围，运行期间的访存保护还需硬件配置。项目目前没有配置应用 MPU 区域，应用栈也没有由这套常量提供自动溢出保护。 [内核内存布局](../kernel/memory.x) · [应用链接脚本](../user/link.x)

### 2.3.2 链接脚本描述运行时布局

`user/build.rs` 给该 crate 的二进制目标传入 `-T…/link.x`，并用 `rerun-if-changed` 使 Cargo 在脚本变化后重新处理构建。这个作用域允许多个应用各自得到同样布局的独立 ELF。用户库提供运行时函数，针对各应用生成的二进制才需要绑定那一份 SRAM 布局。

```ld
ENTRY(_start)

MEMORY
{
    /* Top 128 KiB of D1 AXI SRAM, reserved by the kernel's memory.x. */
    APP   (rwx) : ORIGIN = 0x24060000, LENGTH = 124K
    STACK (rw)  : ORIGIN = 0x2407F000, LENGTH = 4K
}

PHDRS
{
    code PT_LOAD FLAGS(5); /* Read and execute */
    data PT_LOAD FLAGS(6); /* Read and write */
}

SECTIONS
{
    .text ORIGIN(APP) :
    {
        KEEP(*(.text.entry))
        *(.text .text.*)
    } > APP :code

    .rodata :
    {
        *(.rodata .rodata.*)
    } > APP :code

    .ARM.extab :
    {
        *(.ARM.extab .ARM.extab.*)
    } > APP :code

    .ARM.exidx :
    {
        *(.ARM.exidx .ARM.exidx.*)
    } > APP :code

    .data : ALIGN(8)
    {
        *(.data .data.*)
    } > APP :data

    .bss (NOLOAD) : ALIGN(8)
    {
        *(.bss .bss.*)
        *(COMMON)
    } > APP :data

    /* The kernel initializes the PT_LOAD segments and sets PSP before entry. */
    __user_stack_bottom = ORIGIN(STACK);
    __user_stack_top = ORIGIN(STACK) + LENGTH(STACK);
    ASSERT((_start & ~1) == ORIGIN(APP), "user entry must start at APP origin")
}
```

`MEMORY` 中的 `APP` 是链接器放置代码和数据的范围。`PHDRS` 单独指定两类 `PT_LOAD` 程序头，`code` 的 `5` 对应 `PF_R | PF_X`，`data` 的 `6` 对应 `PF_R | PF_W`。`SECTIONS` 把输入 section 合成输出 section，并用 `> APP :code` 或 `> APP :data` 关联到相应段。这个关联影响 ELF 的段布局，装载器读到的权限位也来自这里。当前装载器用可执行位核对入口，运行时内存权限仍取决于硬件配置。

`.text` 直接从 `ORIGIN(APP)` 开始。`KEEP(*(.text.entry))` 把入口段保留在其余 `.text` 之前，即使链接器执行未使用 section 的回收也会保留这段。`ENTRY(_start)` 让 ELF 文件头的 `e_entry` 指向 `_start`，文件末尾的 `ASSERT` 再确认入口去掉 Thumb 标志后确实位于应用区起点。两条规则分别处理入口的记录与入口的位置，因此即便日后有人调整 section 顺序，链接阶段也能及时指出入口漂移。

`.rodata` 跟在指令后，例如格式字符串的字节需要在运行时可读，所以也进入 `code` 段。脚本同样将 `.ARM.extab` 与 `.ARM.exidx` 纳入这类装载段。如果工具输出额外列出一个 `ARM_EXIDX` 程序头，其中描述的字节可能已经处于一个 `LOAD` 的覆盖范围，打包器只按 `PT_LOAD` 进行一次装载。脚本声明了 `data` 程序头，只有程序确实留下了相应的非空运行时数据时，ELF 才会有需要这份加载动作的范围。观察段数时要按编译产物判断。

`.data` 保存带初值、需要在运行时存在的可写内容。`.bss (NOLOAD)` 为零初值数据安排地址，并使它在段末端以运行时占用表达。两段内存都要真实存在，初始化方式由它们在可装载段中的位置决定。`ALIGN(8)` 约束 `.data` 和 `.bss` 的布局。段内为对齐留下的空隙也应通过 `p_filesz` 和 `p_memsz` 理解，二者的差表示需要补零的整个尾部，具体有多少字节属于某个变量还要结合 section 或符号表。

`__user_stack_bottom` 和 `__user_stack_top` 只是链接符号，声明这两个地址不会给镜像增加 4 KiB 的零。装载器通过内核的 `APP_STACK_TOP` 传递栈顶，启动代码据此构造应用的初始异常栈帧。链接脚本、打包器范围检查和内核常量是三个需要联动的地方。改变 SRAM 分区时，保留区、程序头目标地址及装载器允许的范围都要重新核对，否则典型结果是打包时的 `address range outside application SRAM`，或板上 `InvalidSegment`。 [用户 crate 构建脚本](../user/build.rs) · [通用 ELF 程序头规范](https://gabi.xinuos.com/elf/08-pheader.html)

### 2.3.3 `_start` 把内核启动约定交给应用的 `main`

每个应用二进制是 `#![no_std]` 和 `#![no_main]`。应用提供一个导出符号名为 `main` 的 Rust 函数，用户库中的运行时定义 `_start`。应用进入 `_start` 时，SRAM 的相关段已经初始化，应用栈已经准备好。这样同一份运行时代码可以供所有应用复用，应用源码只写它要做的工作以及返回值。

```rust
mod syscall;

use core::panic::PanicInfo;

use crate::println;

const PANIC_EXIT_CODE: i32 = 1;

pub fn write(bytes: &[u8]) -> i32 {
    syscall::sys_write(bytes)
}

/// Ends the current application with the given status.
pub fn exit(code: i32) -> ! {
    let _ = syscall::sys_exit(code);

    // The kernel should never resume an exited app. Avoid panicking here
    // because the panic handler itself calls exit.
    loop {
        core::hint::spin_loop();
    }
}

unsafe extern "Rust" {
    fn main() -> i32;
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
extern "C" fn _start() -> ! {
    let code = unsafe { main() };
    exit(code)
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    if let Some(location) = info.location() {
        println!(
            "Panicked at {}:{} {}",
            location.file(),
            location.line(),
            info.message()
        );
    } else {
        println!("Panicked: {}", info.message());
    }
    exit(PANIC_EXIT_CODE)
}
```

`#[unsafe(no_mangle)]` 使链接器能用 `_start` 找到入口，`#[unsafe(link_section = ".text.entry")]` 将它放入前面脚本特意保留的段。`unsafe extern "Rust"` 声明 `main` 由另一个编译单元提供，调用处依赖应用确实导出这一名字和签名。应用的 `main` 可以返回 `i32`，`_start` 返回 `!`，原因是它必须在 Rust 函数返回后再把结果转换为应用退出请求。进入应用时没有用户态调用者可供 `_start` 通过普通函数返回指令回去。`exit` 调用约定的 `svc #1`。若该调用意外返回，末尾的忙等保持 `!` 的语义，同时避开 panic 处理器再次调用 `exit` 的递归路径。

`write` 通过 `svc #2` 提交原始字节。用户态 `panic` 先尝试打印，再调用退出。 `00hello` 提交打印后继续执行，其 `main()` 返回 `47`。整数测试单独覆盖装载与退出，随后再用该示例观察写入。 [用户调用封装](../user/src/runtime/syscall.rs)

### 2.3.4 从 ELF 的程序头提取加载动作

ELF 同时描述链接结果和运行所需数据。section 适合解释 `.text`、`.rodata`、`.bss` 及调试信息各自的归属，program header 则从装载器角度给出连续的拷贝与清零动作。一个 `PT_LOAD` 可以覆盖多个 section。主机打包器只借助 program header 取装载范围，原 ELF 中保留给调试器使用的符号表和调试 section 无需随应用一起写入 Flash。看一个新生成的应用 ELF 时，优先用 `readelf -W -h` 查看文件头和入口，用 `readelf -W -l` 查看装载程序头及 section 到 segment 的映射，再按需用 `readelf -W -S` 看 section 在哪一段。

| ELF32 字段 | 所在位置 | 在项目中的用途 |
| --- | --- | --- |
| `e_entry` | ELF 头偏移 `24` | 保留为应用记录的 `entry`，最低位携带 Thumb 状态信息 |
| `e_phoff` | ELF 头偏移 `28` | 定位 program header 表在原 ELF 文件的位置 |
| `e_ehsize`、`e_phentsize`、`e_phnum` | ELF 头偏移 `40`、`42`、`44` | 核对 ELF 头大小及表项宽度，由表项数量确定表的读取范围 |
| `p_type` | 程序头偏移 `0` | 只处理 `PT_LOAD`，其编号是 `1` |
| `p_offset` | 程序头偏移 `4` | 初始化字节在 ELF 文件中的位置 |
| `p_vaddr` | 程序头偏移 `8` | 本项目实际写入的 SRAM 地址 `dst_addr` |
| `p_filesz`、`p_memsz` | 程序头偏移 `16`、`20` | 需要复制的长度和整个段在 SRAM 中的长度 |
| `p_flags` | 程序头偏移 `24` | 原样记录段权限位，其中 `PF_X` 参与入口验证 |

通用 ELF 分别定义 `p_vaddr` 与 `p_paddr`，这个打包器按 `p_vaddr` 使用用户脚本设定的运行地址。当前配置没有让应用地址经 MMU 转换，`dst_addr` 最后会直接转为 SRAM 指针。`p_align` 等其他字段也没有参与当前装载，程序头检查以项目的固定地址和范围约束为准。这是面向已链接 ARM ELF32 应用的加载子集，普通宿主程序的 ELF 即使也有 `PT_LOAD` 也不能按此约定直接运行。 [ELF 程序头规范](https://gabi.xinuos.com/elf/08-pheader.html) · [Arm AAELF32 入口约定](https://github.com/ARM-software/abi-aa/blob/main/aaelf32/aaelf32.rst)

```rust
const ELF_HEADER_SIZE: usize = 52;
const PROGRAM_HEADER_SIZE: usize = 32;
const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;

const APP_LOAD_START: u32 = 0x2406_0000;
// The last 4 KiB of user SRAM is reserved for the stack.
const APP_LOAD_END: u32 = 0x2407_F000;
const MAX_SEGMENTS: usize = 16;

pub(super) struct Segment {
    pub dst_addr: u32,
    pub payload: Vec<u8>,
    pub mem_len: u32,
    pub flags: u32,
}

pub(super) fn parse(elf: &[u8]) -> Result<(u32, Vec<Segment>), String> {
    let header = elf
        .first_chunk::<ELF_HEADER_SIZE>()
        .ok_or("truncated ELF header")?;
    let file_type = read_u16(header, 16);
    let machine = read_u16(header, 18);
    let version = read_u32(header, 20);
    // ELFCLASS32, ELFDATA2LSB, EV_CURRENT, ET_EXEC and EM_ARM.
    if &header[..7] != b"\x7fELF\x01\x01\x01" || (file_type, machine, version) != (2, 40, 1) {
        return Err("expected a little-endian ARM ELF32 executable".into());
    }

    let entry = read_u32(header, 24);
    let table_start = read_u32(header, 28) as usize;
    let header_size = usize::from(read_u16(header, 40));
    let entry_size = usize::from(read_u16(header, 42));
    let entry_count = usize::from(read_u16(header, 44));
    if header_size != ELF_HEADER_SIZE
        || table_start < ELF_HEADER_SIZE
        || entry_size != PROGRAM_HEADER_SIZE
    {
        return Err("invalid ELF program header table".into());
    }
    let table = elf
        .get(table_start..)
        .and_then(|tail| tail.get(..entry_size * entry_count))
        .ok_or("invalid ELF program header table")?;

    let mut segments: Vec<Segment> = Vec::new();
    let (records, _) = table.as_chunks::<PROGRAM_HEADER_SIZE>();
    for (index, record) in records.iter().enumerate() {
        if read_u32(record, 0) != PT_LOAD {
            continue;
        }
        let file_offset = read_u32(record, 4) as usize;
        let dst_addr = read_u32(record, 8);
        let file_len = read_u32(record, 16);
        let mem_len = read_u32(record, 20);
        let flags = read_u32(record, 24);

        if file_len > mem_len {
            return Err(format!("PT_LOAD {index}: file size exceeds memory size"));
        }
        // A zero-file-size segment has no source bytes to read.
        let payload = if file_len == 0 {
            &[]
        } else {
            elf.get(file_offset..)
                .and_then(|tail| tail.get(..file_len as usize))
                .ok_or_else(|| format!("PT_LOAD {index}: file range outside ELF"))?
        };
        if mem_len == 0 {
            continue;
        }
        let end = dst_addr
            .checked_add(mem_len)
            .filter(|&end| dst_addr >= APP_LOAD_START && end <= APP_LOAD_END)
            .ok_or_else(|| format!("PT_LOAD {index}: address range outside application SRAM"))?;
        if segments
            .iter()
            .any(|segment| dst_addr < segment.dst_addr + segment.mem_len && segment.dst_addr < end)
        {
            return Err(format!("PT_LOAD {index}: overlaps another load segment"));
        }
        if segments.len() == MAX_SEGMENTS {
            return Err(format!(
                "at most {MAX_SEGMENTS} load segments per application"
            ));
        }
        segments.push(Segment {
            dst_addr,
            payload: payload.to_vec(),
            mem_len,
            flags,
        });
    }

    // Keep the Thumb bit in the image, but check the actual instruction address.
    let entry_addr = entry & !1;
    if entry & 1 == 0
        || !segments.iter().any(|segment| {
            segment.flags & PF_X != 0
                && (segment.dst_addr..segment.dst_addr + segment.payload.len() as u32)
                    .contains(&entry_addr)
        })
    {
        return Err("entry must be a Thumb address in initialized executable data".into());
    }
    Ok((entry, segments))
}

// Callers provide a complete ELF header or program header and fixed field offsets.
fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
```

读解析函数可沿三层范围推进。第一层确认 52 字节 ELF32 文件头存在，验证魔数、32 位、小端、格式版本、可执行文件类型和 ARM 机器编号。文件头长度、程序头起点和固定的 32 字节表项宽度通过检查后，用两次 `get` 取得完整的程序头表。表项计数来自 `u16`，宽度已经固定为 `32`，可从这些字段推出乘积上界。表切片建立后，才可在其 32 字节记录中按固定偏移解码。

第二层把单个段的源范围和目标范围分开。它先检查 `p_filesz <= p_memsz`，随后用 `p_offset` 在原始 ELF 取出相应长度的源切片。遇到 `p_filesz == 0`，这一段没有源字节，可以直接产生空 payload。`p_memsz` 也为零时没有任何动作，记录会跳过。对有运行时内容的段，`checked_add` 把不合法的地址回绕直接当作错误。例如 `0xFFFF_FFF0 + 0x30` 在 32 位无符号整数上无法表达正确末端。若使用回绕结果 `0x20` 后只比较上界，就会把一个跨过地址空间尽头的区间当作落在很低的位置。随后目标段必须完全落在应用装载区内，并通过互不重叠检查。旧段的终点可以用直接加法，因为插入旧段前同一段已通过溢出和边界验证。

第三层核对入口。Thumb ELF 的 `e_entry` 最低位为 `1` 用来携带状态，实际第一条指令仍是半字对齐的地址。以 `0x2406_0001` 为例，`entry & !1` 得 `0x2406_0000`，验证应在这个偶地址进行，而镜像仍保存原始 `entry`。入口必须落入可执行段的初始化字节范围。假设可执行段 `dst_addr = 0x2406_0000`，`file_len = 0x40`，`mem_len = 0x80`，`0x2406_003F` 经掩码后是 `0x2406_003E`，仍在已初始化部分。`0x2406_0041` 经掩码后是 `0x2406_0040`，恰落在补零尾部的起点，因此被拒绝。这是一项范围验证，打包器也就无需解码 Thumb 指令或判断这条指令的具体语义。 [主机 ELF 解析源码](../tools/pack/src/elf.rs)

### 2.3.5 可执行文件和镜像有各自的用途

`make build-apps` 用 `thumbv7em-none-eabihf` 为仓库中的用户二进制生成 ELF。`make pack-apps` 依赖这个目标，然后用 `host-tuple` 构建并运行 Rust 打包器。两个命令的工具链目标不同有直接原因。应用将在 Cortex-M7 上执行，而打包器必须在主机上访问文件系统、分配可增长的 `Vec`、处理命令行参数。`APPS` 默认取 `user/src/bin/*.rs` 的文件名升序，在本仓库是 `00hello`、`01store_fault`。显式设 `APPS='01store_fault 00hello'` 会改变传给打包器的顺序，前置的 `build-apps` 仍编译所有用户二进制。此处的顺序关系可用 `make -n pack-apps` 确认，板上是否会运行到下一项取决于前一项能否正常交回控制权。

`pack::write` 依次规范化路径、读取输入、解析 ELF 并建立打包对象。输入按命令行遍历，名称集合只用于判断重复，实际排列由 `Vec` 保留。名称允许 1 至 16 个 ASCII 字节，字符集为英文字母、数字、下划线、点和连字符。对于带 `.elf` 后缀的输入，名称去掉这一后缀。其余输入从完整文件名取名。这使设备可以用定长 16 字节安装目录，日志也能输出经过检查的名称。主机先完成所有校验和编码，随后在输出同一目录独占创建临时文件。`write_all` 成功后才 `rename` 到目标路径，因此某个应用的 ELF 解析报错不会把已有镜像替换成残缺结果。镜像写入的对象是主机文件，Flash 擦写和板上读取在后续各有自己的入口。 [Makefile](../Makefile) · [打包流程](../tools/pack/src/pack.rs) · [rCore 的应用实现与构建](https://rcore-os.cn/rCore-Tutorial-Book-v3/chapter2/2application.html)

## 2.4 实现批处理装载

### 2.4.1 KAPP 在内核与应用之间建立独立的交付边界

内核留在片内 Flash，应用集合位于片外 W25Q64JV。打包器把多个独立 ELF 转成一个 KAPP v1 镜像，镜像前面写目录，后面按段写初始化字节。内核启动时先打开目录，随后按索引装载应用。重新组合应用顺序或替换某个应用，主机只需要根据现有格式重做 `apps.img`。这个布局把适合主机处理的 ELF 解析放在打包阶段，使 `no_std` 内核只实现短定长记录、小端整数解码和流式读取。

这个镜像涉及三类地址。ELF 的 `p_offset` 只应拿到主机的 ELF 中去索引。`app_table_offset`、`segment_table_offset` 和 `payload_offset` 都是相对 KAPP 首字节的偏移，内核把它们送给 Flash 的 `read_at`。`dst_addr` 则是 CPU 地址，最后用于 SRAM 写入。烧录器接受的默认 `QSPI_ADDR = 0x90000000` 是工具的外部 Flash 映射基址。当前 `Image::open` 发送的是偏移 `0`，因此镜像应放在 NOR 起始处。如果烧录命令选择了对齐的 `0x90001000`，镜像位于 NOR 偏移 `0x1000`，内核依然从偏移 `0` 读，改变 Makefile 参数并不会更新内核的查找位置。 [QSPI 烧录目标](../Makefile) · [内核镜像解析](../kernel/src/batch/image.rs)

### 2.4.2 按字节读懂 KAPP v1

镜像中的 `u32` 一律用小端写入。镜像头固定 24 字节，应用表每项 28 字节，段表每项 20 字节。偏移量以字节计，表内字段的下列偏移相对于当行所属记录的起点。镜像的最外层布局可以画成四个连续区间。

```text
0            24                  24 + 28 × A                payload_start       total_len
|  镜像头     |  A 项应用记录       |  按应用排列的各段表          | 段初始化字节         |
```

| 所属记录 | 字段偏移 | 字段 | 含义 |
| --- | ---: | --- | --- |
| 镜像头 | `0` | `magic[4]` | `KAPP` 的四个 ASCII 字节，用来识别格式 |
| 镜像头 | `4` | `version` | 本实现接受 `1` |
| 镜像头 | `8` | `total_len` | 从首字节到负载末端的全部镜像长度 |
| 镜像头 | `12` | `app_count` | 应用表项数，最多 16 |
| 镜像头 | `16` | `app_table_offset` | 应用表起点，v1 当前实现固定为 `24` |
| 镜像头 | `20` | `image_crc32` | 小端 CRC32，验证时在计算中视为四个零字节 |
| 应用记录 | `0` | `name[16]` | 目录名。长度小于 16 字节时用零补齐 |
| 应用记录 | `16` | `entry` | ELF 的 Thumb 入口值，保留最低位 |
| 应用记录 | `20` | `segment_count` | 该应用的可加载段数，范围 1 至 16 |
| 应用记录 | `24` | `segment_table_offset` | 该应用首条段记录的镜像偏移 |
| 段记录 | `0` | `dst_addr` | 将来写入的 SRAM 起始地址 |
| 段记录 | `4` | `payload_offset` | 该段初始化字节在镜像中的起始偏移 |
| 段记录 | `8` | `file_len` | 从镜像复制的字节数 |
| 段记录 | `12` | `mem_len` | 该段在 SRAM 占用的总字节数 |
| 段记录 | `16` | `flags` | 来自 ELF `p_flags`，入口检查使用最低可执行位 |

例如有 A、B 两个应用，A 有两段，B 有一段，初始化字节长度依次取 `300`、`12`、`40`。镜像头为 `[0, 24)`，应用表为 `[24, 80)`，A 的段表在 `[80, 120)`，B 的段表在 `[120, 140)`。第一段的 `payload_offset = 140`，相邻段依此推进到 `440` 和 `452`，而 `total_len = 492`。如果 A 的第一段需要在 SRAM 占据 `320` 字节，镜像仍只给它 300 个初始化字节。剩余 20 个零从装载规则得出。按这个算例，把第一个应用记录起点 `24` 加上 `entry` 的字段偏移 `16`，就能在镜像绝对偏移 `40` 读入口。如果它是 `0x2406_0001`，在那里应看到 `01 00 06 24`。

携带显式偏移让内核可以从目录定位某一项。编码器先算出全部表的长度，然后维护 `segment_offset` 和 `payload_offset` 两条游标，这样写段记录时就已知道该段初始化字节的最终位置。没有初始化字节的段仍可占一条记录来表达纯补零，形式为 `file_len = 0, mem_len > 0`。这类段不会推进 payload 游标，下一段可以使用相同的 `payload_offset`。分析镜像时要结合源区间的长度判断。起点相同的两条记录，可能分别表示空区间和非空区间。

```rust
use crate::elf::Segment;

const HEADER_SIZE: usize = 24;
const APP_RECORD_SIZE: usize = 28;
const SEGMENT_RECORD_SIZE: usize = 20;
const CRC_OFFSET: usize = 20;
const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;

pub(super) const MAX_APPS: usize = 16;

pub(super) struct App {
    pub name: [u8; 16],
    pub entry: u32,
    pub segments: Vec<Segment>,
}

pub(super) fn encode(apps: &[App]) -> Result<Vec<u8>, String> {
    let segment_count = apps.iter().map(|app| app.segments.len()).sum::<usize>();
    let payload_len = apps
        .iter()
        .flat_map(|app| &app.segments)
        .map(|segment| segment.payload.len())
        .sum::<usize>();

    // Lay out both tables before the payload so the records can store absolute image offsets.
    let mut segment_offset = HEADER_SIZE + apps.len() * APP_RECORD_SIZE;
    let mut payload_offset = segment_offset + segment_count * SEGMENT_RECORD_SIZE;
    let total_len = payload_offset + payload_len;
    if total_len > MAX_IMAGE_BYTES {
        return Err("image exceeds the 8 MiB QSPI flash capacity".into());
    }

    let mut image = vec![0; total_len];
    image[..4].copy_from_slice(b"KAPP");
    write_u32s(
        &mut image[4..HEADER_SIZE],
        [
            1,
            total_len as u32,
            apps.len() as u32,
            HEADER_SIZE as u32,
            0,
        ],
    );

    for (index, app) in apps.iter().enumerate() {
        let app_offset = HEADER_SIZE + index * APP_RECORD_SIZE;
        let record = &mut image[app_offset..app_offset + APP_RECORD_SIZE];
        record[..16].copy_from_slice(&app.name);
        write_u32s(
            &mut record[16..],
            [app.entry, app.segments.len() as u32, segment_offset as u32],
        );

        for segment in &app.segments {
            write_u32s(
                &mut image[segment_offset..segment_offset + SEGMENT_RECORD_SIZE],
                [
                    segment.dst_addr,
                    payload_offset as u32,
                    segment.payload.len() as u32,
                    segment.mem_len,
                    segment.flags,
                ],
            );
            segment_offset += SEGMENT_RECORD_SIZE;

            // Only initialized bytes occupy flash. The kernel zeroes the rest in SRAM.
            let payload_end = payload_offset + segment.payload.len();
            image[payload_offset..payload_end].copy_from_slice(&segment.payload);
            payload_offset = payload_end;
        }
    }

    // The CRC covers the entire image with the checksum field still zero.
    let checksum = crc32(&image);
    image[CRC_OFFSET..HEADER_SIZE].copy_from_slice(&checksum.to_le_bytes());
    Ok(image)
}

fn write_u32s<const N: usize>(output: &mut [u8], values: [u32; N]) {
    debug_assert_eq!(output.len(), N * size_of::<u32>());
    let (words, _) = output.as_chunks_mut::<{ size_of::<u32>() }>();
    for (bytes, value) in words.iter_mut().zip(values) {
        *bytes = value.to_le_bytes();
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    // Reflected IEEE CRC32, matching zlib.crc32 in the Python packer.
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 0 {
                crc >> 1
            } else {
                (crc >> 1) ^ 0xedb8_8320
            };
        }
    }
    !crc
}
```

主机编码用 `to_le_bytes()` 和固定段长显式指定字节顺序。固件读取时由 `from_le_bytes` 反解。这个约定比直接把 Rust 结构体的内存内容写入文件更稳定。结构体布局可能引入填充，裸指针读取多字节字段还会牵涉对齐要求。两端各自用内存数据结构保存已解析的值，用定宽字段序列定义文件。需要演进格式时，改变版本及相应字段规则即可明确定义兼容范围。 [主机 KAPP 编码源码](../tools/pack/src/image.rs)

### 2.4.3 从外部 Flash 读取镜像

板级初始化配置 QUADSPI 的时钟、片选和 IO 引脚，然后把 HAL 的 `Qspi` 实例包进 `W25q64jv`。此后上层只使用按片内偏移读取的接口。外设读取时，CPU 把指令和地址交给控制器，控制器从串行总线取回数据，并经数据寄存器把字节交给软件。这是间接读模式。因而 `read_at(0, &mut raw)` 可以读到镜像首字节，代码也没有理由把 `0x9000_0000` 转成指针直接解引用。后者需要另外配置 QUADSPI 的内存映射模式，访问模型和这里的函数不同。

```rust
pub fn read_at(&mut self, offset: u32, buffer: &mut [u8]) -> Result<(), FlashError> {
    const FLASH_SIZE: u32 = 8 * 1024 * 1024;
    const MAX_READ_SIZE: usize = 32;
    let len = u32::try_from(buffer.len()).map_err(|_| FlashError::OutOfBounds)?;
    let end = offset.checked_add(len).ok_or(FlashError::OutOfBounds)?;

    if end > FLASH_SIZE {
        return Err(FlashError::OutOfBounds);
    }

    let mut addr = offset;
    for chunk in buffer.chunks_mut(MAX_READ_SIZE) {
        self.qspi
            .read_extended(
                QspiWord::U8(0x03),
                QspiWord::U24(addr),
                QspiWord::None,
                0,
                chunk,
            )
            .map_err(FlashError::Qspi)?;

        addr += chunk.len() as u32;
    }

    Ok(())
}
```

W25Q64 的 64 Mbit 换算为 `64 × 1024 × 1024 / 8` 字节，也就是 8 MiB。`read_at` 先把调用者的缓冲长度转成 `u32`，再用 `checked_add` 计算请求末端。`end > FLASH_SIZE` 时返回 `OutOfBounds`。例如最后一个字节偏移为 `0x007F_FFFF`，从这里读一字节的末端为 `0x0080_0000`，正好合法。从这个偏移读两个字节时末端超出边界。零长切片不会生成任何事务，因而不会调用 HAL 对非零长度的底层接口；对边界的数学检查仍会执行。这个检查约束整条请求，哪怕一条请求将被拆成若干合规的小事务，也不能靠拆分绕过芯片容量边界。

每个小事务发送命令 `0x03`，用 `U24(addr)` 指定 24 位字节地址，没有 alternate bytes，也没有 dummy cycle。三个地址字节足以寻址 8 MiB NOR。每次读后地址累加这一块实际读出的请求长度，以便写入下一段切片。`jedec_id` 使用 `0x9F` 进行另一种事务，用于读设备标识。当前启动主路径直接尝试打开镜像。单独调试硬件时，JEDEC ID 可用来先确认命令和数据方向，再研究镜像解析。

这里的 32 字节是当前 HAL `read_extended` 接口支持的单次 FIFO 读取上限。使用 `chunks_mut(32)` 后，调用者可以请求较长切片，驱动负责把它分成合规的事务。HAL 等待完成和外设空闲时使用轮询，当前封装也没有增加超时。`FlashError::Qspi` 能带回 HAL 已返回的错误，硬件始终没有满足轮询条件时仍可能卡在事务里。排查串口既无应用结果也无 `KAPP error` 的情况，需要区分这些轮询与已经返回错误的解析路径。 [板级初始化](../kernel/src/board.rs) · [固定版本 HAL 的 QSPI 实现](https://docs.rs/crate/stm32h7xx-hal/0.16.0/source/src/xspi/mod.rs) · [ST AN4760 的间接模式](https://www.st.com/resource/en/application_note/DM00227538.pdf)

### 2.4.4 打开镜像时先确定结构，再校验 CRC

`Image::open` 读 24 字节头。魔数四个字节都是 `0xFF` 时返回 `NoImage`，对应被擦除的 Flash 的前四字节。四字节里只要有一个发生变化，就按普通格式检查，魔数错误或版本不为 `1` 返回 `InvalidHeader`。随后取长度、应用数、应用表偏移和保存的校验值。对头先设有限范围，可以避免无界遍历损坏的应用数或按任意长度扫描 NOR。

```rust
pub fn open(flash: &mut W25q64jv) -> Result<Image<'_>, ImageError> {
    let mut raw = [0u8; HEADER_SIZE as usize];
    flash
        .read_at(0, &mut raw)
        .map_err(|e| ImageError::Flash(e))?;

    if raw[..4] == [0xff; 4] {
        return Err(ImageError::NoImage);
    }
    if &raw[..4] != b"KAPP" || le_u32(&raw, 4) != 1 {
        return Err(ImageError::InvalidHeader);
    }

    let header = ImageHeader {
        total_len: le_u32(&raw, 8),
        app_count: le_u32(&raw, 12),
        app_table_offset: le_u32(&raw, 16),
        image_crc32: le_u32(&raw, 20),
    };

    if header.app_count > MAX_APPS
        || header.app_table_offset != HEADER_SIZE
        || !(HEADER_SIZE..=FLASH_SIZE).contains(&header.total_len)
    {
        return Err(ImageError::InvalidHeader);
    }

    let mut segment_cursor = header.app_table_offset + header.app_count * APP_SIZE;
    if segment_cursor >= header.total_len {
        return Err(ImageError::InvalidHeader);
    }

    let mut image = Image {
        flash,
        header,
        payload_start: 0,
    };

    for index in 0..image.header.app_count {
        let app = image.read_app(index)?;
        if !(1..=MAX_SEGMENTS as u32).contains(&app.segment_count)
            || app.segment_table_offset != segment_cursor
        {
            return Err(ImageError::InvalidApp(index));
        }
        segment_cursor += app.segment_count * SEGMENT_SIZE;
        if segment_cursor > image.header.total_len {
            return Err(ImageError::InvalidApp(index));
        }
    }
    image.payload_start = segment_cursor;
    image.verify_crc()?;

    Ok(image)
}

fn read_app(&mut self, index: u32) -> Result<AppRecord, ImageError> {
    if index >= self.app_count() {
        return Err(ImageError::InvalidApp(index));
    }

    let mut raw = [0; APP_SIZE as usize];
    let offset = self.header.app_table_offset + index * APP_SIZE;
    self.flash
        .read_at(offset, &mut raw)
        .map_err(ImageError::Flash)?;

    Ok(AppRecord {
        name: raw[..16].try_into().unwrap(),
        entry: le_u32(&raw, 16),
        segment_count: le_u32(&raw, 20),
        segment_table_offset: le_u32(&raw, 24),
    })
}
```

应用表从 `24` 开始，其末端是 `24 + app_count × 28`。由于 `app_count <= 16`，这一步最大是 `472`，不存在接近 `u32` 上限的运算。`open` 用 `segment_cursor` 检查连续排列的段表。第一个应用的段表必须紧接应用表，每读取一个应用记录，先校验段数在 `1..=16` 且段表偏移等于游标，再把游标向前推 `segment_count × 20` 字节。每次移动后都检查末端。即便取最大的 16 个应用且每个有 16 段，这一流程的最后游标也是 `24 + 16 × 28 + 16 × 16 × 20 = 5592`。这是这些游标可直接相加的理由。段记录里的 `dst_addr` 和 `payload_offset` 则来自外部字节的任意 `u32` 值，需要另一套溢出检查。

`read_app` 先验索引，再按固定跨度读取完整 `[u8; 28]`。`raw[..16].try_into().unwrap()` 的 `unwrap` 依赖固定切片长度这一事实，与文件里名称字节的值无关。字段解码函数 `le_u32` 同样仅读调用者提供的定长记录里固定存在的四字节。先读定长原始字节、再解释数值，可以避免把未校验的外部偏移当成一段可直接访问的内存。最后一个段表的末端记为 `payload_start`。`open` 这一步保证表与表的位置关系，具体的名称格式、段目标和入口留给 `validate_app`。

还有一个生产端和读取端的边界差异。打包器的命令行允许给出零个输入，生成长为 24 字节的空目录镜像。对于该产物，`segment_cursor` 与 `total_len` 都是 `24`，于是 `open` 的 `segment_cursor >= total_len` 成立，返回 `InvalidHeader`。因此按照本章的现有实现，至少打包一个用户应用才会有供内核打开的正常镜像。 [内核镜像打开流程](../kernel/src/batch/image.rs) · [打包参数](../tools/pack/src/cli.rs)

```rust
fn verify_crc(&mut self) -> Result<(), ImageError> {
    let mut crc = !0u32;
    let mut offset = 0;
    let mut buf = [0u8; 128];

    while offset < self.header.total_len {
        let len = (self.header.total_len - offset).min(buf.len() as u32) as usize;
        self.flash
            .read_at(offset, &mut buf[..len])
            .map_err(ImageError::Flash)?;

        for (i, &byte) in buf[..len].iter().enumerate() {
            // 打包器计算 CRC 时，这四字节仍为零。
            let byte = if (20..24).contains(&(offset + i as u32)) {
                0
            } else {
                byte
            };
            crc ^= u32::from(byte);
            for _ in 0..8 {
                let low_bit = crc & 1;
                crc >>= 1;
                if low_bit != 0 {
                    crc ^= 0xedb8_8320;
                }
            }
        }
        offset += len as u32;
    }

    if !crc != self.header.image_crc32 {
        return Err(ImageError::BadCrc);
    }
    Ok(())
}
```

读取方计算的 CRC 范围是 `[0, total_len)`。这包含镜像头、应用表、全部段表和所有负载。烧入后的绝对偏移 `[20, 24)` 存着校验值，计算却仍对它输入四个零，与主机写回校验值之前的缓冲完全一致。CRC 循环初始化为 `0xFFFF_FFFF`，每个字节先进入低位，随后按低位及反射多项式 `0xEDB8_8320` 做八次更新，最后按位取反。它和主机常用的 `zlib.crc32` 使用同一个结果约定。最后一次读取用实际剩余长度，所以 NOR 上紧随镜像的擦除值、预留区内容、完整读回文件的填充字节都不参加校验。

例如主机读出的文件长度是 `4096`，头给出的 `total_len` 是 `492`，要复算就先取前 492 字节，把偏移 `20` 至 `23` 改为零，再计算 CRC。若对全文件或只对 payload 计算，所得值对应另一段输入。无论主机一次读全文件，还是 MCU 每次读取最多 128 字节，CRC 状态都须沿整个字节顺序连续更新。每块重置状态等于给多份短消息分别计算校验值。这个实现只存一块缓冲区和一个 CRC 状态，因此镜像大小决定用时，却不要求相同大小的 RAM。在当前调用路径中，启动时全镜像校验一次，装载某个应用时又从 Flash 读取它的字节。

CRC 保护完整性检查的范围，包括记录中可能被意外改动的 `dst_addr` 和 `entry`。重算 CRC 后仍能造出格式正确的字节，所以执行前要验证每一段的地址，运行约定仍从可信应用出发。调试时可以按错误发生的顺序定位。魔数错误、应用段表不连续或目录越界会在 CRC 前返回，因此随机修改记录后遇到 `InvalidApp` 不一定能看到 `BadCrc`。若有意验证 CRC 的错误分支，可只在主机镜像副本的某个 payload 字节上翻转一位，并保留头和表，以确保先通过结构检查。 [CRC 实现与结果约定](../kernel/src/batch/image.rs) · [主机编码的 CRC](../tools/pack/src/image.rs)

### 2.4.5 把一条外部记录变成经过检查的 SRAM 写入范围

`validate_app` 每次只建立当前应用的段信息。它用 `heapless::Vec<SegmentRecord, 16>` 在固定容量内保存已验证的记录。名字先在首个零处截断，或者在没有零时取完整的 16 字节。前缀必须非空且各字节属于打包器同样允许的 ASCII 范围，后缀必须全部是零。这样随后调用 `ValidatedApp::name()` 时，从内容检查就可推出 UTF-8 解码成功。主机还会拒绝重复名称，固件此处按索引处理单项，固件的 `validate_app` 没有再比较其他应用的名称。

```rust
pub fn validate_app(&mut self, index: u32) -> Result<ValidatedApp, ImageError> {
    let app = self.read_app(index)?;
    let name_end = app.name.iter().position(|&b| b == 0).unwrap_or(16);
    if name_end == 0
        || !app.name[..name_end]
            .iter()
            .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
        || app.name[name_end..].iter().any(|&b| b != 0)
    {
        return Err(ImageError::InvalidApp(index));
    }

    let mut segments = Vec::<SegmentRecord, MAX_SEGMENTS>::new();
    for n in 0..app.segment_count {
        let mut raw = [0; SEGMENT_SIZE as usize];
        let offset = app.segment_table_offset + n * SEGMENT_SIZE;
        self.flash
            .read_at(offset, &mut raw)
            .map_err(ImageError::Flash)?;

        let segment = SegmentRecord {
            dst_addr: le_u32(&raw, 0),
            payload_offset: le_u32(&raw, 4),
            file_len: le_u32(&raw, 8),
            mem_len: le_u32(&raw, 12),
            flags: le_u32(&raw, 16),
        };

        let invalid = || ImageError::InvalidSegment(index, n);
        let dst_end = segment
            .dst_addr
            .checked_add(segment.mem_len)
            .ok_or_else(invalid)?;
        let src_end = segment
            .payload_offset
            .checked_add(segment.file_len)
            .ok_or_else(invalid)?;

        if segment.mem_len == 0
            || segment.file_len > segment.mem_len
            || segment.dst_addr < APP_START
            || dst_end > APP_END
            || segment.payload_offset < self.payload_start
            || src_end > self.header.total_len
            || {
                segments.iter().any(|prev| {
                    prev.dst_addr < dst_end && segment.dst_addr < prev.dst_addr + prev.mem_len
                })
            }
        {
            return Err(invalid());
        }

        segments.push(segment).map_err(|_| invalid())?;
    }

    let entry_addr = app.entry & !1;

    if app.entry & 1 == 0
        || !segments.iter().any(|segment| {
            segment.flags & PF_X != 0
                && segment.dst_addr <= entry_addr
                && entry_addr < segment.dst_addr + segment.file_len
        })
    {
        return Err(ImageError::InvalidEntry(index));
    }

    Ok(ValidatedApp {
        name: app.name,
        entry: app.entry,
        segments,
    })
}
```

每条段记录先解出五个整数，再分别计算源和目标的末端。目标范围是 `[dst_addr, dst_addr + mem_len)`，源范围是 `[payload_offset, payload_offset + file_len)`。两个末端都用 `checked_add`。源开始处要晚于或等于 `payload_start`，源末端不得越过 `total_len`，这堵住了以表中某个偏移冒充指令内容的路径。目标开始处要达到 `APP_START`，末端不得超过 `APP_END`。`mem_len` 必须大于零，`file_len` 必须小于或等于 `mem_len`，由此可安全计算后续补零长度，并保证 `file_len` 以内的任何目标地址都留在同一应用区间。

把数字代入更容易检查边界。若 `dst_addr = 0x2407_EFF0`，`mem_len = 0x10`，目标末端等于 `0x2407_F000`，可以接着验证其他字段。只增加一个字节便占用应用栈的第一个地址。假设此段 `file_len = 0x18` 而 `mem_len` 仍是 `0x10`，纵使降一点目标地址，文件长度与内存长度的约定也会被破坏。若 `payload_offset = 0xFFFF_FFF8`、`file_len = 0x10`，加法报溢出，判断不会使用回绕出的 `0x8`。零长源允许恰好从 `total_len` 表示一个空区间，但 `dst_addr` 仍要能容纳整个非空补零区。

段间重叠的判定采用 `prev.dst_addr < dst_end && segment.dst_addr < prev.dst_addr + prev.mem_len`。两个半开区间要相交，双方的起点都必须小于对方的末端。以 `[0x2406_0000, 0x2406_0040)` 和 `[0x2406_0040, 0x2406_0080)` 为例，第二个区间的起点等于第一个末端，严格小于条件不成立，两段可以相邻。如果第二段从 `0x2406_0030` 开始，重叠就会被拒绝。`mem_len` 参与判定，因而后一段的复制也不能覆盖前一段已经保留的补零尾部。这个检查针对同一个应用的目标区间，各应用先后使用同一 SRAM 时，目标范围相同是正常现象。

所有段检查完后，还需验证入口。最低位必须为 `1`，掩码后的指令地址必须位于一条带有 `PF_X` 的段的初始化部分。因此一个正处于应用区内、指向 `.data` 或某个补零尾部的入口仍会被拒绝。对于段记录中被表示为可执行的字节，内核只检查这里的地址关系。应用指令的正确性由构建和测试保证。`ValidatedApp` 收集的只是已通过当前约束的装载信息，足以让加载器计算受限的源和目标范围。启动这份代码仍需要约定中的执行权限、异常栈帧和可信程序。 [应用与段检查源码](../kernel/src/batch/image.rs)

### 2.4.6 分块复制并建立零初始化数据

```rust
use core::ptr;

use super::APP_STACK_TOP;
use super::image::{Image, ImageError};

pub struct LoadedApp {
    pub entry: u32,
    pub stack_top: u32,
}

pub unsafe fn load(image: &mut Image<'_>, index: u32) -> Result<LoadedApp, ImageError> {
    let app = image.validate_app(index)?;
    let mut buf = [0u8; 128];

    for segment in app.segments() {
        let dst = segment.dst_addr as *mut u8;
        let mut copied = 0;
        while copied < segment.file_len {
            let len = (segment.file_len - copied).min(buf.len() as u32) as usize;
            image.read_payload(segment.payload_offset + copied, &mut buf[..len])?;
            unsafe {
                ptr::copy_nonoverlapping(buf.as_ptr(), dst.add(copied as usize), len);
            }
            copied += len as u32;
        }
        let zero_len = segment.mem_len - segment.file_len;
        if zero_len != 0 {
            unsafe {
                ptr::write_bytes(dst.add(segment.file_len as usize), 0, zero_len as usize);
            }
        }
    }

    Ok(LoadedApp {
        entry: app.entry(),
        stack_top: APP_STACK_TOP,
    })
}
```

`load` 先运行完整的 `validate_app`，然后按原段表顺序写 SRAM。`copy_nonoverlapping` 的源指向内核栈上 128 字节缓冲区，目标指向应用装载区。每次 `read_payload` 只填本轮将使用的 `buf[..len]`，然后立即复制等长字节。不同地址域加的是同一个 `copied`，使 Flash 与 SRAM 的进度一致。使用 `u8` 指针时，`dst.add(copied as usize)` 的步长正好是字节。调用这个不安全函数依赖应用区在装载期间没有被正在执行的应用占用，写出的范围由已验证的段界定。目标段应当真实可写，临时缓冲不能与目标重叠，链接器预留的布局为这些条件提供依据。

仍以前面的 `file_len = 300`、`mem_len = 320` 为例。复制循环三次请求 128、128 和 44 字节。若段在镜像中的 `payload_offset` 为 `140`、目标为 `0x2406_0000`，三次读从镜像偏移 `140`、`268`、`396` 开始，向 SRAM 的 `0x2406_0000`、`0x2406_0080`、`0x2406_0100` 写入。驱动对前两次各发四个 32 字节事务，最后一次发一个 32 字节事务和一个 12 字节事务，共十个负载读取事务。第一层的 128 是加载器在内核栈上的缓冲大小，旨在让一个缓冲区承接较多连续字节。第二层的 32 是 HAL 每次读的上限，保证每个事务可以由当前接口完成。CRC 用的是自己的一份 128 字节缓冲区，也调用同一个驱动分拆。读取目录和校验阶段的事务应单独计算。

复制完后，`zero_len = 320 - 300 = 20`，`ptr::write_bytes(dst.add(300), 0, 20)` 正好把 `[0x2406_012C, 0x2406_0140)` 清零。`write_bytes` 以目标类型的元素计数，一个 `u8` 就对应一个字节。如果误把清零起点写成 `dst`，会覆盖刚装好的一部分指令或数据。如果从 `dst.add(mem_len)` 开始，会漏掉当前段的尾部，还可能踩到相邻段或应用栈。内存中的 `.bss` 可能来自上一应用曾写入的 SRAM，正确的初值必须由加载步骤重新建立，不能根据开发板上电后的当前字节状态推断。`file_len = 0` 时跳过复制，整个段随即清零。`file_len = mem_len` 时已有完整初始化内容，清零分支自然跳过。

当前实现逐段初始化，段之间未被描述的地址和整块 PSP 栈可能保留旧字节。异常启动会在栈顶写初始帧，随后应用使用栈区。若有研究需求在应用之间彻底清理执行区或栈区，应明确由谁操作它们、何时没有执行现场还占用这些地址。发生 Flash 读错误时，`?` 会把错误向上传给批循环，CPU 不会从部分装好的代码启动。此前已复制的字节仍在 SRAM，这个实现没有事务回滚。再次成功装载同一应用会重新初始化它声明的各段。 [装载器源码](../kernel/src/batch/loader.rs)

### 2.4.7 生命周期、运行顺序与上一个应用留下的指令

```rust
pub(crate) unsafe fn run(
    flash: &mut W25q64jv,
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
        console.wait_for_write();
        console.print_fmt(format_args!("app[{index}] exited: {code}\r\n"));
    }
    console.print_fmt(format_args!("Batch complete\r\n"));
    Ok(())
}
```

`batch::run` 在启动时打开镜像，随后按 `0..app_count` 的顺序遍历。它在 `loader::load` 成功后把入口和栈顶交给 `trap::run_app`。当应用通过支持的退出协议交回控制权，Rust 调用产生一个退出码，批循环先排空输出队列，再打印当前索引的结果并继续下一个应用。只要打开、验证或读段有错误，`?` 就退出批处理，`main` 负责报告 `KAPP error`。应用在执行中触发未处理的异常走的是另一条路径。`Batch complete` 只在循环顺利走完时出现，内核入口随后还执行用于演示的 `panic!("Done!")`。`batch::inspect` 可打印镜像条目并检查段。当前内核入口调用的是 `run`，所以普通启动日志不会自动列出整个目录。 [批处理模块](../kernel/src/batch.rs) · [内核入口](../kernel/src/main.rs)

rCore 第二章的应用构建链可以去掉 ELF 外层表示，把裸二进制经汇编 `.incbin` 纳入内核镜像。内核读取 `_num_app` 和应用边界表，求相邻两地址之差获得长度，再装入固定运行区域。`krunix` 也采用在构建脚本里生成 `link_app.S`、把镜像嵌入 `.rodata.apps` 的做法，现有 `load_app` 使用 `0x8040_0000` 和 128 KiB 的固定区，先清整区，再复制当前应用。这里应用目录在片外 NOR，长度直接存在 KAPP 段记录中，每段可有自己的目标地址，装载只清本应用中声明需要补零的尾部。三个系统都依靠同一个批处理条件。各个应用按共同的内存契约构建，由内核确定装入的内容，并安排各应用依次运行。

还有一条容易从 RISC-V 类比过来的硬件条件。`krunix` 的 `fence.i` 用于让接下来的取指看到代码复制后的更新。当前 Cortex-M7 的加载函数没有做 I-Cache 和 D-Cache 维护，板级初始化也没有主动开启它们。启动汇编在 SVC 前执行 `dsb`，并在改 `CONTROL` 后执行 `isb`。以后如果开启缓存，需要核对这段 SRAM 的缓存属性，依据 Arm 与 ST 的缓存规则让新代码更新对取指可见。典型做法涉及清理新代码覆盖的 D-Cache 范围、同步并失效对应的指令缓存，再执行必要的屏障。否则复用地址时，CPU 可能从旧 I-Cache 行取到上一应用的指令，或者有新的字节仍留在 D-Cache 中。只看主机镜像的 CRC 无法说明 CPU 取指已经看到新内容。 [rCore 的批处理装载](https://rcore-os.cn/rCore-Tutorial-Book-v3/chapter2/3batch-system.html) · [项目缓存处理说明](batch-implementation.md) · [ST AN4839 缓存维护](https://www.st.com/resource/en/application_note/an4839-level-1-cache-on-stm32f7-series-and-stm32h7-series-microcontrollers-stmicroelectronics.pdf)

### 2.4.8 在主机上分段核对构建产物

在仓库根目录可先构建应用，在写入板卡之前检查 ELF 头、程序头、镜像头和字节位置。以下命令以默认 debug 产物为例。

```bash
make pack-apps
readelf -W -h target/thumbv7em-none-eabihf/debug/00hello
readelf -W -l target/thumbv7em-none-eabihf/debug/00hello
xxd -g 1 -l 24 output/apps.img
```

`readelf` 的 `LOAD` 行给出 `Offset`、`VirtAddr`、`FileSiz` 和 `MemSiz`，在镜像里依次找到对应的 `payload_offset`、`dst_addr`、`file_len` 和 `mem_len`。第一个值要由 ELF 里的原位置换算为 KAPP 编码器重新安排的位置，其余三个值应与 ELF 匹配。一份 `00hello` 的调试构建确实可能只有一条 `LOAD`，它的 `FileSiz` 与 `MemSiz` 相同，表示该次构建没有产生要补零的段尾。若要实测补零的目录表示，可以另行编译一个确实读取或写入零初值静态区、并让该变量保留在最终 ELF 中的应用，检查 `readelf` 是否显示某段 `MemSiz > FileSiz`。构建选项会改变具体长度和输出中保留的 section，判断应直接读取本轮文件。

下面的主机检查程序使用 Python 标准库，核对头并打印目录。它读取镜像中声明的范围后复算 CRC。对已经由设备读回、末尾含额外字节的文件也可用同样方法。`struct` 格式的 `<` 固定用小端顺序，`I` 读取 4 字节无符号整数。可以把路径换成本轮产物，在该输出里检查两个应用的索引顺序以及负载、目标区间的端点。

```bash
python3 - output/apps.img <<'PY'
import struct
import sys
import zlib
from pathlib import Path

raw = Path(sys.argv[1]).read_bytes()
magic, version, total_len, app_count, app_table, saved_crc = struct.unpack_from('<4s5I', raw)
assert (magic, version, app_table) == (b'KAPP', 1, 24)
assert 24 <= total_len <= len(raw)
checked = bytearray(raw[:total_len])
checked[20:24] = b'\0' * 4
assert zlib.crc32(checked) == saved_crc
print(f'{app_count} app(s), {total_len} bytes, CRC32={saved_crc:#010x}')
for index in range(app_count):
    name, entry, count, table = struct.unpack_from('<16sIII', raw, app_table + 28 * index)
    name = name.rstrip(b'\0').decode('ascii')
    print(f'[{index}] {name} entry={entry:#010x}')
    for number in range(count):
        dst, src, filesz, memsz, flags = struct.unpack_from('<5I', raw, table + 20 * number)
        print(f'  dst=[{dst:#010x}, {dst + memsz:#010x}) '
              f'src=[{src:#x}, {src + filesz:#x}) zero={memsz - filesz} flags={flags:#x}')
PY
```

如果输出中的 Thumb 入口与 ELF 一致、段源落在镜像范围内、段目标落在应用区，并且 CRC 匹配，主机端能确认这份文件符合这些检查。板上还需确认写入的 NOR 区域确实是内核读到的区域，以及 SVC 能完成加载后的交接。装载结束后，启动代码从 `LoadedApp` 得到应用入口与 PSP 初始栈顶。接下来要看 Cortex-M7 如何利用它们建立第一次异常返回。

## 2.5 实现特权级切换与系统调用

加载器把应用放进 AXI SRAM 后，内核手里有两个数，应用入口 `entry` 和应用栈顶 `stack_top`。如果把入口转成函数指针再调用，CPU 会沿用当前的 Thread 特权级和选中的栈。应用便会在特权态使用内核的 MSP，普通函数调用还约定它最后用 `lr` 回到调用者。这和我们希望的运行方式不同。内核希望保留自己的调用链，让应用在非特权 Thread mode 使用 PSP，最后根据应用请求决定是恢复应用，还是结束应用并继续批处理。因此入口地址要交给异常返回机制，退出也要经过特权入口。 [内核批处理](../kernel/src/batch.rs) · [切换入口](../kernel/src/trap.rs)

### 2.5.1 从一次应用启动跟到批处理继续

把一次运行拆成三条控制流。第一条是内核 Thread，它执行 `loader::load()` 和 `trap::run_app()`，等待最终退出码。第二条是特权 Handler，每次执行 SVC 后由硬件启动，负责选择下一条控制流。第三条是应用 Thread，它从 `_start()` 执行到 `main()`，也可以主动调用运行时提交服务请求。内核 Thread 和应用 Thread 都属于 Thread mode，区别在权限、栈和各自的寄存器现场。Handler 是第三处执行环境，始终在特权态并使用 MSP。特权可以随异常入口和返回发生变化，内核 Thread 的 Rust 调用并没有因此结束。 [Arm Cortex-M7 程序员模型与异常模型](https://documentation-service.arm.com/static/61efd8352dd99944d05142d1?token=)

| 时刻 | 正在执行 | 现场放在哪里 | 接下来执行 |
| --- | --- | --- | --- |
| `run_app()` 准备首次进入 | 特权 Thread，MSP | Rust 在应用栈顶下方写初始帧 | `__kr_run_app()` 执行 `svc #0` |
| 第一次进入 `SVCall` | 特权 Handler，MSP | 硬件帧和汇编另存的现场留在 MSP | 用初始帧进入非特权 Thread |
| 应用 `_start()` 与 `main()` | 非特权 Thread，PSP | 普通函数栈在 PSP，休眠的内核现场仍在 MSP | 应用调用 `svc #1` |
| 仅退出实验的第二次 `SVCall` | 特权 Handler，MSP | 硬件把应用现场写到 PSP，MSP 指向内核软件保存区 | 按指令分派。退出把退出码写入内核硬件帧 |
| 内核恢复 | 特权 Thread，MSP | 硬件恢复内核基本帧，汇编函数向 Rust 返回 | 批处理打印退出码并继续迭代 |

这里出现三份基本异常帧。**应用的初始帧**由 Rust 在应用栈上预先写入，用来完成第一次异常返回。**内核启动时的硬件帧**由 `svc #0` 触发，位于 MSP，里面有内核暂停的位置。**应用退出时的硬件帧**由 `svc #1` 触发，位于 PSP，里面有应用退出码和触发服务的位置。第一次进入应用时，初始帧已经被弹出，应用随后可以复用那段内存。后来的 PSP 硬件帧按当时的 PSP 重新分配，其起点由应用累计使用的栈空间决定。另外还有一块 MSP 软件保存区，它紧挨内核硬件帧并保存硬件没有代劳的寄存器。区分它们的办法是同时记下写入者、生成时刻和最终由谁消费。 [Rust 初始帧](../kernel/src/trap.rs) · [异常汇编](../kernel/src/trap/svc.s)

### 2.5.2 `svc #x`、参数寄存器与硬件压栈

`svc #x` 是 Thumb 的一条 16 位指令，`x` 是编译时编码进指令的无符号八位立即数。指令半字的高八位固定为 `0xdf`，低八位保存 `x`，因此 `svc #0`、`svc #1`、`svc #2` 的半字分别是 `0xdf00`、`0xdf01`、`0xdf02`。遇到 `svc #1` 时，处理器请求的是异常号 11 的 SVCall，执行 `svc #2` 请求的也是同一个异常。立即数用于软件选择服务，和异常号属于两套编号。硬件也没有把立即数自动放进 `r0` 或另一个通用寄存器。内核可先拿到栈里的返回 PC，再回看前两字节，自己从指令中读出立即数。 [Arm Cortex-M7 通用指南的异常类型和 SVC 指令](https://documentation-service.arm.com/static/61efd8352dd99944d05142d1?token=)

应用请求服务时，除了服务号还可能需要本次调用的数据。比如内核每次输出的字节串都可能不同。如果把地址或长度编码进指令，即使数值装得下，运行时还要为不同参数更改机器码。把服务号放立即数、数据放寄存器，就能为同一个服务生成固定入口，同时传递动态参数。当前 ABI 用 `r0` 放第一个参数、`r1` 放第二个参数。`exit(code)` 的 `r0` 是退出状态，`r1` 由用户封装设为零。`write(bytes)` 的 `r0` 是字节缓冲区首地址，`r1` 是长度。普通服务返回时，用户封装也从 `r0` 取得结果。这样安排借用了 Arm 常见的整数参数和返回值寄存器，SVC 本身负责异常入口，具体的参数约定仍由两侧代码共同定义。硬件可以把 `r2` 一并压栈，当前用户封装只传两个参数，内核汇编把自己的活 `r2` 当计算 PC 和读取指令半字的临时寄存器。将来若协议扩展到第三个参数，两侧须共同约定读取帧的 `r2` 槽。 [用户 ABI](../user/src/runtime/syscall.rs) · [AAPCS32](https://github.com/ARM-software/abi-aa/blob/main/aapcs32/aapcs32.rst)

处理器进入 SVC 时，先在被暂停一方正在使用的栈上保存基本帧，然后开始用 MSP 执行 Handler。应用在 PSP 上运行，所以应用的帧在 PSP，Handler 里的当前 `sp` 此时却是 MSP。在 Cortex-M7 的基本帧中，从栈顶的最低地址往高地址看，布局如下。 [Arm Cortex-M7 异常入口和栈帧格式](https://documentation-service.arm.com/static/61efd8352dd99944d05142d1?token=)

| 帧内字节偏移 | 硬件保存的状态 | 本实现用法 |
| --- | --- | --- |
| `0x00` | `r0` | 读初始帧指针或应用退出码，也可写入返回值 |
| `0x04` | `r1` | 记录第二个用户参数，写服务实现可从这里读长度 |
| `0x08` | `r2` | 记录异常前的 `r2`，Handlers 可独立使用活 `r2` |
| `0x0c` | `r3` | 基本帧保存，被暂停一侧恢复时由硬件恢复 |
| `0x10` | `r12` | 基本帧保存，允许处理代码使用此临时寄存器 |
| `0x14` | `lr` | 保存异常前的普通 LR，退出 Handler 后可恢复函数返回链 |
| `0x18` | `pc` | 对 SVC 是下一条指令地址，首次启动则填入口地址 |
| `0x1c` | `xPSR` | 保存状态，包括 Thumb 的 `T` 位和硬件使用的对齐信息 |

表中八个字共有 32 字节。如果入栈前 SP 只有四字节对齐，Cortex-M7 的异常入栈还可以在帧的高地址一侧带一个对齐填充字，使帧起点满足八字节对齐。堆栈里的 `xPSR` 位 9 告知硬件返回时如何恢复原 SP。上述寄存器相对帧起点的偏移保持固定，调试时仍可用 `+0x18` 读 PC，但计算入栈前的 SP 时要考虑对齐字。构造应用初始帧时栈顶原本八字节对齐，`xpsr: 1 << 24` 的位 9 为零，因此消耗量就是八个字。手算内核的 SVC 时也可以使用调用边界的八字节对齐前提，应用处于任意单条指令边界时则应查看实际保存的对齐位。 [Arm Cortex-M7 异常栈对齐](https://documentation-service.arm.com/static/61efd8352dd99944d05142d1?token=)

异常一进门就有两种 LR 要分别追踪。压栈的 `lr` 记录异常前的普通寄存器值，例如内核调用 `__kr_run_app()` 时由 `bl` 得到的函数返回地址。进入 Handler 后，处理器在活 `lr` 放入 `EXC_RETURN`。这个特殊值告诉处理器应回到哪种模式、从哪条栈恢复、恢复哪类帧。用 `bx lr` 触发异常返回时，硬件按 `EXC_RETURN` 找帧，最后用帧里那个普通 LR 覆盖活 LR。若把两个 LR 当成同一个值，容易误以为内核把代码放在 `0xffff_fff9`，或者误把应用栈的 LR 当成回到内核的指令地址。前者在 Handler 中是硬件识别的返回标记，真正要执行的地址由选中的帧提供。 [Arm Cortex-M7 异常返回与 EXC_RETURN](https://documentation-service.arm.com/static/61efd8352dd99944d05142d1?token=)

| 活 LR 的值 | 所描述的基本帧 | 使用该值异常返回的效果 |
| --- | --- | --- |
| `0xffff_fff1` | Handler mode 的 MSP | 回到先前的 Handler，当前实现拒绝这个来源 |
| `0xffff_fff9` | Thread mode 的 MSP | 从 MSP 恢复，回到使用 MSP 的 Thread |
| `0xffff_fffd` | Thread mode 的 PSP | 从 PSP 恢复，回到使用 PSP 的 Thread |

这三组里位 4 为 1，对应基本帧。位 3 表达返回 Handler 或 Thread，位 2 区分在 Thread 返回时选择 MSP 还是 PSP。返回的 Thread 是否为非特权态则由 `CONTROL.nPRIV` 决定。看到 `0xffff_fffd` 可以判断帧来自 PSP 和 Thread，单凭它推导不出中断前的 Thread 权限。项目设定的应用上下文保证它同时是非特权态。反过来，应用使用 PSP 并不能替代 MPU 的地址权限。 [Arm Cortex-M7 CONTROL 与 EXC_RETURN 定义](https://documentation-service.arm.com/static/61efd8352dd99944d05142d1?token=)

### 2.5.3 Rust 先构造可被硬件恢复的现场

[内核的 `trap.rs`](../kernel/src/trap.rs) 把基本帧写成有固定布局的 Rust 结构，然后借用 C ABI 把指针交给汇编。完整代码如下。

```rust
use core::{arch::global_asm, slice};

use crate::{
    batch::{APP_STACK_TOP, APP_START},
    drivers::console::{self, MAX_MESSAGE_BYTES, WriteError},
};

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

unsafe extern "C" {
    fn __kr_run_app(frame: *mut BasicExceptionFrame) -> i32;
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
        __kr_run_app(frame)
    }
}

#[unsafe(no_mangle)]
extern "C" fn __kr_sys_write(addr: u32, len: u32) -> i32 {
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

global_asm!(include_str!("trap/svc.s"), options(raw));
```

`#[repr(C)]` 给这个结构提供可与底层汇编约定的字段顺序。八个字段都是 `u32`，在 Cortex-M7 上按四字节排列，`pc` 是第七个字，偏移是 `6 × 4 = 24`，这正是后面 `[sp, #24]` 或 `[r0, #24]` 的由来。`size_of::<BasicExceptionFrame>()` 用结构本身表达要预留的大小，避免 Rust 建帧和汇编读帧使用不同的数量。与此同时，汇编中的偏移仍是固定常数，因此新增字段或调整顺序都需要重新核对硬件和汇编契约。`frame.write` 把结构值写到原始指针指向的应用栈，适合首次初始化这块存储。调用方需要保证整块内存可写、对齐、位于已经划定的栈内，且此时没有活跃应用占用它。两个 `debug_assert` 分别检查低三位和低一位，帮助开发构建发现不符合契约的栈顶和入口。正式镜像的入口与区域验证由加载链路完成，`debug_assert` 可以在优化构建中去掉。 [镜像验证](../kernel/src/batch/image.rs) · [加载器](../kernel/src/batch/loader.rs)

假定镜像入口字段是 `0x2406_0001`，这个低位 1 是 Thumb 函数入口的标记。异常帧把它拆成两项，指令本身位于 `entry & !1 = 0x2406_0000`，`xPSR` 的第 24 位 `T` 设为 1。否则异常返回后的执行状态不符合 Cortex-M7 的 Thumb 要求。当前应用栈顶为 `0x2408_0000`，向低地址让出 `0x20` 字节，初始帧起点是 `0x2407_ffe0`。当 PSP 指向这份帧，硬件弹出全部八个字后，应用开始执行时的栈顶恰好回到 `0x2408_0000`，便可在它下方正常建立 `_start()` 的普通函数栈。初始 `lr = 0` 的含义是这条控制流没有可返回的普通调用者。`_start()` 的签名是 `-> !`，它调用 `main()` 时由正常函数调用机制设置返回地址，取得 `main()` 的结果后调用 `exit()`。因此零 LR 与 `main()` 能返回到 `_start()` 并不冲突。 [应用链接脚本](../user/link.x) · [用户态入口](../user/src/runtime.rs)

帧里的 `r0` 到 `r3`、`r12` 设零，使首次启动的参数和暂存寄存器具有确定初值。此处把初始帧地址作为 `__kr_run_app` 的普通函数参数，C ABI 将它放入活 `r0`。两者处于不同时间和不同位置，`[0x2407_ffe0]` 存的是将来应用的初始 `r0 = 0`，此刻内核活 `r0 = 0x2407_ffe0`。等内核发出 `svc #0` 后，后一数值会被硬件存入内核帧的 `r0` 槽。这个区分也解释了为什么启动分支先读 `[sp]`，而后 `msr PSP, r0` 就能指向应用现场。 [Arm 过程调用约定](https://github.com/ARM-software/abi-aa/blob/main/aapcs32/aapcs32.rst)

### 2.5.4 将汇编入口接到异常向量

`global_asm!` 把独立的 [`svc.s`](../kernel/src/trap/svc.s) 并入内核目标代码。`include_str!("trap/svc.s")` 以 `trap.rs` 所在目录为基准读文件，`options(raw)` 保留汇编寄存器列表中的 `{r3-r11, lr}`，使花括号无需作为 Rust 汇编模板的占位符重新转义。汇编本身用 `.syntax unified` 和 `.thumb` 确定指令语法及执行集，用 `.section .text.batch_trap,"ax",%progbits` 把代码放在可分配、可执行的输入节。内核的 `build.rs` 为内核二进制传入 `-Tlink.x`，`cortex-m-rt` 的链接脚本把 `.text.*` 收进最终代码区，并将异常表保留在 Flash 的 `.vector_table`。公共函数符号由 `.global` 导出，`.type` 和 `.size` 让 ELF 记录它是函数及其长度，`.thumb_func` 提供 Thumb 入口标记。`.p2align 2` 的齐边界是 `2^2 = 4` 字节，该指令只影响布局，帧的八字节对齐来自栈的规则。 [内核构建脚本](../kernel/build.rs) · [内核存储布局](../kernel/memory.x) · [cortex-m-rt 源码](https://docs.rs/crate/cortex-m-rt/0.7.7/source/link.x.in)

`SVCall` 是 `cortex-m-rt` 向量表的固定符号。链接脚本以 `PROVIDE(SVCall = DefaultHandler)` 提供缺省入口，我们的汇编定义了同名函数，最终由第 11 号向量引用它。内核的普通 Rust 调用只引用 `__kr_run_app`，Handler 是通过硬件向量访问的。若修改汇编标签后只改了 Rust 声明，链接器仍可能保留缺省的异常入口，而在 `svc` 执行时跑到 `DefaultHandler`；调试这类问题时同时检查 ELF 的函数符号和向量表里的地址。内核 Flash 起点 `0x0800_0000` 的链接映像中，向量表下标 11 位于 `+11 × 4 = +0x2c`。一次现有调试 ELF 的反汇编显示这里的四个字节是 `d1 37 00 08`，按小端读成 `0x0800_37d1`，它指向实际指令地址 `0x0800_37d0` 的 `SVCall`。最低位承担 Thumb 标记。链接地址会随构建变化，固定的是向量编号、节布局规则以及符号之间的关系。运行时硬件从当前向量表基址取项，所以离开板上默认启动路径时还要核对启动代码选用的向量表。 [cortex-m-rt 向量表声明](https://docs.rs/crate/cortex-m-rt/0.7.7/source/src/lib.rs) · [Arm Cortex-M7 异常向量](https://documentation-service.arm.com/static/61efd8352dd99944d05142d1?token=)

```asm
.syntax unified
.thumb
.section .text.batch_trap,"ax",%progbits

.p2align 2
.global __kr_run_app
.type __kr_run_app,%function
.thumb_func
__kr_run_app:
    dsb
    svc     #0
.Lresume_kernel:
    bx      lr
.size __kr_run_app, . - __kr_run_app

.p2align 2
.global SVCall
.type SVCall,%function
.thumb_func
SVCall:
    mvn.w   r1, #6
    cmp     lr, r1
    beq     .Lstart_app

    mvn.w   r1, #2
    cmp     lr, r1
    beq     .Luser_svc
    b       .Lunsupported_svc

.Lstart_app:
    ldr     r2, [sp, #24]
    adr.w   r1, .Lresume_kernel
    cmp     r2, r1
    bne     .Lunsupported_svc

    ldr     r0, [sp]
    stmdb   sp!, {r3-r11, lr}
    msr     PSP, r0

    movs    r1, #0
    mov     r4, r1
    mov     r5, r1
    mov     r6, r1
    mov     r7, r1
    mov     r8, r1
    mov     r9, r1
    mov     r10, r1
    mov     r11, r1

    movs    r1, #1
    msr     CONTROL, r1
    isb
    mvn.w   lr, #2
    bx      lr

.Luser_svc:
    mrs     r0, PSP
    ldr     r2, [r0, #24]
    subs    r2, #2
    ldrh    r2, [r2]
    movw    r1, #0xdf01
    cmp     r2, r1
    beq     .Lexit_app
    movw    r1, #0xdf02
    cmp     r2, r1
    beq     .Lwrite_app
    b       .Lunsupported_svc

.Lexit_app:
    ldr     r0, [r0]
    ldmia   sp!, {r3-r11, lr}
    str     r0, [sp]

    movs    r1, #0
    msr     CONTROL, r1
    isb
    bx      lr

.Lwrite_app:
    push    {r4, lr}
    mov     r4, r0
    ldr     r0, [r4]
    ldr     r1, [r4, #4]
    bl      __kr_sys_write
    str     r0, [r4]
    ldmia   sp!, {r4, lr}
    bx      lr

.Lunsupported_svc:
    udf     #0
.size SVCall, . - SVCall
```

入口用原始汇编提供 Handler 的原因与执行顺序有关。`SVCall` 一进门，`sp` 与 `lr` 都有硬件定义的特殊用途。普通 Rust 函数可能为建立函数栈而修改 `sp`，还可能在调用其他函数时覆盖 `lr`。如果先让编译器生成函数序言，再照抄 `[sp, #24]`，读到的就可能是普通函数的局部数据。汇编先按约定保存现场并确定帧基址，再根据需要引入 Rust 分发函数，才有一条可核对的 ABI 边界。启动和退出由汇编直接完成。写入分支保存现场后用 C ABI 调用 Rust。 [异常汇编](../kernel/src/trap/svc.s)

### 2.5.5 从内核 SVC 返回到应用 `_start()`

`__kr_run_app` 先执行 `dsb`，让此前写入应用段和初始化帧的内存访问在后续继续前达到屏障所要求的完成状态。它是数据同步屏障。若将来打开相关缓存并改变应用装载策略，仍须另外满足 Cortex-M7 对写后执行的缓存维护要求，保证新写入的指令能被取指一侧看到。`svc #0` 随后引发异常，硬件在 MSP 上保存内核的 `r0` 到 `r3`、`r12`、普通 LR、返回 PC 和 `xPSR`。返回 PC 已是紧随 SVC 的 `.Lresume_kernel`，因此可用它辨认启动调用点。对于支持可执行缓存的配置，判断 `dsb` 和后续 `isb` 各自保障什么，有助于把缓存可见性与特殊寄存器配置更新的问题分开分析。 [Arm Cortex-M7 屏障与缓存维护](https://documentation-service.arm.com/static/61efd8352dd99944d05142d1?token=)

`SVCall` 先用 `mvn.w r1, #6` 计算按位取反 `~0x0000_0006 = 0xffff_fff9`，然后将活 LR 与它相比较。若匹配，说明从使用 MSP 的 Thread 进入且使用基本帧，于是 `[sp, #24]` 可以解释成内核返回 PC。接下来 `adr.w r1, .Lresume_kernel` 按 PC 相对寻址取得这个本地标签的指令地址，`cmp` 和 `bne` 要求两者相等。这个分支因此只接受内核固定位置触发的启动请求。若内核在另外一个位置写 `svc #0`，即使立即数相同，压栈 PC 也和标签不同。应用用 PSP 执行 `svc #0` 时，来源比较也不会进入启动分支。项目把启动定义为一次特定的控制流交接，立即数只是桩中用来触发异常的编码。 [异常入口与分支实现](../kernel/src/trap/svc.s)

随后 `ldr r0, [sp]` 从内核帧读回应用初始帧指针。`stmdb sp!, {r3-r11, lr}` 以递减地址分配十个字并更新 MSP。这个保存区是内核跨应用运行必须保留的第二部分：硬件已经保留 `r0` 到 `r3` 和 `r12`，而 `r4` 到 `r11` 可能承载内核编译器分配的跨调用变量，应用也会使用同一组物理寄存器。`lr` 在这里专指内核的 `EXC_RETURN`，是以后让硬件选回 MSP 基本帧的凭据。`r3` 增加一个已由硬件保存的槽，把软件区凑成 `10 × 4 = 40` 字节，保持 MSP 的八字节对齐。多个寄存器按编号从低地址依次排列，所以新 `sp` 对应 `r3`，最后的 `sp + 36` 对应 LR。这块保存区压上之后，MSP 顶端已从硬件帧地址移动，后续若需要读内核帧，必须先加软件保存区大小或先弹出它。 [Cortex-M7 多寄存器传送与异常帧](https://documentation-service.arm.com/static/61efd8352dd99944d05142d1?token=)

`msr PSP, r0` 把 PSP 设置成应用初始帧起点。Handler 仍用 MSP，所以设置 PSP 不会移走刚保存的内核上下文。接下来先让 `r1 = 0`，再用一组 `mov` 把活 `r4` 到 `r11` 清零。保存后再清零的顺序很重要。第一次启动时硬件只恢复基本帧的八项，活的 `r4` 到 `r11` 会原样带入应用。如果让它们沿用内核值，用户入口的状态会随内核编译结果和调用点变化，还可能携带内核残留值。保存区确保内核日后能找回自己的版本。应用后续遇到正常系统调用时，若 Handler 保持这组寄存器，应用原值便随异常返回继续使用。 [启动分支](../kernel/src/trap/svc.s)

接着 `msr CONTROL, r1` 写入数值 `1`，设置 `nPRIV = 1`，使将要恢复的 Thread 以非特权运行。在 Handler mode 里，`CONTROL.SPSEL` 的写入不起作用，实际返回哪条栈由接下来的 `EXC_RETURN` 决定。`isb` 使后续指令在同步后的执行上下文中运行。`mvn.w lr, #2` 生成 `0xffff_fffd`，`bx lr` 触发异常返回。硬件从 PSP 取出应用起始现场，自动选择 PSP 作为 Thread 栈，恢复 PC 后从 `_start()` 取指，此时 Thread 所见 `CONTROL` 低两位是 `0b11`。MSP 的两块内核保存区保持原位，下次异常进入 Handler 仍然会使用 MSP。启动路径从哪条栈取帧与 Thread 的权限由不同状态决定，因此不能只用一次对 `CONTROL` 的写入来代替这个异常返回。 [Arm Cortex-M7 CONTROL、ISB 与异常返回](https://documentation-service.arm.com/static/61efd8352dd99944d05142d1?token=)

### 2.5.6 应用怎样发送退出码，内核怎样接收

用户侧把服务号和参数位置集中在 [`syscall.rs`](../user/src/runtime/syscall.rs) 中。这样 `main()`、打印宏和运行时处理的是整数与切片，只有最底层包装需要知道寄存器名和 SVC 编码。当前完整文件如下。

```rust
use core::arch::asm;

const SYS_EXIT: u8 = 1;
const SYS_WRITE: u8 = 2;

fn syscall<const NUMBER: u8>(arg0: usize, arg1: usize) -> i32 {
    let result: i32;
    unsafe {
        asm!(
            "svc {number}",
            number = const NUMBER,
            inlateout("r0") arg0 => result,
            in("r1") arg1,
        );
    }

    result
}

pub(super) fn sys_write(bytes: &[u8]) -> i32 {
    syscall::<SYS_WRITE>(bytes.as_ptr() as usize, bytes.len())
}

pub(super) fn sys_exit(code: i32) -> i32 {
    syscall::<SYS_EXIT>(code as usize, 0)
}
```

`const NUMBER: u8` 是编译时泛型参数，因此 `number = const NUMBER` 可以作为 SVC 的立即数。一个运行时函数形参无法替代它，因为汇编器生成指令时就需要决定低八位。`inlateout("r0") arg0 => result` 描述同一寄存器的两个阶段，进指令前由编译器把 `arg0` 放进 `r0`，若控制流返回，再把 `r0` 作为 `result`。`in("r1") arg1` 告诉编译器第二个值也必须在指定寄存器里。在应用线程中，`r1` 的原输入可以由异常返回恢复。Handler 中允许暂存的活 `r1` 与应用恢复后的 `r1` 由不同阶段的帧数据连接，内联汇编约束看到的是跨整次调用的结果。当前 `exit` 在成功路径上终止应用，`syscall` 的通用返回形状仍可供将来的普通服务使用。 [Rust 内联汇编规则](https://doc.rust-lang.org/reference/inline-assembly.html) · [用户系统调用实现](../user/src/runtime/syscall.rs)

`_start()` 调用应用 `main()`，将返回的 `i32` 传给 `exit()`。`sys_exit(code)` 选择服务号 1，把 `code as usize` 放在 `r0`。Cortex-M7 目标上 `usize` 和 `i32` 都占 32 位，所以负数转换后在寄存器中保留其二进制补码位型。内核把这 32 位转交回声明返回 `i32` 的 `__kr_run_app()` 边界，Rust 就按有符号数解释它。比如 `-1` 的位型是 `0xffff_ffff`，内核打印的依旧是 `-1`。运行时 `exit(code) -> !` 在发出 SVC 之后还留了自旋兜底。如果某个有误的 Handler 恢复了应用，它也不会顺着一个不存在的调用者继续，更不会再由 panic handler 递归发出退出请求。 [用户运行时](../user/src/runtime.rs)

应用执行 `svc #1` 时，硬件在 PSP 上写入应用基本帧。`SVCall` 看到活 `lr = 0xffff_fffd` 后尝试处理应用请求。进入 `.Luser_svc` 先用 `mrs r0, PSP` 把应用帧基址读到活 `r0`，然后 `ldr r2, [r0, #24]` 取出压栈 PC。在硬件压栈结束后，原来装退出码的活 `r0` 已可以用来存帧指针，退出码仍安稳地躺在帧内第一字。`subs r2, #2` 回到实际执行的 `svc`，`ldrh r2, [r2]` 按 16 位取指令编码，`movw r1, #0xdf01` 装入退出服务半字并作比较。用 `ldrh` 可以只读取这条 SVC，避免将后面指令的两个字节混入结果。从 PSP 取异常前参数的同时用 MSP 执行内核汇编，是这里最容易混淆的地址关系。 [异常退出分支](../kernel/src/trap/svc.s)

识别服务后，`ldr r0, [r0]` 再把应用帧首字读成退出码。这里同一个寄存器充当加载地址和加载结果是合法的，硬件先用旧 `r0` 形成地址，再将读到的值写入 `r0`。`ldmia sp!, {r3-r11, lr}` 恢复启动时留下的内核软件区并将 MSP 增加 40 字节。它特意没有恢复 `r0`，因此退出码跨这条指令保存下来。此刻 `[sp]` 已是内核硬件帧的 `r0` 槽，`str r0, [sp]` 将原先的应用帧指针替换成退出码。直接保留活 `r0` 无法起到同样的作用，因为后面的异常返回会按 MSP 基本帧恢复 `r0`。改动帧里的槽位，产生的才是内核 Thread 恢复后可见的整数返回值。 [内核保存与恢复汇编](../kernel/src/trap/svc.s)

恢复了软件区的活 `lr = 0xffff_fff9` 后，汇编把 `CONTROL.nPRIV` 清零、执行 `isb`，然后从 Handler 的 `bx lr` 走异常返回。硬件弹出 MSP 基本帧，恢复前面被暂停的内核 Thread，PC 回到 `.Lresume_kernel`。现在已回到 Thread，活的 LR 也被帧里原本的普通函数返回地址替换。`.Lresume_kernel` 又执行一条 `bx lr`，这次是普通函数返回，从 `__kr_run_app()` 回到 Rust。两条 `bx lr` 使用同一条指令编码，行为由所在模式和 LR 的值共同决定。最终 `batch::run()` 得到的 `code` 经由 `trap::run_app()` 原样传出，再打印本轮的退出状态。 [内核返回汇编](../kernel/src/trap/svc.s) · [批处理循环](../kernel/src/batch.rs)


`Image::open()` 在循环外，因此镜像校验和目录读取不会在每次应用退出时重新开始。相同的 AXI SRAM 区用于每一轮应用，代码用 Rust 的同步返回来划分占用时段。`loader::load()` 已完成本轮装载才能调用 `run_app()`，只有它恢复内核上下文并返回，下一轮才会启动 `loader::load()` 覆盖执行区。镜像或装载失败通过 `Result` 返回。应用正常发出 `exit(code)` 时，负数和正数都按退出状态打印，代码没有把非零值当作镜像错误。CPU Fault 属于另一条异常处理路径，也不会自动变成这个 `Result` 里的 `ImageError`。 [批处理循环](../kernel/src/batch.rs) · [内核入口及错误处理](../kernel/src/main.rs)

### 2.5.7 用具体地址手算一次 `exit(42)`

这次手算采用前面反汇编中的内核代码地址。为便于独立推演，假设内核进入 `__kr_run_app` 时 `MSP = 0x2405_f900`，函数调用的普通 `lr = 0x0800_3a11`。应用入口字段和栈顶沿用 `0x2406_0001` 与 `0x2408_0000`。应用里的 SVC 指令位置和触发时的 PSP 也是假设值，用来展示栈在经过 `_start()` 和其他函数调用后如何变化。取两次异常前的 SP 都已八字节对齐，每份基本帧占 `0x20` 字节。

| 检查点 | MSP | PSP | 寄存器或内存中关键的值 |
| --- | --- | --- | --- |
| 内核执行地址 `0x0800_37cc` 的 `svc #0` 前 | `0x2405_f900` | 无需使用 | 活 `r0 = 0x2407_ffe0`，活 `lr = 0x0800_3a11` |
| 第一次进入 `SVCall` | `0x2405_f8e0` | 尚未设置应用帧 | 活 `lr = 0xffff_fff9`，`[MSP + 0x18] = 0x0800_37ce` |
| 保存软件区，准备第一次异常返回 | `0x2405_f8b8` | `0x2407_ffe0` | `[MSP + 0x24] = 0xffff_fff9`，活 `lr` 改为 `0xffff_fffd` |
| 应用入口开始执行 | `0x2405_f8b8` | `0x2408_0000` | `pc = 0x2406_0000`，活 `r0 = 0`，`CONTROL & 3 = 3` |
| 应用执行地址 `0x2406_0150` 的 `svc #1` 前 | `0x2405_f8b8` | `0x2407_ffd0` | 活 `r0 = 0x0000_002a`，`r1 = 0` |
| 第二次进入 `SVCall` | `0x2405_f8b8` | `0x2407_ffb0` | 活 `lr = 0xffff_fffd`，`[PSP + 0x18] = 0x2406_0152`，`[PSP] = 0x0000_002a` |
| 出栈软件区并写回退出码 | `0x2405_f8e0` | `0x2407_ffb0` | 活 `lr = 0xffff_fff9`，`[MSP] = 0x0000_002a` |
| 回到 `.Lresume_kernel` | `0x2405_f900` | `0x2407_ffb0`，当前 Thread 使用 MSP | `pc = 0x0800_37ce`，活 `r0 = 0x0000_002a`，活 `lr = 0x0800_3a11` |

第一次进 Handler 时，内核帧的普通 LR 和 PC 位于 `0x2405_f8f4` 和 `0x2405_f8f8`。在软件区压栈之后，`0x2405_f8dc` 存的是另外一个 LR，即 `EXC_RETURN`。它紧挨着更高一字 `0x2405_f8e0`，那是内核硬件帧的首字。把两个地址及内容并列查看，就能预测 `ldmia` 后的 MSP 会落在哪里，也能判断何时应从活 LR 读返回令牌、何时应从硬件帧中找普通返回地址。

应用再次进入 Handler 后，`mrs r0, PSP` 令活 `r0 = 0x2407_ffb0`。`ldr r2, [r0, #24]` 从 `0x2407_ffc8` 读到 `0x2406_0152`。`subs r2, #2` 使它变成 `0x2406_0150`，小端指令字节 `01 df` 经 `ldrh` 就是 `0x0000_df01`。比较时活 `r1` 也等于 `0x0000_df01`，应用帧里 `[0x2407_ffb4]` 依然是应用传入的零。校验通过后，`ldr r0, [r0]` 把活 `r0` 从内存地址变成十进制 42。退出分支没有从 PSP 弹出应用帧，随后恢复内核使用的是 MSP，这份应用帧没有再恢复的必要。下一应用通过 `run_app()` 新建一份初始帧时，会重新为 PSP 指定起点。

这里有两个可验证的因果关系。第一，应用第一份帧在初次返回后释放，所以第二次异常的帧地址随应用栈深度改变。第二，内核的 MSP 在应用执行期间指向软件保存区，后续只恢复 `sp + 0x28` 到原来硬件帧，才能把退出码写到正确槽位。如果在弹出软件区之前直接执行 `str r0, [sp]`，它会覆盖软件区的 `r3` 而非内核硬件帧的 `r0`。应用看起来成功触发了 SVC，内核却会收到启动时压栈的指针位型。这样可以从返回值反推出最可能检查的栈位置。 [异常汇编](../kernel/src/trap/svc.s)

### 2.5.8 普通系统调用如何返回应用

应用执行 `svc #2` 后，`.Luser_svc` 读出 `0xdf02`，转到 `.Lwrite_app`。此时活 `r0` 指向应用基本帧。`push {r4, lr}` 把应用的 `r4` 和本次 `EXC_RETURN` 保存到 MSP，正好占 8 字节。把帧地址暂存到 `r4`，是因为 C ABI 要求被调用者保留它，而 `r0` 和 `r1` 要用来传入应用帧中的缓冲区地址和长度。`bl` 会覆盖 LR，所以调用前必须保留异常返回令牌。

Rust 返回后，`str r0, [r4]` 把结果写回 **PSP 上的应用帧**。汇编只弹出写入分支自己压入的两个字，再按原令牌异常返回，应用从 SVC 的下一条指令继续。内核等待退出的现场仍保留在 MSP 上。

```rust
#[unsafe(no_mangle)]
extern "C" fn __kr_sys_write(addr: u32, len: u32) -> i32 {
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

范围检查覆盖整块应用 SRAM，包含 PSP 栈和段间空洞。代码依赖受信任的应用传入已初始化字节，运行时隔离仍需 MPU。handler 使用独立的全局队列接口，因此无需取得第二份控制台驱动的可变借用。外层批处理等到应用退出后轮询排空队列，再打印退出码。写入返回 5 表示已接收 5 字节，串口此时仍可能没有输出。 [写入实现](../kernel/src/trap.rs) · [控制台](../kernel/src/drivers/console.rs) · [批处理](../kernel/src/batch.rs)

### 2.5.9 对照 krunix 和 rCore 的控制流

rCore 第 2 章和本地 krunix 都有“先构造应用现场，再使用与陷阱返回相同的机制首次进入应用”的设计。把实现目标对齐，就能确定哪些部分应迁移到 Cortex-M7，以及哪些寄存器与栈操作由不同架构负责。RISC-V 部分分析以 rCore 教程本节及 krunix 的 `os/src/trap/mod.rs`、`os/src/trap/context.rs`、`os/src/trap/trap.S`、`os/src/batch.rs` 为依据。 [rCore 的实现特权级切换](https://rcore-os.cn/rCore-Tutorial-Book-v3/chapter2/4trap-handling.html) · [RISC-V Supervisor ISA](https://docs.riscv.org/reference/isa/priv/supervisor.html)

| 需要解决的问题 | rCore 第 2 章与 krunix 的 RV64 路径 | kr-stm-os 的 Cortex-M7 路径 |
| --- | --- | --- |
| 让用户代码陷入 | `ecall` 引发 U-mode 环境调用，进入 S-mode | `svc #x` 引发 SVCall，进入特权 Handler |
| CPU 在哪找入口 | `stvec` 指向 `__alltraps`，读取 `scause` 区分异常原因 | 异常向量表固定下标 11 关联 `SVCall` |
| 异常前后的栈 | 汇编用 `csrrw sp, sscratch, sp` 交换用户和内核栈地址 | 硬件在被打断的栈压入基本帧，Handler 使用 MSP |
| 现场布局 | 汇编在内核栈分配 `34 × 8 = 272` 字节并存放 `TrapContext` | 硬件处理 32 字节基本帧，启动汇编额外保存 40 字节内核状态 |
| 最初如何进入应用 | 构造 `TrapContext`，用 `__restore` 和 `sret` 恢复 | 在 PSP 准备基本帧，用 `bx lr` 和 `EXC_RETURN` 恢复 |
| 确定下次 Thread 或 U-mode 的权限 | `sstatus.SPP = User`，供 `sret` 使用 | `CONTROL.nPRIV = 1`，令返回后的 Thread 非特权 |
| 普通调用从哪里继续 | `sepc` 起初是 `ecall` 的地址，处理时加 4 | SVC 压栈 PC 已指向下一条，减 2 只用于读出服务号 |
| 服务号和参数 | `a7` 传服务号，`a0` 到 `a2` 传参数，`a0` 接收返回值 | `svc` 的立即数传服务号，`r0` 和 `r1` 传参数，`r0` 接收返回值 |
| 结束本轮应用 | `sys_exit` 打印退出码后从陷阱处理路径直接启动下一应用 | `sys_exit` 对应的汇编恢复暂停的内核 Thread，由批处理循环继续 |

RISC-V 进入 S-mode 时，会更新 `sepc`、`scause` 等 CSR，并按 `stvec` 进入内核代码。通用寄存器和 SP 的保存由软件安排。krunix 在用户代码运行期间让 `sscratch` 持有内核栈顶，于是 `__alltraps` 的第一条指令 `csrrw sp, sscratch, sp` 同时取得内核栈和暂存用户 SP。立即切到可信的内核栈后，汇编向下分配 272 字节。现场中的前 32 槽对应 `x0` 到 `x31`，但实现跳过恒零的 `x0` 和当前应用约定不用的 `tp`。`x2` 也需要特殊处理，因为交换后的 `sp` 已经是内核地址，所以原用户 SP 要从 `sscratch` 取回，再写到 `x[2]`。最后两槽是 `sstatus`、`sepc`，在 RV64 上偏移分别是 `0x100`、`0x108`。Cortex-M7 的 PSP/MSP 在硬件里已经是两个栈指针，应用进入异常时 `SP` 不用通过一个 CSR 与另一个地址交换，这也决定了不能照搬 `csrrw` 对应的汇编序言。 [rCore 的上下文保存](https://rcore-os.cn/rCore-Tutorial-Book-v3/chapter2/4trap-handling.html#id8) · [RISC-V Supervisor ISA](https://docs.riscv.org/reference/isa/priv/supervisor.html)

RISC-V 启动应用时，在内核栈上准备 `TrapContext`，将 `x[2]` 设为用户栈顶、`sepc` 设为应用入口、`sstatus.SPP` 设为 User。`__restore` 取得帧指针后恢复 CSR 与通用寄存器，再交换 `sp` 与 `sscratch`，最后 `sret`。kr-stm-os 的等效操作分布到 PSP 上的初始帧、Thread 权限位和异常返回令牌里。两个入口都需要可恢复的初始地址、用户栈和权限状态。把 RV64 的 `x[2]` 机械地放到 Cortex-M7 基本帧会错位，因为 Cortex-M7 通过异常返回选栈与回收帧来得出恢复后的 SP。`EXC_RETURN` 也承担不了 `sstatus.SPP` 的全部角色。`0xffff_fffd` 表达 Thread 和 PSP，非特权还要由 `nPRIV` 表达。 [rCore 的首次执行应用](https://rcore-os.cn/rCore-Tutorial-Book-v3/chapter2/4trap-handling.html#ch2-app-execution) · [Arm Cortex-M7 异常返回](https://documentation-service.arm.com/static/61efd8352dd99944d05142d1?token=)

以两边的写服务为例，krunix 的用户库让 `a7 = 64`，并将文件描述符、地址和长度放在 `a0`、`a1` 和 `a2`。内核从 `x[17]` 取得编号，从 `x[10..=12]` 取得三项数据，`sys_write` 检查这条路径支持的标准输出描述符并打印 UTF-8，结果写回 `x[10]`。普通 `ecall` 的返回要先使 `cx.sepc += 4`，否则 `sret` 又回到 `ecall`。kr-stm-os 用户库生成 `svc #2`，当前接口只传地址和长度，序号在机器码的低八位。将在 PSP 帧中读 `[frame]`、`[frame + 4]`，将结果写 `[frame]`。`frame.pc - 2` 只用于定位 SVC 编码，直接保存原 PC 才能让调用恢复在下一条指令。如果仿照 RISC-V 再给压栈 PC 加四，就会跳过应用汇编之后的一部分指令。若错误写回 `frame.pc - 2`，恢复的应用会反复执行 SVC。 [rCore 的陷阱分发与系统调用](https://rcore-os.cn/rCore-Tutorial-Book-v3/chapter2/4trap-handling.html#id9) · [STM32 用户 ABI](../user/src/runtime/syscall.rs)

退出也体现了两种批处理组织。krunix 用 `a7 = 93` 请求退出，`sys_exit` 直接调用 `run_next_app() -> !`，它在处理路径中加载下一个应用并切换到新 `TrapContext`。krunix 的 `StoreFault`、`StorePageFault` 和 `IllegalInstruction` 也有显式分支，会报告后尝试运行下一个应用。kr-stm-os 使用暂停内核 Thread 的方式运行本轮应用，关键是等退出后收回原硬件帧，再回到 Rust 的 `for` 循环。它目前没有走故障恢复来执行下一应用的分支。两套系统都复用用户执行区，但一套将推进批次放在陷阱处理路径里，另一套在调用装载器的原控制流中推进。对照源码时可追踪“结束后谁拿到控制权”。它决定退出码写到哪里，以及内核需要保存怎样的后续状态。 [rCore 的应用执行](https://rcore-os.cn/rCore-Tutorial-Book-v3/chapter2/4trap-handling.html#ch2-app-execution) · [STM32 批处理循环](../kernel/src/batch.rs)

### 2.5.10 当前切换协议的适用边界

这条路径要求内核从特权 Thread mode 使用 MSP 启动，优先级和异常屏蔽状态允许 SVCall 进入，并且只有一个活跃应用。`run_app` 把这些条件写进安全契约，其中有的条件由当前启动路径保证。比如，如果在禁止相应异常的临界区内发出 SVC，CPU 就可能按异常优先级规则升级为 HardFault。在 Handler 中再请求 SVC，也要考虑当前异常的活动态和优先级。示例中的两个入口比较是针对固定场景的来源识别，适用于逐次运行的启动和退出测试。将来引入 PendSV、抢占或多个应用时，应让上下文记录带有自己的栈和运行状态，并建立统一的入栈、选择下一应用、恢复顺序，仅保留一份内核调用链的方式无法直接承担同时存在的不同用户现场。 [切换契约](../kernel/src/trap.rs) · [ARMv7-M 优先级及抢占](https://support.arm.com/documentation/ddi0403/d/System-Level-Architecture/System-Level-Programmers--Model/ARMv7-M-exception-model/Exception-priorities-and-preemption)

浮点也是一条明确的边界。Rust 编译目标是 `thumbv7em-none-eabihf`，`eabihf` 描述面向硬件浮点的 ABI，而异常发生时处理器是否采用扩展帧由实际浮点上下文决定。即使本轮应用用的主要是整数，也要考虑启动环境或服务代码是否使浮点上下文变为活跃。当前 Handler 用完整的 `EXC_RETURN` 与两个基本帧令牌比较，因此扩展帧会进入未支持路径。若扩展为支持浮点，需要一并决定保存并恢复哪些浮点寄存器，如何处理惰性压栈，以及从被暂停的一侧恢复怎样的状态。仅接受更多的 `EXC_RETURN` 常量会绕过当前检查，但不能给这套人工构造的帧添加缺少的浮点状态。 [目标配置](../.cargo/config.toml) · [Arm Cortex-M7 浮点异常上下文](https://documentation-service.arm.com/static/61efd8352dd99944d05142d1?token=)

退出服务当前直接读取 PSP 帧和帧内 PC 对应的半字，适用于项目要求的可信测试应用。要对外接收独立构建的应用，特权 Handler 读取用户提供的信息前应验证帧所在区间及 PC 对应的可执行范围，并处理 PSP 溢出、异常压栈未完成、非法指令和未知服务。找到代码范围后，仍需在减二前检查地址下界，避免针对无效 PC 的半字加载触发第二个异常。若准备从应用 Fault 回到批处理，必须区分故障来自用户 Thread、内核 Thread 还是 Handler，并验证可用的内核现场仍完整。来自内核的故障需要保留诊断语义，来自应用且允许回收的故障才适合转成应用失败。建立这条边界后，后续扩展写入、故障终止或调度时，都能明确回答三件事：现场从哪条栈读，结果写回哪一份帧，异常最终返回哪个 Thread。 [当前 SVC 处理](../kernel/src/trap/svc.s) · [Arm Cortex-M7 Fault 状态与栈错误](https://documentation-service.arm.com/static/61efd8352dd99944d05142d1?token=)

## 2.6 逐步验证与板上实验

这组实验以一个只返回整数的应用开始。串口能看到 `42`，说明应用从 QSPI 到了 SRAM，在非特权线程中走完入口，并将结果经异常返回交给内核。为了查清其中某一步出了问题时该往哪里看，我们同时保留应用 ELF、KAPP 文件、板上读回文件和调试记录。它们分别回答链接器放了什么、打包器保存了什么、烧录器写了什么、CPU 最终执行了什么。每个检查的预期都来自本章实现。板上输出和断点值应以读者实际观察为准。

### 2.6.1 用两个整数应用建立基线

准备支持 `thumbv7em-none-eabihf` 的 Rust 工具链、GNU Make、Python 3 和 LLVM 的 `llvm-readelf`、`llvm-objdump`。先在工程根目录运行 `rustup target list --installed`，检查 ARM 目标组件已经安装。离线模式假定 Rust 工具链、这个目标的标准库组件和 Cargo 依赖都已在本机。初次准备时可联网执行 `rustup target add thumbv7em-none-eabihf` 和 `cargo fetch --locked --target thumbv7em-none-eabihf`。`--locked` 固定依赖解析结果，`--offline` 禁止 Cargo 获取缺失依赖。两者分别约束版本和网络。后面的 `CARGO_NET_OFFLINE=true` 会同时传给固件构建以及 Makefile 另行启动的主机打包器。 [构建入口](../Makefile) · [Cargo 目标设置](../.cargo/config.toml)

在工程根目录运行下面这段 Bash，创建 `00exit42` 和 `02exit7`。命令检测两个路径是否已存在，便于在有过实验记录的工作树里保留原文件。如果已存在，请对照内容后再继续。Cargo 会发现 `user/src/bin/` 下对应的文件，二进制名取自文件名。两个应用复用同一个运行时，唯一需要区别的行为是 `main()` 的返回值。使用两个不同的非零整数，方便在串口上辨出顺序，也避免把意外残留的零当成正确结果。

```bash
if [ -e user/src/bin/00exit42.rs ] || [ -e user/src/bin/02exit7.rs ]; then
    printf '%s\n' '实验文件已存在，请先核对其内容。' >&2
else
    cat > user/src/bin/00exit42.rs <<'RUST'
#![no_std]
#![no_main]

use user_lib as _;

#[unsafe(no_mangle)]
fn main() -> i32 {
    42
}
RUST

    cat > user/src/bin/02exit7.rs <<'RUST'
#![no_std]
#![no_main]

use user_lib as _;

#[unsafe(no_mangle)]
fn main() -> i32 {
    7
}
RUST
fi
```

`#![no_main]` 让链接脚本和用户运行时决定入口。`use user_lib as _;` 将运行时链接进 ELF，而 `#[unsafe(no_mangle)]` 使运行时能按符号名找到测试函数。`main()` 执行普通的 Rust 返回后，`_start()` 把它的结果传给 `exit()`。这两个用例在进入 SVC 前只做整数运算，符合当前基本异常帧路径的前提。也请保持内核沿这条路径只使用基本帧。目标名里的 `eabihf` 说明浮点 ABI，运行中是否实际压入浮点上下文还要由异常入口的 `EXC_RETURN` 判断。 [用户运行时](../user/src/runtime.rs) · [SVC 汇编](../kernel/src/trap/svc.s)

下面的命令可在同一个 Bash 会话顺序执行。每个命令正常退出后再进入下一步。固定使用 debug 构建，便于与仓库的 VS Code 调试配置和符号地址对应。将镜像放到独立子目录并使用三个文件名，可以在后续直接比较每轮结果，无需靠记忆确认最后一次打包覆盖了哪个文件。`APPS` 交给打包器决定镜像内容和顺序，`build-apps` 前置目标仍会编译用户 crate 的所有二进制。这里用显式 `APPS` 隔离退出实验。随后用 `00hello` 验证写入。 [Makefile](../Makefile) · [应用启动与退出指南](svc-app-start-exit.md)

```bash
export CARGO_NET_OFFLINE=true
build_dir=target/thumbv7em-none-eabihf/debug
lab_dir=output/ch2

make PROFILE=debug build
make PROFILE=debug pack-apps APPS=00exit42 \
    QSPI_IMAGE="$lab_dir/single.img"
make PROFILE=debug pack-apps APPS='00exit42 02exit7' \
    QSPI_IMAGE="$lab_dir/pair.img"
make PROFILE=debug pack-apps APPS='02exit7 00exit42' \
    QSPI_IMAGE="$lab_dir/reversed.img"
```

打包器应依次报告 `1 application(s)`、`2 application(s)` 和 `2 application(s)`。目录项只包含应用名、带 Thumb 标记的入口和该应用的段表位置。两个应用的 ELF 入口地址相同，因此仅核对入口无法确认顺序。下一步还要检查目录顺序及每个目录项实际关联的段。若构建阶段报错，先看是 ARM 目标组件、离线依赖还是源代码问题。哪怕输出目录里已经有同名镜像，也要等这一轮打包成功再使用文件。旧镜像就算通过 CRC，也可能来自完全不同的应用或前一次实验。

### 2.6.2 在主机上核对 ELF、KAPP 和 CRC

用 `llvm-readelf` 读取两个 ELF 的文件头和程序头，用 `llvm-objdump` 留下反汇编。检查应用是 ARM 小端 ELF32，入口预计为 `0x24060001`。可执行的 `LOAD` 段从应用 SRAM 开始，`FileSiz` 描述文件中已有的字节，`MemSiz` 说明段在内存里要占多少。对于这两个最小程序，可能只出现一个有内容的 `LOAD` 段，也可能看到 ELF 的 `EXIDX` 程序头。打包器只处理有内存长度的 `PT_LOAD`，以文件实际输出为准。 [应用链接脚本](../user/link.x) · [ELF 解析器](../tools/pack/src/elf.rs)

```bash
llvm-readelf -hW -lW "$build_dir/00exit42"
llvm-readelf -hW -lW "$build_dir/02exit7"
llvm-readelf -sW "$build_dir/kr-stm-os" | rg 'SVCall|__kr_run_app'
llvm-readelf -x .vector_table "$build_dir/kr-stm-os"
llvm-objdump -d "$build_dir/00exit42" > "$lab_dir/exit42.dis"
llvm-objdump -d "$build_dir/02exit7" > "$lab_dir/exit7.dis"
```

内核 ELF 的两个函数符号各有用途。`__kr_run_app` 是内核主动发起启动 SVC 的位置，`SVCall` 是处理器根据向量表取得的入口。在 `.vector_table` 中找向量编号 11，也就是从表基址起算的 `11 × 4 = 0x2c` 处。按小端字解读该位置的四个字节，与 `SVCall` 入口核对，保留向量里的 Thumb 标记位。工程当前把向量表链接到片内 Flash 的 `0x08000000`。如果板上调试时符号存在而断点没有命中，还需读取实际 `VTOR` 和实际第 11 项，因为符号表只能反映手头这个 ELF。 [内核内存布局](../kernel/memory.x) · [向量入口](../kernel/src/trap/svc.s)

用下面的只读检查独立验证三份产物。脚本假定输入是刚打包的 Cortex-M7 实验文件，此处以 `assert` 报告不匹配，使用普通 `python3` 运行。程序头含 `p_vaddr`、`p_paddr` 两个地址，脚本选 `p_vaddr`，与打包器计算 SRAM 目的地址的规则一致。它还逐字节核对 KAPP payload 与原 ELF 的文件数据。目录里的应用名和入口能够正确，段数据却依然可能与预期文件不符，因此这些检查需要同时覆盖。 [KAPP 编码器](../tools/pack/src/image.rs) · [内核镜像校验](../kernel/src/batch/image.rs)

```bash
python3 - "$build_dir" "$lab_dir" <<'PY'
from pathlib import Path
from struct import unpack_from
import sys
import zlib

build_dir, lab_dir = map(Path, sys.argv[1:])
cases = {
    "single.img": ("00exit42",),
    "pair.img": ("00exit42", "02exit7"),
    "reversed.img": ("02exit7", "00exit42"),
}

for filename, expected in cases.items():
    data = (lab_dir / filename).read_bytes()
    magic, version, size, count, app_off, crc = unpack_from("<4s5I", data)
    assert (magic, version, app_off) == (b"KAPP", 1, 24), filename
    assert size == len(data) and count == len(expected), filename
    computed = zlib.crc32(data[:20] + bytes(4) + data[24:])
    assert computed == crc, (filename, hex(crc), hex(computed))

    records = [unpack_from("<16s3I", data, app_off + i * 28)
               for i in range(count)]
    segment_cursor = app_off + 28 * count
    payload_cursor = segment_cursor + 20 * sum(record[2] for record in records)
    for name, (raw, entry, nsegments, segment_off) in zip(expected, records):
        assert raw == name.encode().ljust(16, b"\0"), (filename, name, raw)
        assert entry == 0x24060001 and segment_off == segment_cursor
        elf = (build_dir / name).read_bytes()
        assert elf[:7] == b"\x7fELF\x01\x01\x01"
        elf_entry, ph_off = unpack_from("<2I", elf, 24)
        ph_size, ph_count = unpack_from("<2H", elf, 42)
        assert entry == elf_entry and ph_size == 32
        load = []
        for i in range(ph_count):
            typ, off, vaddr, _, file_len, mem_len, flags, _ = unpack_from(
                "<8I", elf, ph_off + i * ph_size
            )
            if typ == 1 and mem_len:
                load.append((off, vaddr, file_len, mem_len, flags))
        assert nsegments == len(load)
        for off, vaddr, file_len, mem_len, flags in load:
            record = unpack_from("<5I", data, segment_cursor)
            assert record == (vaddr, payload_cursor, file_len, mem_len, flags)
            assert 0 <= file_len <= mem_len
            assert 0x24060000 <= vaddr < vaddr + mem_len <= 0x2407f000
            assert payload_cursor + file_len <= size
            assert data[payload_cursor:payload_cursor + file_len] == elf[off:off + file_len]
            segment_cursor += 20
            payload_cursor += file_len
    assert payload_cursor == size
    print(f"{filename}: {' -> '.join(expected)}, {size} bytes, CRC32 {crc:08x}")
PY
```

这里计算 CRC 时将镜像偏移 `20..24` 处视为四个零，算法与编码器一致。先从文件中记下存储的 CRC，再把这四个字节临时替换为零，比较两种结果。镜像存储的长度给出 CRC 的边界。用固定的 4096 字节或 NOR 芯片全片长度计算，会把镜像外的数据也算进去。打包器输出的文件恰好等于 `total_len`，后面的读回实验也选这个长度。镜像内容的一点点变化都可能产生不同的 CRC，所以不要求三个镜像与别人的构建结果使用相同数值。报告需要记录手头文件的实测值和应用顺序。

### 2.6.3 核对板卡并烧录单应用

以下操作面向使用仓库板级配置的 STM32H743VGT6 和 W25Q64JV。连接 ST-LINK 的 SWDIO、SWCLK、GND、NRST，按探针要求接目标电压参考。串口使用 3.3 V TTL，板端 PA9 接转接器 RX，两侧 GND 相连，只观察日志时 PA10 可以留空。将终端设为 **19200 baud、8N1、无流控**，先打开日志窗口再复位，避免丢掉只发一次的启动消息。LED 只能作为旁证，代码控制的是 PC13 电平，灯的亮灭还由板载电路决定。 [板级设置](../kernel/src/board.rs) · [接线说明](../README.md#查看串口输出) · [批处理实施指南](batch-implementation.md#21-板上的引脚和三个连接路径)

外部 Flash loader 需要与板卡的管脚映射和 NOR 型号匹配。仓库默认使用 DevEBox 的 `STM32H743VI_W25Q64.stldr`。已有该文件时 `make qspi-loader` 会校验摘要，缺少时会联网下载。离线实验应提前准备好 loader；自定义板卡可通过 `QSPI_LOADER` 指定已确认匹配的文件。CubeProgrammer CLI 若采用原始名称，给硬件相关命令传入 `PROGRAMMER=STM32_Programmer_CLI`。烧录和调试共享同一个探针，先关闭已连接的调试会话再运行 CubeProgrammer。不要依靠两个程序同时占用探针来维持断点。 [烧录规则](../Makefile) · [仓库烧录说明](../README.md#构建与烧录)

确认三份镜像覆盖到的整个扇区可用于实验。写入的起始地址固定 `0x90000000`，这是烧录工具对应 NOR 偏移零的地址，也是当前内核读头的位置。NOR 按 4096 字节扇区擦除，镜像结尾即使落在扇区中间，同一扇区其余数据也可能受影响。若板上已经有需保留的数据，事先读出并保存这些完整扇区。以下代码在同一个 Bash 会话中使用上文的 `$lab_dir`；`make qspi-read` 只读，备份命令可用于已有数据的板卡。`backup` 目标文件应另选一个尚未使用的名字，便于之后确认原始内容。

```bash
max_size=0
for image in "$lab_dir/single.img" "$lab_dir/pair.img" "$lab_dir/reversed.img"; do
    size=$(wc -c < "$image")
    if [ "$size" -gt "$max_size" ]; then
        max_size=$size
    fi
done
backup_size=$(( (max_size + 4095) / 4096 * 4096 ))
make qspi-read QSPI_ADDR=0x90000000 QSPI_SIZE="$backup_size" \
    QSPI_OUTPUT="$lab_dir/qspi-before-ch2.bin"
```

写单应用镜像，立即读回它所占的精确字节数，并用 `cmp` 核对。保持烧录和读回的地址、loader、探针参数一致。`qspi-write` 内部已经调用写入校验，独立读回是为了将板上内容留成文件，便于出现异常时定位差异。片内固件仍需要单独烧录，`flash-kernel` 依照内核 ELF 的地址写片内 Flash。请在每一步成功后再继续。若板上有需要保留的旧内核，先按板卡流程保留对应固件或备份相关区域。

```bash
make qspi-write QSPI_ADDR=0x90000000 QSPI_IMAGE="$lab_dir/single.img"
make qspi-read QSPI_ADDR=0x90000000 \
    QSPI_SIZE="$(wc -c < "$lab_dir/single.img")" \
    QSPI_OUTPUT="$lab_dir/single-readback.img"
cmp "$lab_dir/single.img" "$lab_dir/single-readback.img"
make PROFILE=debug flash-kernel
```

`cmp` 正常退出且无文字表示两个文件相同。读回确认的是外部 loader 能取到相同内容。板上启动后走的是内核自己的 QSPI 初始化、间接读和 CRC 校验，它们仍要分别验证。此时按板上的复位键，预期串口出现以下输出。文中的 `<行号>` 取决于构建时的源文件，panic 文本可能包含终端用于着色的 ANSI 转义码。

```text
kr-stm-os booted
app[0] exited: 42
Batch complete

[kernel panic]
Panicked at kernel/src/main.rs:<行号> Done!
```

在 `main()` 中，`batch::run()` 返回后会故意调用 `panic!("Done!")`，用来演示 panic DMA，因而把 `Batch complete` 作为批处理的成功信号。即使镜像解析返回错误，`main()` 也会先打印 `KAPP error: ...`，然后执行同一个 `Done!`。孤立的一行 panic 文本因此无法证明应用已经运行。应用执行不合法指令或发起未知 SVC时，则由处理器走 Fault 路径，内核目前没有应用 Fault 恢复和跳到下一项的逻辑。遇到静默停机应检查当前 PC 和异常寄存器。 [内核入口](../kernel/src/main.rs) · [panic 处理](../kernel/src/panic_handler.rs) · [SVC 分派](../kernel/src/trap/svc.s)

### 2.6.4 跟着断点确认栈与异常返回

先用串口完成单应用的一轮连续运行，再让调试器接管。连续运行所得日志回答实验整体是否完成，断点回答控制权在哪里交接。VS Code 的 `Debug STM32H743VGT6` 配置使用 probe-rs，默认会构建并烧录片内内核，配置中的 `haltAfterReset` 为 `false`。若想从复位保持暂停，需要在有相应支持的调试设置中选择停在复位，或连上后复位并暂停。调试配置的烧录对象是内核 ELF，QSPI 应用仍由前面命令写入。使用 GDB 时先通过支持目标板的 SWD GDB server 建立连接，并加载这轮内核 ELF 的符号。下面的 GDB 表达式依赖后端公开 `msp`、`psp`、`control` 等系统寄存器，未公开时用调试器提供的核心寄存器视图；只有通用 `sp` 时，应明确它在当前执行模式下看到的是哪条栈。 [调试配置](../.vscode/launch.json) · [断点观察指南](svc-app-start-exit.md#6-在调试器里逐站验证)

先对 `__kr_run_app` 和 `SVCall` 做反汇编，找出处理启动的第一个 `bx lr`、内核 `svc #0` 后面的恢复点。地址随编译而变化，从正在调试的 ELF 读取。应用入口则由链接脚本固定为 `0x24060000`。应用指令到运行时才写入 RAM，所以在尚未装载时要用硬件断点，避免软件断点写入 RAM 后被装载覆盖。可以从 `hbreak __kr_run_app` 开始，命中后删掉这个断点，再设 `hbreak SVCall` 和需要的地址断点。目标可用的硬件断点有限，逐站替换即可。

```gdb
info registers pc sp lr
info registers msp psp control
disassemble __kr_run_app
disassemble SVCall
hbreak SVCall
hbreak *0x24060000
```

在可用的调试后端上，按照下表逐站拍下 PC、当前模式和栈值。也可以只做一轮完整记录，之后分别复位重跑有疑问的检查点。读内存前先在指定指令执行之前停住，因为一次入栈或出栈就会改变同一个寄存器所指的对象。

| 停止位置 | 记录与预期 | 读这一处的理由 |
| --- | --- | --- |
| `__kr_run_app` 入门，内核还未执行启动 SVC | `r0` 指向 `0x2407ffe0`；读取 `0x24060000` 起的指令半字，与本轮应用 ELF 的 `_start` 对照 | 检查应用确实已被加载，初始帧也已写好；这里的相同地址还可以用于比较第二轮应用 |
| 第一次进入 `SVCall` 的首条指令 | `lr = 0xfffffff9`，`x/8wx $msp` 的首字为应用初始帧指针，第七字为内核 `svc #0` 后一条指令地址 | 识别由内核线程触发的基本帧，并找到将来应恢复的准确位置 |
| 启动分支的 `bx lr` 执行前 | `lr = 0xfffffffd`，`psp = 0x2407ffe0`，第七字 `0x24060000`、第八字 `0x01000000` | 对比异常返回值指定的栈与栈内将被使用的 PC、Thumb 状态 |
| 应用入口的首条指令执行前 | `pc = 0x24060000`，`psp = 0x24080000`，`control & 3 = 3` | CPU 已弹出初始帧并进入非特权 Thread；入口代码开始后常会建立普通调用栈，PSP 随即变化 |
| 第二次进入 `SVCall` 的首条指令 | `lr = 0xfffffffd`，`x/8wx $psp` 首字为 `0x2a`，从帧 PC 减 2处读取半字得到 `0xdf01` | 退出码保存在应用的基本异常帧；指令编码确认这一回是 `svc #1` |
| 内核 `svc #0` 的下一条指令执行前 | `r0 = 42`，`control & 3 = 0`，当前线程回到 MSP | 对照第一次进门时的内核续执行 PC，证明退出码和内核的控制流同时得到恢复 |

在第一次进门时，可先保存所见的 MSP 地址作为参照。软件再把十个字压到同一条栈上，MSP 减少 40 字节。进行第二次 SVC 时，硬件向 PSP 压入应用现场，留下 MSP 上那十个字。退出分支先从 MSP 恢复软件保存区，再将退出码写到内核基本帧的首字。此时 MSP 应再次指向最初那份硬件帧。若对单步现象有疑问，用新一次复位重跑并检查两个时刻的 MSP，比停住后往栈上改值更容易保留证据。

下面的命令适合在第二次命中 `SVCall` 的第一条指令处使用。栈里的第七字是被打断上下文的 PC 槽，Thumb `svc` 本身只有两个字节，帧里保存的是往后继续的位置，所以需要减二才能读到该指令。这里不对调试器面板上同时出现的两个 LR 作同一解释。核心寄存器 `lr` 是异常入口的返回令牌，帧里第六字是被打断代码保存下来的普通 LR。

```gdb
x/8wx $psp
set $svc_pc = *(unsigned int *)($psp + 0x18)
x/1hx ($svc_pc - 2)
x/8hx 0x24060000
```

第二次进门时的 PSP 取决于 `_start()`、`exit()` 及内联代码在这一份构建中压了多少栈，也可能受异常入栈对齐影响。`0x2407ffe0` 是第一份合成帧的确切位置。退出时再发现 PSP 等于这个数值，只能说明这次栈恰好落在那里，仍应读取当下 PSP 指向的内容。实际异常帧的 PC 和 xPSR 才是正确的判断依据。

### 2.6.5 装入两个应用，再交换顺序

保持同一个内核和探针配置，首先烧 `pair.img`。读回与比较继续使用应用镜像本身的长度。停止上一轮调试会话再烧录；写完并核对后手动复位，观察新的启动日志。由于内核沿用刚才验证的同一份 ELF，这一轮更改的是 KAPP 的应用目录和负载。

```bash
make qspi-write QSPI_ADDR=0x90000000 QSPI_IMAGE="$lab_dir/pair.img"
make qspi-read QSPI_ADDR=0x90000000 \
    QSPI_SIZE="$(wc -c < "$lab_dir/pair.img")" \
    QSPI_OUTPUT="$lab_dir/pair-readback.img"
cmp "$lab_dir/pair.img" "$lab_dir/pair-readback.img"
```

```text
kr-stm-os booted
app[0] exited: 42
app[1] exited: 7
Batch complete
```

第一次 `run_app()` 返回后，内核先打印 `app[0]`，随后进入下一次 `loader::load()`。如果在 `__kr_run_app` 入口设置硬件断点，应能停两次。第一次停住时查看 `00exit42` 的实际装载字节，第二次查看 `02exit7` 的实际装载字节。两者的公共 `_start` 指令可能完全相同。寻找差异时，先从两份 `*.dis` 定位 `main`，或根据两个 ELF 的程序段字节找到差异偏移，再读 SRAM 的 `p_vaddr + 偏移`。两轮都使用同一个入口和栈顶，差异来自恰在当轮被装入的内容，不能只靠入口地址分辨应用。若只盯着 SVC，则会按启动、退出、启动、退出的顺序到达四次。 [批处理循环](../kernel/src/batch.rs) · [段加载](../kernel/src/batch/loader.rs)

接着把 `reversed.img` 写到相同地址并读回。保持两个 ELF 和内核都不变，可以把观察到的结果归因给打包顺序。

```bash
make qspi-write QSPI_ADDR=0x90000000 QSPI_IMAGE="$lab_dir/reversed.img"
make qspi-read QSPI_ADDR=0x90000000 \
    QSPI_SIZE="$(wc -c < "$lab_dir/reversed.img")" \
    QSPI_OUTPUT="$lab_dir/reversed-readback.img"
cmp "$lab_dir/reversed.img" "$lab_dir/reversed-readback.img"
```

```text
kr-stm-os booted
app[0] exited: 7
app[1] exited: 42
Batch complete
```

倒序同时控制了两项干扰。文件名依旧按 `00`、`02` 排列，因而可以看出内核按镜像表的顺序取应用。两个应用始终使用同一份运行时和同一个载入区，交换的结果又能检验每次退出后是否真的读入下一项。日志中的 `app[0]` 表示当前镜像第零项。它既不携带名称，也不随应用独立存在，所以验收记录应同时附上脚本读出的目录项和应用返回值。若希望用同一份镜像复现交接，保留原始 ELF、镜像散列和构建配置，比重新打包同名文件更直接。

### 2.6.6 用 00hello 验证用户态写入

单独打包仓库里的 `00hello`。该应用依次经过启动、写入和退出。

```bash
make pack-apps APPS=00hello QSPI_IMAGE=output/ch2/hello.img
make qspi-write QSPI_IMAGE=output/ch2/hello.img
```

复位后预期看到如下日志。 `__kr_sys_write` 的长度参数是 14，包括 `Hello World!` 的 12 字节和 CRLF。写入分支返回后应用继续运行，内核要等退出才轮询发送这些字节。

```text
kr-stm-os booted
Hello World!
app[0] exited: 47
Batch complete
```

`01store_fault` 会先尝试把两条日志入队，但随后的非法写入可能在排空队列前触发 Fault。队列中有字节并不保证已发送，源码末尾的 `57` 也只有在函数实际返回时才会成为退出码。

### 2.6.7 按最早失败的检查点定位

把成功条件逐一对照。能读回文件但 CPU 还没到 `SVCall`，先从镜像解析、段加载到入口确认。能执行应用代码但没有退出日志，先在 PSP 帧观察 `0xdf01`，再追踪 MSP 中的返回值。这样每次调整都有可复测的依据，避免同时更换串口连接、内核和镜像后无法归因。

| 现象 | 优先检查 |
| --- | --- |
| 串口完全无输出 | 核对电平、接线和参数，再断到内核 `main` 或 `Board::init`。确认片内固件和启动配置，观察是否跑到向控制台提交启动文本的位置 |
| 启动后出现 `KAPP error: NoImage` | 头部四字节被读成全 `0xff`。确认烧录和内核都从偏移零读，比较读回文件开头，核对 NOR 是否擦除或 QSPI 读路径是否返回了全高电平 |
| `InvalidHeader` 或 `BadCrc` | 前者先核对魔数、版本、总长度和表的起点；后者核对同一份镜像的 `total_len`、CRC 字段置零规则及整段读回。若编程器读回正确而内核给出 `BadCrc`，分别检查 loader 与固件板级 QSPI 读路径 |
| `InvalidApp`、`InvalidSegment` 或 `InvalidEntry` | 用错误携带的索引找表项，检查名称、段表、源和目的范围、段重叠与入口标记。CRC 正确仍可能遇到语义错误，还需区分整份镜像已打开与某项已经逐段校验 |
| 启动 SVC 发出却没有命中 `SVCall` | 核对正在运行的固件和断点符号是否来自同一 ELF；读当前 `VTOR` 和向量 11；再查异常优先级或屏蔽状态以及是否先进入 Fault |
| 首次异常返回后立刻 Fault | 关注 `EXC_RETURN`、PSP 对齐、初始 PC 及 `xPSR.T`。若确认帧已正确弹出再查入口处装载字节是否和 ELF 一致 |
| 从应用进 `SVCall` 后跳到 `udf #0` | 读取入口 LR 判定帧类型，基本 PSP 帧下再读 `PC-2` 指令。基本 PSP 帧中的 `0xdf02` 应走写入分支，未知编号或扩展帧走未支持路径 |
| 第一项退出正确，第二项重复第一项或挂住 | 在第二次加载后比较真正存在差异的应用代码字节，核对表项和段偏移；若内核未进入第二次装载，检查恢复后的 PC、MSP 以及串口发送是否真正完成 |

如果出现 Fault，在已连接的调试器上记录 `HFSR`（`0xe000ed2c`）及 `CFSR`（`0xe000ed28`），注意故障状态位可能粘滞，独立实验用复位建立干净起点。有的后端支持 Fault vector catch，也可以在当前 ELF 里存在的 `HardFault` 或 `DefaultHandler` 符号上设置断点。`HFSR.FORCED` 表示故障升级时，继续读 `CFSR` 找根因。只有在 `BFARVALID` 或 `MMARVALID` 置位时，分别使用 `BFAR` 或 `MMFAR` 中的地址。若故障发生在 SVC Handler 中执行 `udf`，Fault 这一次的帧和应用进入 SVC 时保存的 PSP 帧是两个层次。开头的寄存器快照和异常返回值有助于避免把 Handler 自己的故障 PC 当成用户入口。 [应用启动与退出指南](svc-app-start-exit.md#6-在调试器里逐站验证) · [Arm Cortex-M7 异常模型](https://support.arm.com/documentation/dui0646/a/The-Cortex-M7-Processor/Exception-model/Exception-entry-and-return)

完成一轮实验后，记录仓库版本与工作树状态、构建 profile、三个镜像的顺序和 CRC、板卡与 loader 配置、串口输出以及两次 SVC 的核心值。还要记下验证停在哪一层。主机工具可以证明应用 ELF 与 KAPP 一致，串口可以观察整条路径，读回能确认 flash 字节，调试断点能核对运行上下文。缺少板卡或探针时，前两小节和以下主机练习可以独立进行，结果写成主机构建与文件校验即可。

## 2.7 练习与参考答案

建议先保留实验中三份镜像的配对结果，再动手解题。前几题沿着本章的有效路径做推理，后几题研究错误检测、平台差异以及下一步的系统调用设计。题目使用 Cortex-M7 的寄存器和本项目 KAPP 约定。rCore 原章练习可继续作为 RISC-V 对照，尤其适合对比栈切换、保存上下文与系统调用边界。 [rCore 第二章练习](https://rcore-os.cn/rCore-Tutorial-Book-v3/chapter2/5exercise.html) · [rCore 第二章练习参考答案](https://rcore-os.cn/rCore-Tutorial-Book-v3/chapter2/6answer.html)

### 2.7.1 题一　从一个段推算读写区间

设 KAPP 的一个段记录为 `dst_addr = 0x24061000`、`payload_offset = 0x78`、`file_len = 300`、`mem_len = 320`。装载器每次最多从镜像取 128 字节。计算三轮读取的起始偏移、长度和 SRAM 写入地址。完成文件数据搬运后，哪些地址需要清零？Flash 驱动一次最多读 32 字节，这与装载器的三轮循环如何对应？

**参考答案。** `300 = 128 + 128 + 44`，装载器维护 `copied` 来表示前面已搬运的字节数。三轮的输入偏移依次为 `0x78`、`0xf8`、`0x178`，输出地址依次为 `0x24061000`、`0x24061080`、`0x24061100`。第三轮送来 44 字节，直到目的地址的排他上界 `0x2406112c`。内存长度 320，也就是 `0x140`，因此余下的 20 字节位于 `[0x2406112c, 0x24061140)`。将这一段清零可以建立 ELF 对零初始化存储的语义，使先前应用写过的 SRAM 不会影响当前段的初值。

驱动会对每个装载器请求继续分片。两个 128 字节请求分别拆成四次 32 字节，最后 44 字节拆成 32 和 12。在该段共有十次普通读数据事务。第一层分块限制装载器的临时栈缓冲区，第二层服从 HAL 的事务长度限制。这个例子只计算装入某个段时的数据读，镜像头、目录、段表与 CRC 的预读也需要 NOR 访问。若日志按驱动次数计数，应从断点确定统计边界。 [段加载](../kernel/src/batch/loader.rs) · [QSPI 读取](../kernel/src/drivers/qspi.rs)

### 2.7.2 题二　说明两个 `r0` 和两个 LR

第一个应用返回 `42`。分别写出第一次与第二次进入 `SVCall` 的 LR 值、基本帧的位置及其中 `r0` 的含义。再解释退出分支恢复 `r3–r11, lr` 后，为什么必须把退出码存入 `[sp]`。假如只将退出码留在活跃寄存器 `r0`，回到 Rust 时会收到什么？

**参考答案。** 启动请求发生在 MSP 上的特权 Thread。`__kr_run_app` 按调用约定用 `r0` 携带应用初始帧指针，因而 MSP 基本帧的 `r0` 槽中最初是 `0x2407ffe0`，Handler 入门 LR 指示 `0xfffffff9`。退出请求发生在 PSP 上的应用 Thread。`svc #1` 的调用者用 `r0` 携带 `42`，所以 PSP 基本帧的首字为 `0x2a`，入口 LR 指示 `0xfffffffd`。两份硬件帧里的 LR 仍保留各自被打断代码的普通返回地址。

从 MSP 弹出软件保存区后，SP 恰好指回内核基本帧。退出分支将值存入这个帧的 `r0` 槽，CPU 因而会在异常返回时把 `42` 弹入 `r0`。若只保存在寄存器里，异常返回会取帧中原有的指针覆盖它。`__kr_run_app` 接着把恢复后的 `r0` 作为 `i32` 交还 Rust，日志可能打印对应栈地址的整数值。把这个异常现象与 `0x2407ffe0` 换算的数值对照，便能检验是否漏掉了写回。 [异常帧构造](../kernel/src/trap.rs) · [退出恢复](../kernel/src/trap/svc.s)

### 2.7.3 题三　跨平台保存现场的理由

rCore 章节使用 `__alltraps` 保存程序现场，用 `__restore` 进入用户态或恢复用户态。这里首次启动使用合成基本帧，应用退出回到旧内核。分别指出谁保存通用寄存器、谁选返回栈，以及两种设计中什么状态必须留到下一次返回。为什么 Cortex-M7 的例子不把所有寄存器又复制一遍？如果增加 `write` 并恢复同一个应用，应用的 `r4–r11` 又该怎么办？

**参考答案。** RISC-V 上 `ecall` 进入 trap 后，硬件更新异常相关 CSR，软件负责将需要的通用寄存器放到约定的 trap 上下文，并安排下一次 `sret` 使用的 PC 和权限。Cortex-M7 基本异常进入直接把八个字压到被打断线程所用的栈，内核使用被选中的 `EXC_RETURN` 指定从 MSP 或 PSP 取帧。启动应用时，内核基本帧已经保存易失寄存器、普通 LR、PC 与 xPSR。软件另外留住内核的被调用者保存寄存器和恢复用令牌，才能在应用有机会改写活跃寄存器后继续原来的 Rust 调用。硬件已经保留的槽直接复用，其余按切换边界实际所需保存。

如果写请求处理完要继续原应用，应用同一份 PSP 基本帧仍在那里。汇编入口的临时寄存器使用应局限在基本帧覆盖的调用者保存寄存器；调用符合 ARM ABI 的 Rust 函数时，被调用者负责保护其使用到的 `r4–r11`。入口汇编自己若使用这些寄存器，应成对保存和恢复。运行应用时预留的整套内核 MSP 上下文需要继续保留，因为写调用返回时还没有结束这次装载。推导保存集合时，先问异常结束后会继续哪个上下文，再找出这条路径实际修改了哪些还要用的值。 [本项目的 SVC 路径](../kernel/src/trap/svc.s) · [rCore 的 Trap 处理](https://rcore-os.cn/rCore-Tutorial-Book-v3/chapter2/4trap-handling.html)

### 2.7.4 题四　损坏检测和语义检查有何分工

准备两个主机上的变体。第一个翻转 `single.img` 的最后一个 payload 字节，保留旧 CRC。第二个把首个段的目的地址设为 `0x2405fffc`，并重新计算 CRC。只在电脑内存中构造变体即可。预测这两份数据分别通过哪些检查，哪里会返回错误。说明为何同一份镜像返回 `BadCrc` 和 `InvalidSegment(0, 0)` 时应采取不同的排查步骤。

**参考答案。** 文件结尾的字节参与 CRC。只变更它而保留原 CRC，不影响头和目录的结构，内核应在 `Image::open()` 进行整份镜像检查时返回 `BadCrc`。第二个例子的目录、表偏移和载荷都还有原结构，重算 CRC 后可以通过 `Image::open()`。当 `loader::load()` 按索引请求应用时，`validate_app()` 检查段目标起点小于 `APP_START`，于是返回 `InvalidSegment(0, 0)`。错误在段数据写入之前就被识别。只做 CRC 会接受第二种数据，让段目标进入内核保留区。只做范围检查则可能接受第一种数据，让篡改后的字节参与执行。CRC 为文件传输和存储提供检错，装载器还需要对地址和长度施加结构约束。

在主机上运行下段代码即可观察 CRC 层的差别。代码只读原文件，两个变体保留在 Python 对象中。这项实验验证本地文件变化和 CRC 的关系，内核错误类型仍由源码推导。

```bash
python3 - "$lab_dir/single.img" <<'PY'
from pathlib import Path
from struct import pack_into, unpack_from
import sys
import zlib

original = Path(sys.argv[1]).read_bytes()

def checksum(image):
    return zlib.crc32(image[:20] + bytes(4) + image[24:])

def stored(image):
    return unpack_from("<I", image, 20)[0]

assert original[:4] == b"KAPP"
assert stored(original) == checksum(original)

changed_payload = bytearray(original)
changed_payload[-1] ^= 1
assert stored(changed_payload) != checksum(changed_payload)

changed_destination = bytearray(original)
app_table = unpack_from("<I", original, 16)[0]
first_segment = unpack_from("<I", original, app_table + 24)[0]
pack_into("<I", changed_destination, first_segment, 0x2405fffc)
pack_into("<I", changed_destination, 20, checksum(changed_destination))
assert stored(changed_destination) == checksum(changed_destination)
assert unpack_from("<I", changed_destination, first_segment)[0] < 0x24060000
print("payload: CRC mismatch; segment destination: CRC valid, address invalid")
PY
```

在调试中，`BadCrc` 应从写入与读回的字节是否一致查起。`InvalidSegment(0, 0)` 则按索引查看第一条段记录，结合 ELF 的 `p_vaddr` 和编解码结果，核对是地址还是长度违反了约束。比如完整段表的字节数来自每个应用的段数，若主机脚本误把应用表大小当作段记录大小，打印出来的数值会失去意义。这道题所选的字段位置同时考察偏移与小端格式。 [KAPP 校验与错误类型](../kernel/src/batch/image.rs)

### 2.7.5 题五　为 `sys_write` 设计边界检查

阅读已经实现的 `svc #2`。解释应用帧里的两个参数、检查顺序和返回值。再说明如果要把范围收紧为逐段可读区间，需要哪些额外元数据。

**参考答案。** 首先保留已验证的启动和退出分支。仅对当前正在运行的应用、基本 PSP 帧以及确实执行 `svc #2` 的调用分派写入，入口使用可信的加载元数据来识别正在运行的应用。保存 Handler 自己要用的 `EXC_RETURN`，再用符合 ABI 的普通调用边界进入 Rust 帮助函数。基本帧首字是 `r0 = ptr`，第二字是 `r1 = len`。处理写入时保留原来的应用 PSP 帧，完成后把已接收的字节数或错误码写回这份帧的 `r0`，按原 PSP 返回令牌恢复同一应用。写服务返回到 `svc` 的下一条指令，内核挂起的 MSP 上下文仍等应用主动退出时使用。

当前实现按零长度、128 字节上限、地址范围、队列容量的次序检查。成功返回整段字节数，三类失败分别返回 `-90`、`-14` 或 `-11`。排他末地址等于栈顶时可接受。

当前只检查整块应用 SRAM 的数值范围，并依赖应用传入已初始化字节。若要增加逐段权限，需要保存已验证的段记录和用户栈边界，再排除未授权的段间空洞。 `try_write(&[u8])` 已接受原始字节，队列保有独立副本。

当前写入服务只复制入队，批处理在应用退出后使用轮询发送。以 19200 baud、8N1 计算，128 字节的线上传输约需 67 ms。若需要应用运行期间持续输出，可以另外设计发送推进机制。

下表按整块应用 SRAM 的范围列出用例。应对成功入队的非空请求使用已初始化的缓冲区，也可以在主机纯函数测试中验证数值边界。

| 输入或前置条件 | 预期结果和原因 |
| --- | --- |
| `ptr = 0`、`len = 0` | 返回 `0`，不进行地址访问或入队 |
| `ptr = 0x24061000`、`len = 3` | 返回 `3`，队尾恰好新增指定的三个字节 |
| `ptr = 0x2407ffff`、`len = 1` | 返回 `1`，排他上界可与区域末尾相等 |
| `ptr = 0x2407ffff`、`len = 2` | 范围错误，队列原样保留，整个请求越过末尾 |
| `ptr = 0xfffffffc`、`len = 8` | 范围错误，按目标 32 位地址模型加法溢出；主机测试可直接用 `u32` 模拟 |
| `ptr = 0x2405fffc`、`len = 4` | 范围错误，内核 SRAM 不属于本应用 |
| `ptr` 落在应用 SRAM 内的段间空洞 | 数值范围检查通过，仍需满足字节已初始化的前提 |
| `len = 129` | 长度错误，超过所约定的单次上限 |
| 合法缓冲区，队列只剩两个字节且 `len = 3` | 队列满错误，原有数据和剩余容量均保持原样 |
| 合法缓冲区包含 `0x00` 与 `0xff` | 在原始字节 ABI 下按字节接受，与传入顺序一致 |

连接实板后，再用一个受信任的应用先写短消息，再依据返回值选用不同退出码。断点应依次遇到启动、写入、退出，`svc #2` 后的应用 PC 和 PSP 应恢复，最终 `svc #1` 仍能把退出码传回内核。可以让应用调用后立刻改写自己的缓冲区，观察串口是否仍发出入队前的数据，以检验队列持有独立副本。对跨界和溢出等参数，主机纯函数测试可以验证拒绝路径，板上测试可由专用测试入口传入整数参数，避免 Rust 先尝试构造无效引用。针对 Handler 返回路径，应再在用户代码中加入能够检查活跃寄存器的受控测试，确保跨 `svc #2` 保留 ABI 承诺的上下文。

这些约束防止写服务代为读取未获准的内存。应用还可自行执行 load 和 store，因此 MPU 需要另外配置区域访问权限，保护内核 RAM、控制寄存器和设备，把批处理进程可直接访问的区域限制在它拥有的内存。等 MPU 和 Fault 策略具备以后，再验证访问违规的应用能否被终止并交回调度。只用退出码 `42` 验证的当前阶段，应用使用非特权 PSP 的事实可由断点证实，内存访问隔离和 Fault 恢复仍是后续工作。 [应用启动与退出指南](svc-app-start-exit.md) · [MPU 与 Fault 的实施顺序](batch-implementation.md#54-irqfaultmpu-和抢占的先后)

### 2.7.6 题六　提交可复现的实验记录

选择一次运行写出尽量短的实验记录。给出应用构建的 profile、打包顺序、镜像长度和 CRC、读回比较结果、串口输出、关键断点两次命中的寄存器与帧内容。说明观察到的是成功结束、镜像错误还是 CPU 故障。在只有电脑的情况下，附三份 KAPP 摘要和一个主机错例的推导，写清哪些数据还需要上板确认。把寄存器值和源码动作连起来，避免仅抄一串十六进制数。

**参考答案。** 单应用、双应用和倒序镜像分别记录 `00exit42`、`00exit42 → 02exit7`、`02exit7 → 00exit42`。对每个镜像同时保存源文件和读回文件的长度与比较结果，附对应的 ELF `LOAD` 记录。若单应用先后见到 `0xfffffff9` 与 `0xfffffffd`，可说明第一次是从 MSP 进入正规启动入口，第二次是应用用 PSP 发起 SVC。再用应用帧中的 `0xdf01` 与 `r0 = 42` 指出具体请求，最后以恢复点的 `r0 = 42` 验证退出结果真正送达内核。若只见 `Done!`，报告里还要给出前面的输出。`Batch complete` 说明批处理循环正常结束，`KAPP error` 则说明内核走了失败分支。板上没有采到输出时，只提交实际得到的离线证据与推导，待测栏写明对应的硬件检查点。这样的记录能够让下一位读者使用相同镜像复现现象，并直接从最早分叉的位置继续排查。

## 2.8 源码索引与实现边界

### 2.8.1 从一条日志逆向查到所属模块

本章的入口写法刻意把板级启动、批处理决策和异常切换分开。`Board::init()` 结束后，调用方手里已有 `flash` 与 `console`，`batch::run()` 不负责决定这些外设使用哪个引脚或时钟频率。这样在应用装载过程中看到外设错误，可以沿 `ImageError::Flash` 找到读驱动，再沿 `Board` 找到硬件配置。另一个方向是看到应用退出消息时，先到 `batch.rs` 找格式化位置，然后逆向追到 `trap::run_app()` 的返回值。把日志当成源码中的观测点，调试结果就可以直接定位到模块。 [启动职责与代码索引](architecture.md)

| 串口或调试器现象 | 先定位到的源码 | 此处已经完成的事 |
| --- | --- | --- |
| `kr-stm-os booted` | `kernel/src/main.rs` | 内核入口已运行，控制台开始发送启动消息 |
| `KAPP error: NoImage` | `kernel/src/batch/image.rs` | 已读取 Flash 镜像头部，前四字节都是擦除态 |
| `KAPP error: BadCrc` | 同一文件的 `verify_crc()` | 基本头和目录检查已走过，整镜像复算不一致 |
| 应用地址附近的断点命中 | `kernel/src/trap/svc.s`、`user/src/runtime.rs` | 装载与首次异常返回已推进到应用入口 |
| `app[0] exited: 42` | `kernel/src/batch.rs` | 应用正常发出退出，内核现场已恢复 |
| `Batch complete` 后显示 `Done!` | `kernel/src/main.rs`、`kernel/src/panic_handler.rs` | 本轮迭代结束，入口进入已有的 DMA panic 演示 |

每行都有前置条件。例如 `BadCrc` 比 `NoImage` 更晚出现，但还没到装载任何应用。退出消息则不可能来自测试应用里的 `println!`，因为本章测试应用根本没有打印操作。它是内核格式化的结果。排查串口问题时，判断这些消息各由谁发出，比只看终端最后一行更可靠。

遇到没有后续日志的情况，也可以先给自己划一条时序边界。先记录最后一条肯定正确的输出，再找下一处代码必然执行的观测点。若上一条是内核启动提示，下一处可以是最早读取镜像头的函数。若已经看到应用入口，下一处可以是 SVC 向量。若从 SVC 入口可看到 `42`，下一处就应是内核保存帧被修改的指令。按这样的顺序加断点，不会把“设备读失败”“镜像不合规”和“异常返回去了错误的地方”混在一起。 [实板断点与故障表](svc-app-start-exit.md#6-在调试器里逐站验证)

### 2.8.2 从地址与所有权理解这些 unsafe

装载器中的 `unsafe` 把镜像记录中的整数解释为内存地址。Rust 能检查本地缓冲区下标，并保证 `copy_nonoverlapping()` 调用时的类型正确，但整数有多少字节对应有效 SRAM、这段内存是否仍由前一个应用使用，要由调用关系来保证。镜像验证给出来源范围、目的区间、段不重叠等条件。批处理循环给出时间条件，即只有当旧应用的运行已经结束，才会开始新一轮装载。地址正确和使用时机正确是两条分开的证据。

可以用两个反例理解为什么要分两步。假设镜像里的目标地址落在应用区内，但是第一个应用仍在执行该区域的指令。即使每次字节复制都落在合法地址，复制仍可能覆盖当前 CPU 即将执行的机器码。另一个方向是第一个应用已经退出，第二个应用的镜像却声称一个段从应用区中段一直延伸到内核或栈边界之外。此时顺序没问题，但必须拒绝目的范围。让 `validate_app()` 只处理空间约束，让 `batch::run()` 管理运行次序，便于分别解释这两个条件。

`trap::run_app()` 也有对应的前提。其输入 `entry` 和 `stack_top` 都是数值，并在 Rust 与汇编之间传递。调用前应知道它们属于当前装载成功的那个应用。调用的内核线程也应处在特权 Thread mode，使用 MSP，并允许 SVC 被正常接收。若从错误的 Handler 上下文重入，就算入口地址和栈地址都正确，也无法复用这一版的现场保存路径。将这些前提写在 `Safety` 注释里，目的是让未来接入者知道调用约束跨越了 Rust 类型系统、硬件模式和生命周期三层。 [加载器调用点](../kernel/src/batch.rs#L47-L66) · [切换函数契约](../kernel/src/trap.rs#L19-L42)

应用发起系统调用时，汇编读到的每个寄存器仍然是应用给出的值。对退出码来说，一个任意 32 位整数就可以作为结果。对 `write` 来说，同样一个整数可能被解释为指针，错误就会变成对真实内存的访问。签名里的 `usize` 只提供位宽。`__kr_sys_write` 先按整数检查边界，再构造临时切片供入队复制。把验证放在转换之前，是因为验证失败时内核根本没有理由触碰这块地址。

### 2.8.3 文件格式、硬件保护与故障恢复分别验收

镜像处理层能回答的问题是“送进来的一串字节是否符合布局”。它会检查 magic、版本、长度、CRC、名称以及段边界。这些检查足以让加载器拒绝多数格式错误，但镜像只要包含合法的 ARM 指令与正确的入口，就可能带有一个无限循环。把这种应用写入 Flash 不会令 CRC 自动失败。CRC 依赖数据内容，无法替内核决定程序将来怎么执行。

特权层回答“这时 CPU 允许以什么权限执行”。非特权 Thread mode、PSP 与明确的 SVC 入口建立了普通应用和内核服务的边界。MPU 的作用是进一步限制应用能访问的具体内存与设备。装载时的区域划分同时是未来配置 MPU 时的基础。应用代码和数据是否用不同权限，要取决于最终采用怎样的区域划分、对齐，以及它是否真的覆盖链接脚本安排的地址。验收时应同时检查链接地址、实际 MPU 寄存器值和出错现场，不能由一个预想中的 Fault 就推断权限表已正确应用。

故障恢复层回答“这次异常结束后还能继续哪个程序”。即使能检测到应用触发了访存错误，也要先看异常来源和现场是否可信。如果错误发生于应用运行时，而且有可靠的 PSP 帧，可以设计恢复 MSP 上的内核现场，由批处理记录结束原因。如果错误发生于内核或异常栈压入过程，现场可能已损坏，就需要更加保守的诊断流程。当前仓库的 `SVCall` 负责可预期的启动、写入与退出。把应用故障恢复列为单独工作，有助于保留现在能验证的成功路径，并为新异常找到可观测的验收标准。 [后续 MPU 与 Fault 规划](batch-implementation.md#54-irqfaultmpu-和抢占的先后)

### 2.8.4 同一个应用槽要怎样扩展

一开始选择让所有应用运行在同一片 AXI SRAM，避免了在运行时重定位，也避免了同时维护多个用户栈。镜像中仍能放许多程序，只要它们依次使用这个槽。等待退出的程序完成一项任务后，内核就可以覆盖地址。若当前应用陷入无限循环，外层迭代回不来，这是运行方式的直接结果。要支持这样的应用，就需要另一个可获得控制权的路径，例如定时器中断和后续的抢占逻辑。

从批处理扩展到可暂停的任务时，“退出后丢弃现场”的假设会变化。暂停的程序以后还要恢复所有继续计算所需的寄存器和栈。它原来正在使用的代码与可写数据也不能被下一项覆盖。后续可以先安排多块应用内存，或者设计把不活跃任务的状态保存到其他位置。无论用哪种方法，都要先说明内存中同时存在几个任务，它们的入口和用户栈如何分配，调度器记录什么信息，以及谁有权修改各自的区域。否则，把 `run_app()` 的返回条件改为“任意一次 SVC”无法建立完整的任务切换。

退出恢复 MSP 上的内核帧，写入恢复 PSP 上的应用帧。批处理等应用退出后排空队列，DMA panic 使用独立发送路径。继续用两种退出码回归，可确认修改写入服务时批处理仍能推进。 [控制台](../kernel/src/drivers/console.rs) · [DMA panic](dma-panic.md)

### 2.8.5 如何记录一次可复查的实验

实验记录应同时描述输入和观测结果。记录固件源码版本、目标名、构建配置、应用 ELF 文件名和打包参数。单应用镜像测试时写明只有哪个应用，双应用测试时写明确切顺序，必要时附上镜像尺寸与校验结果。这样串口显示 42 与 7 时，另一位读者能够从镜像输入重现顺序，而不用猜测机器里还留着哪次的烧录产物。若保留两份镜像作对照，文件名可以直接包含它们对应的单应用、正序和倒序用途。

对寄存器截图也应注明断点所在指令。写“在 `SVCall` 看到 PSP 为某值”不够，因为 handler 会命中两次，而且启动分支会主动修改 PSP。写“首次进入 `SVCall`、执行第一条指令前”和“启动分支 `bx lr` 之前”，就能精确说明当时保存的是哪一方。记下每处的 LR、PC、MSP、PSP，以及感兴趣的帧槽。把屏幕上的地址换算成帧起点加偏移，便于别人即使使用不同的栈地址，也能核对相同的逻辑。

主机构建验证和板上观测按各自证据记录。主机能确认 ELF 入口、段属性、符号和镜像次序，板上调试能确认实际 Flash、SRAM 和 CPU 模式。未烧录时，示例串口文本是验收预期。只要报告清楚已经做到的层次，就可以在下一轮专门追踪缺少证据的部分，避免把“构建成功”误当成“已经验证异常返回”。

### 2.8.6 阅读资料

- 原书第二章的引言、特权级、应用、批处理、异常处理、练习和参考答案为本章提供教学次序。 [rCore 教程第二章](https://rcore-os.cn/rCore-Tutorial-Book-v3/chapter2/index.html)
- Cortex-M7 手册列出异常入口、基本与扩展帧、异常返回与所用栈。阅读时可以用本章的寄存器轨迹核对手册表格。 [Arm Cortex-M7 异常进入与返回](https://support.arm.com/documentation/dui0646/a/The-Cortex-M7-Processor/Exception-model/Exception-entry-and-return)
- RISC-V 规范可用于区分 `ecall` 的硬件动作与项目自己通过 `a7` 传编号的软件约定。 [RISC-V 特权架构](https://docs.riscv.org/reference/isa/priv/machine.html) · [RISC-V ABI 寄存器约定](https://riscv-non-isa.github.io/riscv-elf-psabi-doc/)
- 本仓库独立保留了板级接线、Flash 装载和 SVC 实现步骤。遇到需要逐文件复现或切换编程器的操作，从这些指南进入。 [批处理系统实施指南](batch-implementation.md) · [应用启动与退出实现指南](svc-app-start-exit.md)
