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

+ 了解进程与子进程的概念，理解 Linux 系统中进程创建与程序替换的机制；
+ 掌握管道、FIFO、POSIX 消息队列、共享内存等 IPC 机制的原理。
+ 掌握利用 POSIX API 在用户态实现 IPC 的方法，理解不同 IPC 机制的适用场景与优缺点。

= 实验内容

本实验旨在深入理解 Linux 操作系统的 IPC 机制，并综合运用多种 IPC 原语构建多进程应用程序。

实验的前半部分要求实现一个支持管道（`|` 操作）与输入输出重定向（如 `>`、`<`、`>>`、`fd>&fd` 等操作）的简单 Shell 模拟程序。该程序需具备命令行解析能力，能够识别管道操作符并正确建立输入输出管道，对各子进程进行正确的文件描述符重定向，为后续步骤提供基础。实验的后半部分要求分别使用 FIFO、POSIX 消息队列和 POSIX 共享内存实现多进程间通信。

= 实验方法

#paragraph([环境准备与项目架构])[
  搭建基于 Zig 语言的项目框架，利用其原生安全特性与对 POSIX API 的良好支持。设计清晰的模块划分：命令行解析器负责处理管道与重定向语法；IPC 公共模块定义统一的帧协议格式；各 IPC 实现模块（FIFO、MQ、SHMEM）分别封装对应系统调用的细节。项目采用 Makefile 或 Zig 原生构建系统进行管理。
]

#paragraph([模拟 Shell 实现])[
  实现简单分词器处理用户输入，识别管道符、重定向操作符及文件描述符编号等语法单位。使用 `fork(2)` 为每个子命令创建子进程，通过 `pipe(2)` 建立双向通信通道，并使用 `dup2(2)` 将管道端映射至标准输入输出。使用 `execve(2)` 与其包装函数完成程序替换。内置命令（`cd`、`pwd`、`exit`）直接在父进程执行，不创建子进程。
]

#paragraph([多端中心化聊天室实现])[
  使用星型拓扑实现中心化聊天室，主机作为中心节点维护所有客户端的状态，客户端之间不直接通信。在 FIFO 实现中，主机创建控制管道监听所有客户端的接入请求，各客户端创建私有数据管道供主机广播消息；利用 `poll()` 系统调用检测管道可写性以识别僵尸客户端。MQ 实现中，主机持有接入队列 `/mychat-host`，各客户端持有私有接收队列 `/mychat-client-<pid>`；主机以阻塞方式等待消息，另有后台线程定期探测已退出的客户端。SHMEM 实现中，每个客户端与主机共享一段独立的内存区域，通过 POSIX 信号量 `sem_t` 实现生产者和消费者之间的同步与互斥；僵尸检测借助 `sem_timedwait(3)` 的超时机制实现。
]

#paragraph([双端点对点聊天室实现])[
  使用点对点拓扑实现一对一聊天，以达到任务书中使用 POSIX 共享内存实现双向通信的要求。使用读者--写者模式实现同步，两端互为对方的读者，共享同一个单缓冲；同时还需防止第三人加入对话。对话发起方首先通过 `shm_open(3)` 创建共享内存区段，初始化五个信号量：

  - $#raw("client_present") = 1$，确保同一时刻最多只有一个客户端接入；
  - $#raw("turnstile") = 1$，实现当前发送方的互斥访问；
  - $#raw("empty") = 1$，表示缓冲区空闲；
  - $#raw("full_host") = 0$、$#raw("full_client") = 0$，分别作为两端的接收就绪信号。

  服务端与客户端使用如@code:p2p-server-send、@code:p2p-client-send、@code:p2p-server-receive、@code:p2p-client-receive 所示算法进行资源共享。
]

#grid(
  columns: (1fr, 1fr),
  rows: 2,
  align: top,
  gutter: 1em,
  [
    #code(
      ```txt
      procedure server_send() {
        P(turnstile)
        P(empty)
          [ Write data to buffer ]
        V(full_client)
        V(turnstile)
      }
      ```,
      caption: [服务端发送函数]
    ) <code:p2p-server-send>
  ],
  [
    #code(
      ```txt
      procedure client_send() {
        P(turnstile)
        P(empty)
          [ Write data to buffer ]
        V(full_server)
        V(turnstile)
      }
      ```,
      caption: [客户端发送函数]
    ) <code:p2p-client-send>
  ],
  [
    #code(
      ```txt
      procedure server_receive() {
        P(full_server)
          [ Read data from buffer ]
        V(empty)
      }
      ```,
      caption: [服务端接收函数]
    ) <code:p2p-server-receive>
  ],
  [
    #code(
      ```txt
      procedure client_receive() {
        P(full_client)
          [ Read data from buffer ]
        V(empty)
      }
      ```,
      caption: [客户端接收函数]
    ) <code:p2p-client-receive>
  ],
)

#paragraph([功能验证])[
  分别在三种 IPC 模式下启动主机进程与多个客户端进程，观察客户端加入、消息广播、客户端退出的行为是否符合预期。对每种 IPC 机制进行故障测试，涉及客户端收到信号退出、消息过长、客户端重名等场景，以验证帧协议解析、僵尸回收、进程退出资源清理等功能的正确性。
]

= 实验过程

#lorem(50)

= 结果分析与实验体会

#lorem(50)

= 源代码

本报告源文件可以从 #link("https://github.com/CSharperMantle/hdu2026_os_exp/tree/main/exp_3") 处获取。

#pagebreak()

#bibliography("bib.bib", style: "gb-7714-2015-numeric")
