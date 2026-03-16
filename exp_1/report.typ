#import "../assets/hdu-report-typst/template/template.typ": *

#show: project.with(
  title: [
    杭州电子科技大学\
    《操作系统课程实践》\
    实验报告\
  ],
  subtitle: [],
  class: "计算机科学英才班",
  department: "卓越学院",
  authors: "鲍溶",
  author_id: "23060827",
  date: datetime.today(),
  cover_style: "hdu_report",
)

#show link: underline

#let paragraph(title, body) = [
  #strong[#title] #h(1em) #body
]

#toc()

#pagebreak()

= 实验目的

1. 了解 Linux 操作系统的体系结构和运行机制。
2. 掌握内核源码的获取、配置、编译流程，并能够替换操作系统运行自己的内核。
3. 深入理解 Linux 系统调用的工作原理，掌握向 Linux 内核添加自定义系统调用的步骤及相关数据结构的修改方法。
4. 熟悉 Linux 环境下的 C 语言编程思维以及用户态与内核态数据交互的方法。

= 实验内容

本实验旨在熟悉 Linux 内核编译过程，并在内核中新增一个自定义系统调用。

实验首先要求从官方维护的仓库获取 Linux 源码，在本地环境中配置编译选项，对源码进行完整编译，并将新编译的内核安装、引导到现有系统中，确保机器可以正常启动并运行新的内核。新的内核中需包含自定义输出以表明替换成功。

之后，实验要求在内核中添加一个新的系统调用，实现自定义功能。需要为新的系统调用分配系统调用号、在系统调用表中注册、在内核中编写具体的代码逻辑等。

本实验选择实现“改变和获取主机名称为自定义字符串的系统调用”一条，通过用户提供的缓冲区与标记实现对主机名的查询与修改。

= 实验方法

#paragraph([环境准备])[
  使用 Git 克隆获取 Linux 源码树，并安装构建内核所需的所有依赖。
]

#paragraph([内核编译与替换])[
  在克隆的 Linux 源码树下使用 menuconfig 等工具修改内核配置，在版本号或启动序列中加入标识字符串，之后进行编译。替换虚拟环境使用的内核并尝试启动，观察启动时 dmesg 输出，确认替换成功。
]

#paragraph([添加系统调用])[
  在内核相关记录结构中注册新的系统调用号、系统调用名、处理函数等，保证不与当前已有系统调用相冲突。编写处理函数的具体实现，使用内核提供的接口完成对主机名的查询、修改等工作，并正确与用户传入的参数交互、正确进行错误处理。
]

#paragraph([验证])[
  重新替换内核，编写用户程序调用添加的系统调用。若观察到主机名查询成功、主机名修改成功，且系统调用能够在参数不合法、用户无权限时恰当进行错误处理，则实验成功。若观察到系统调用不存在、内核崩溃等异常现象，需重新检查并修正实现。
]

= 实验过程

== 内核编译环境准备与文件系统构建

本实验选取 Linux 6.19.6 作为基线版本。使用 Git 克隆获取 Linux 源码，在源码根下使用 `make menuconfig` 进入图形化界面，通过配置“General setup -> Local version”选项加入版本标识符 `-hdu`，以与上游版本区分。

为了保障隔离性并快速验证自定义代码，本实验使用 QEMU 作为验证环境。其支持使用独立的内核文件、bootloader、rootfs 直接启动虚拟机@qemu2026direct，无需预先构建完整的磁盘镜像，极大加速了测试迭代。`alpine-make-rootfs` 工具能够构建精简基础的 Alpine Linux rootfs，将其导入一个本地的裸磁盘镜像后即可作为 QEMU 的 rootfs 镜像使用。

== 自定义系统调用注册与实现

本实验中添加的系统调用名称为 `csm_hostname`，系统调用号为 1024，内核处理函数为 `sys_csm_hostname`。其接受三个参数：行为标志 `is_set`，用户态字符序列指针 `name` 与操作数据长度 `len`。根据 `is_set` 的值，或将主机名复制至 `name[len]` 指定的用户空间，或从用户空间读取、设置新主机名。

向 Linux 内核成功集成自定义行为需注册系统调用并向下暴露接口。Linux 6.11 引入了一套更平台无关的系统调用添加流程@kernel2026adding。首先，需要在通用系统调用表 `scripts/syscall.tbl` 中，分配并登记调用号 `1024`，并将其绑定至内核态处理函数 `sys_csm_hostname`。接着，在 `include/linux/syscalls.h` 中声明该调用的内核函数原型。最后在 `kernel/sys_ni.c` 中通过 `COND_SYSCALL` 注册了它的占位定义，满足兼容性设计逻辑以应对功能缺失时的正常降级。

参考现有的 sethostname(2)，选择在 `kernel/sys.c` 文件中实现 `sys_csm_hostname`，并与 sethostname(2) 采用相似的权限校验与并发控制机制。

Linux 2.6.19 引入了 UTS（Unix 分时系统）命名空间管理宿主机与其上运行容器的主机名@kerrisk2013namespaces。同样参考 sethostname(2) 实现对该结构的读取与写入，在实现过程中注意使用 `uts_sem` 锁管理并发访问。对用户空间的读取与写入使用 `copy_{from,to}_user` 实现。

== 系统启动与验证调试

在完成源码更改后，借助跨平台构建指令对内核重新开展一轮编译，随后可由 QEMU 直接加载所产出的 LoongArch64 架构 EFI 启动镜像 `vmlinuz.efi` 用于测验。构建具备 KVM 硬件虚拟化特性的命令行交互环境时，需使用指定模拟架构参数，挂载先前生成的根文件系统设备。针对此配置，详细调试挂载环境搭建命令如下@code:qemu-boot 所示。

#code(
  ```sh
  qemu-system-loongarch64 -accel kvm -machine virt -cpu la464 -m 1G -bios /usr/share/edk2/loongarch64/QEMU_EFI.fd -kernel ~/dist/linux-arch/arch/loongarch/boot/vmlinuz.efi -drive file=~/alpine-rootfs-qemu.img,format=raw,if=virtio -append 'loglevel=7 root=/dev/vda rw console=ttyS0,115200 nomodeset init=/sbin/init' -nographic
  ```,
  caption: [QEMU 虚拟环境启动命令],
) <code:qemu-boot>

成功进入 shell 后，fastfetch(1) 显示运行的内核确实为实验中构建的版本，如@figure:fastfetch。

#img(
  image("assets/fastfetch.png"),
  caption: [检查运行内核版本]
) <figure:fastfetch>

为了验证新系统调用 `csm_hostname` 的鲁棒性和业务表现，在终端中借助自行编写的 simplecall(1) 工具进行针对性调用测试。simplecall(1) 是一个命令行工具，可以根据提供的参数发起系统调用，并将结果使用十六进制打印至终端。如@figure:verif 所示，首先使用命令 `simplecall 1024 u8:0 o:64 u8:64` 传入 `is_set = 0` 及 64 字节长度的输出缓冲区读取当前主机名。接着，使用命令 `simplecall 1024 u8:1 s:new-name u8:9` 发起修改操作，传递新的主机名称字符串 `new-name`。操作完毕，后不仅能通过系统自带的 hostname(1) 观察到主机名已正确修改，当前的终端环境提示符也随着主机名同步更新为 `new-name:~#`。

#img(
  image("assets/verification.png"),
  caption: [验证系统调用运行情况]
) <figure:verif>

= 结果分析与实验体会

通过本次操作系统的系统调用添加和内核编译实验，我掌握了 Linux 操作系统的基础构建方式，体会到内核开发的严谨性：当数据需要在用户态和内核态之间交换时，不能直接使用普通的指针赋值，而必须依赖特定函数，如 `copy_{from,to}_user`；在修改敏感的全局系统信息时，不仅需要判断当前程序的角色权限避免安全漏洞越权操作，更改过程还需要考虑共享访问的保护。本次实验加深了我对课程中有关操作系统边界控制、同步与互斥等理论课程内容的理解。

= 源代码

本报告源文件可以从 #link("https://github.com/CSharperMantle/hdu2026_os_exp/tree/main/exp_1") 处获取。

实验中使用的 Linux 内核源码树为 #link("https://github.com/CSharperMantle/linux-arch")。

实验中使用的 simplecall(1) 工具可从 #link("https://github.com/CSharperMantle/simplecall") 处获取。

#pagebreak()

#bibliography("bib.bib", style: "gb-7714-2015-numeric")
