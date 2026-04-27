#import "../assets/hdu-report-typst/template/template.typ": *

#show: project.with(
  title: [
    杭州电子科技大学\
    《操作系统课程实践》\
    实验报告\
  ],
  subtitle: [实验5：简单文件系统设计与实现],
  class: "计算机科学英才班",
  department: "卓越学院",
  authors: "鲍溶",
  author_id: "23060827",
  date: datetime(year: 2026, month: 4, day: 26),
  cover_style: "hdu_report",
)

#show link: underline

#let paragraph(title, body) = [
  #strong[#title] #h(1em) #body
]

#toc()

#pagebreak()

= 实验目的

+ 理解 FAT 系列文件系统的设计原理，包括磁盘布局、FCB（文件控制块）结构、FAT 链管理方式等核心概念。
+ 掌握基本文件系统操作的实现方法：格式化、目录遍历、文件创建与删除、文件顺序与随机读写。

= 实验内容

本实验要求设计并实现一个类 FAT16 的简单文件系统。文件系统支持参数化的几何配置，包括可配置的块大小、每簇块数与总块数。磁盘布局分为引导扇区（块 0）、两份 FAT 副本及数据区域三部分，根目录以普通目录文件的形式存放于数据区域起始处。实验需在用户态实现基本的目录操作、文件操作与内容修改操作，并支持以十六进制格式转储 FAT 数据以验证文件系统内部状态。

= 实验方法

== 类 FAT16 文件系统结构设计

#paragraph([设备抽象与盘上组织])[
  按照教科书要求，实验中实现的磁盘布局为经简化的类 FAT16 结构@wikipedia2026fat。如@figure:disk 所示，逻辑块 0 为引导扇区，包含描述文件系统结构参数的元数据。其后依次为两份互为冗余的文件分配表副本 FAT#sub[1] 和 FAT#sub[2]。每个 FAT 表项为 2 字节。剩余区域为数据区域，用于存放目录与文件内容。与 FAT16 不同，本设计不设独立的根目录区域，而是将根目录作为一普通目录文件存放于数据区域起始处，起始簇号为 2，支持通过 FAT 链扩展按需增长。盘簇 0 的 FAT 表项作为空指针节点恒置为链结束标记，盘簇 1 同理。
]

#img(
  image("assets/disk.png", width: 85%),
  caption: [类 FAT16 文件系统结构],
) <figure:disk>

实验中的文件系统采用分层设备抽象将存储介质的物理细节与核心逻辑解耦。如@figure:phys-blocks 所示，物理块设备抽象层以固定大小的物理块为单位提供原始读写能力；上层逻辑块设备适配器将多个物理块合并为更大的逻辑块供文件系统核心使用，逻辑块大小须为物理块大小的整数倍。这种分层设计使得文件系统核心逻辑可在内存模拟、磁盘文件镜像、未来的块设备后端等不同底层设备之间复用，而无需修改核心代码。

#img(
  image("assets/phys_blocks.png", width: 60%),
  caption: [物理块、逻辑块与 FAT 簇的关系],
) <figure:phys-blocks>

#paragraph([引导信息扇区结构])[
  引导扇区存储于物理块 0 的前缀位置，以固定长度记录文件系统的几何配置。其字段包括块大小、磁盘总块数、每簇块数、FAT 起始块号、每份 FAT 副本的块数、FAT 副本数、数据区域起始块号以及根目录起始簇号。重新打开已格式化磁盘时须从引导扇区读取这些参数并与实际设备信息进行一致性校验，以确保挂载的正确性。
]

#paragraph([文件控制块结构])[
  文件控制块（FCB）是文件系统目录项的基本单元，以固定长度 16 字节记录文件的元信息。其字段包括文件名与扩展名、标记区分文件与目录的属性字节、修改时间与日期、文件起始簇号以及文件大小。文件名采用 FAT 标准的存储格式，使用大写 ASCII 编码，不足部分以空格填充；修改日期采用 FAT 风格的位压缩编码：日期占 7 位年份、4 位月份和 5 位日期，时间占 5 位小时、6 位分钟和 5 位秒数。
]

目录文件本身由连续排列的 FCB 构成，每簇可容纳多个 FCB 项。由于 FCB 大小不一定为簇大小的因子，在每簇尾部允许出现少量浪费空间。

#img(
  image("assets/fcb_layout.png", width: 60%),
  caption: [文件控制块结构],
) <figure:fcb-layout>

#paragraph([文件分配表结构])[
  文件分配表（FAT）是连续的 16 位表项数组，表项语义包括空闲簇、链中下一簇号和链尾（EOC）三种。FAT 的大小确定是一个特殊的迭代收敛问题。如@code:fat-sizing 所示，$B$ 为块大小，$N$ 为磁盘总块数，$C$ 为每簇块数。算法以 1 块为初始猜测，计算当前 FAT 大小能容纳的簇条目数及数据区域实际需要的簇数，若 FAT 寻址能力不足则增大 FAT 块数后重复上述过程直至收敛。
]

#code(
  ```txt
  function compute_fat_block_count(B, N, C):
      fat_blocks := 1
      loop:
          data_start := 1 + 2 * fat_blocks
          if data_start >= N:
              return fat_blocks
          data_clusters := (N - data_start) / C
          needed_entries := 2 + data_clusters
          needed_bytes := needed_entries * 2
          needed_blocks := Ceil(needed_bytes / B)
          if needed_blocks <= fat_blocks:
              return fat_blocks
          fat_blocks := needed_blocks
  ```,
  caption: [FAT 块数迭代计算算法],
) <code:fat-sizing>

该算法的可行性源于其单调性与有界性。迭代过程中 `fat_blocks` 严格递增，因此优化是单调的。当 FAT 的块数大到数据区域为零时必然满足寻址需求，因此存在可行解，即存在上界，优化具有有界性。因此，该优化过程必在有限步内收敛。

两份互为冗余的 FAT 副本同时存放在磁盘上。FAT 修改操作只更新内存中的缓存，在需要同步时写回磁盘。

== 基本文件操作

文件系统须提供格式化、目录操作、文件操作及内容读写等基本功能。格式化负责写入引导扇区、初始化 FAT 副本并建立根目录。目录操作包括在指定位置创建子目录、删除空目录及遍历目录内容。文件操作涵盖创建空文件、删除文件并回收其占用的 FAT 链、按路径打开与关闭文件，以及基于文件描述符的顺序读写与基于字节偏移量的随机读写。写入过程中若当前 FAT 链容量不足，须自动分配新簇并扩展链结构。截断操作用于缩小或扩展文件大小：缩小需释放尾部多余簇，扩展需分配新簇并以零填充。

文件的创建流程如@code:create-file 所示。首先将用户输入的名称规范化为 FAT 短名格式，检查父目录中是否已存在同名文件。通过线性扫描在父目录中寻找空闲 FCB 槽位，若当前簇的 FCB 已满则沿 FAT 链扩展目录文件。以空闲簇标记与零大小初始化 FCB 并写入槽位，最后更新父目录的尺寸元数据。

#code(
  ```txt
  function create_file(parent, name):
      key := normalize(name)
      if lookup(parent, key) succeeds:
          return error(AlreadyExists)
      slot := fill_free_slot(parent)
      fcb := new_fcb(key, attr=FILE, start=FREE, size=0)
      write_fcb(slot, fcb)
      refresh_dir_size(parent)
      return slot
  ```,
  caption: [文件创建流程],
) <code:create-file>

按偏移量读取文件内容的过程如@code:read-at 所示。从 FCB 中读取文件大小，若请求偏移超出文件尾则直接返回空。实际读取长度取请求长度与文件剩余字节数的较小者。若文件起始簇为空闲标记（即空文件），直接返回空；否则委托给链式读取函数，后者根据偏移量定位目标簇，沿 FAT 表项逐簇前进直至读取足量字节或抵达链尾。

#code(
  ```txt
  function read_at(loc, offset, len):
      fcb := read_fcb(loc)
      if fcb.attr == DIRECTORY:
          return error(IsADirectory)
      if offset >= fcb.size:
          return empty
      read_len := min(len, fcb.size - offset)
      if fcb.start == FREE:
          return empty
      return read_chain(fcb.start, offset, read_len)
  ```,
  caption: [按偏移量读取文件],
) <code:read-at>

截断操作如@code:truncate 所示。增长路径先通过 `ensure_chain_capacity` 扩展 FAT 链至足够容纳新大小的簇数，再以零填充新增的字节区域；若中途分配失败须将已扩展的链回滚至原长度。缩小路径则计算新大小所需的簇数：若降至零则释放整条链并将起始簇置为空闲，否则调用 `trim_chain_len` 裁剪尾部多余簇并对最后一个簇的尾部做零填充。两种路径完成后统一更新 FCB 中的文件大小字段，并调整所有打开句柄中超过新文件尾的游标位置。

#code(
  ```txt
  function truncate(loc, new_size):
      fcb := read_fcb(loc)
      if new_size == fcb.size:
          return
      if new_size > fcb.size:                     // 扩展
          old := cluster_count(fcb)
          needed := Ceil(new_size / CLUSTER_SIZE)
          ensure_chain_capacity(fcb, needed)
          if failed:
              return error(NoSpace)
          zero_fill(fcb.start, fcb.size, new_size - fcb.size)
          if failed:                              // 回滚
              trim_chain_len(fcb.start, old)
              return error(NoSpace)
      else:                                        // 缩小
          needed := Ceil(new_size / CLUSTER_SIZE)
          if needed == 0:
              free_full_chain(fcb.start)
              fcb.start := FREE
          else:
              trim_chain_len(fcb.start, needed)
              zero_tail(fcb.start, new_size)
      fcb.size := new_size
      write_fcb(loc, fcb)
      clamp_open_cursors(loc, new_size)
  ```,
  caption: [文件截断（缩小与扩展）],
) <code:truncate>

为简化设计，实验限制了同一时间能够打开的文件数量。同时，重复打开同一文件应予拒绝，正在打开状态下的文件应拒绝删除操作。每次修改文件内容或元数据后需更新对应的修改时间戳。

== 用户态文件系统（FUSE）接口

用户态文件系统（Filesystem in User space，FUSE）是 Linux 内核提供的一种机制，允许在用户态实现文件系统逻辑，通过内核虚拟文件系统层暴露为标准目录树。用户态进程实现内核 VFS 所需的回调接口，包括路径名查找、节点属性读取与修改、文件与目录的创建与删除、文件打开与关闭、基于偏移量的读写以及目录枚举等操作。内核将文件系统请求通过 FUSE 设备转发至守护进程，守护进程完成操作后返回结果@kernel2026fuse。通过 FUSE，自定义文件系统可像内核原生文件系统一样被挂载，支持用户程序访问。

= 实验过程

== Rust 语言特性与规避

本实验使用 Rust 编程语言完成。Rust 的所有权与借用模型为系统编程提供了内存安全保障，但也对涉及共享可变状态的文件系统代码提出了特殊的实现要求。`bytemuck` crate 的 `Zeroable` 与 `Pod` trait 允许对 `#[repr(C)]` 结构体进行零开销字节转换，使盘上数据结构的序列化与反序列化退化为内存直译，避免了手写编解码。然而，要将该特性应用于文件系统仍需解决若干语言层面的约束。

FAT 标准的短名称格式为 8.3（文件名 8 字节加扩展名 3 字节，共 11 字节）。若以 `[u8; 8]` 与 `[u8; 3]` 作为相邻字段，结构体在 `#[repr(C)]` 下的内存布局中，其后的 `mtime` 字段与前面的字段序列之间存在填充空隙。该填充字节是 `bytemuck` 库无法处理的。`Pod` 要求类型的所有字节表示均有效，而填充字节的值在编译器的内存布局中不做保证。作为妥协，本实验将文件名扩展至 9.3 格式（文件名 9 字节加扩展名 3 字节共 12 字节），使得后续字段序列天然满足 `u16` 的对齐，消除了所有填充空隙，从而在保持兼容 FAT 命名规范的前提下使 FCB 结构体满足 bytemuck 的安全约束。

文件系统的核心结构体 `MyFileSystem<D>` 封装了可变设备 `D`。若将所有读取操作为 `&mut self`，会导致无法在持有簇迭代器或目录槽位迭代器的同时进行任何修改操作。文件系统大量不修改文件系统状态的查询操作（如读取文件元数据、查找文件等）也必须扩散 `&mut` 要求，严重牺牲 API 的可用性。本实验使用 `RefCell<D>` 包裹设备，使得读取类 API 可保持 `&self` 签名，在运行时由 `RefCell` 执行借用检查。该方案以最小代价保证了 API 接口与实现的简洁与正确性。

文件操作中需要遍历 FAT 链读取簇内容。若采用先读取完整链再处理的策略，当文件包含大量簇时会产生不必要的内存拷贝与分配开销。本实验使用 `ChainIter` 迭代器从起始簇号出发跟随 FAT 表项逐簇推进，内部通过 `HashSet` 记录已访问簇以检测循环。目录遍历同样使用两层迭代器完成：`DirSlotIter` 遍历原始槽位并处理每簇末尾的尾部松弛浪费，`DirEntryIter` 在其基础上过滤已占用项并解码为面向用户的目录条目。迭代器的惰性求值特性使得链遍历与目录扫描仅在访问发生时按需推进，无需提前将全部数据复制至内存。

== 实现文件系统核心库 libmyfs

模块划分围绕数据依赖关系展开。使用 `repr(C)` 内存布局约束落盘数据结构，如 FCB、引导扇区内容、时间压缩编码等结构体。为了避免整数的端序问题和手动转换的危险性，编写 `FatEntry` 枚举实现与 `u16` 的双向转换。为定义设备 trait 层次并实现两个后端，核心文件系统代码完全通过 trait 接口访问存储，对 MemoryBackend 与 FileBackend 的存在并不知情。这种分离使得新增后端（如真实的 Linux 块设备）不需修改任何核心逻辑。

本实验使用 FAT 文件系统的文件名命名规则，存储大写 ASCII、`.` 与空格分隔的定长记录。因此需对用户输入的路径进行规格化，包括拆分扩展名、空格填充、拒绝过长或不符合要求的文件名等。本实验为了支持 Linux 上常见的“dotfile”文件名，如 `.git` 等，对 FAT 命名规则进行扩展，允许在扩展名存在的前提下基础名为空。

FAT 链是文件的底层字节流抽象。为了达到与 C 语言类似的性能，实验遍历 FAT 链编写了迭代器 `ChainIter`，从给定的起始位置出发，跟随 FAT 中的 Next 表项推进。该方案可以在常数辅助空间内支持任意长度的链，效率与手写的循环相当。为了在磁盘上的 FAT 链出现循环时立即报告错误，还加入一个哈希表记录已访问过的表项。`allocate_clusters` 采用线性扫描寻找空闲项：每次调用扫描全局簇范围，分配第一个可用的连续区间。

目录存储为一系列连续排列的 FCB。由于 FCB 大小不一定是簇大小的因子，每个簇的尾部可能存在内部碎片。同样为了快速实现目录内容的迭代，编写 `DirSlotIter` 将目录抽象为槽位序列，按照簇索引与簇内索引的方式自动跳过每簇末尾的无效部分。`DirEntryIter` 在槽位迭代器之上过滤出已分配且未删除的项，并将原始 FCB 转换为适合暴露给用户的目录内容数据。

== 实现简单 Shell 交互

`myfs_shell` 的设计目标是以最小的工程量暴露文件系统的完整功能，用于验证与调试。核心设计决策是路径解析的单入口模式：所有对文件系统对象的访问统一经过 `resolve_target` 函数，该函数从根目录开始向下遍历路径组件，逐步调用 `lookup` 确认每个中间目录的存在，最终返回目标节点的 `DirEntryLoc`。路径遍历过程与 `..` 的父目录回溯在同一趟中完成，用户侧则无需关心路径是绝对还是相对形式。

命令的分发采用简单的字符串匹配：解析首个命令词后分派至对应的处理函数，避免引入额外的命令行解析库。除常规文件操作外，Shell 还提供 `fat` 命令用于调试。该命令以可读形式遍历每条 FAT 链，显示各簇的分配状态与链接关系，使文件占用的簇链可直接观察。

== 挂接 FUSE

FUSE 围绕索引式文件系统设计，而本实验中实现的类 FAT16 文件系统不存在索引节点，因此需要为 FUSE 交互手动构造稳定唯一的“伪”索引节点标识，并能够高效相互转换。本实验将 `DirEntryLoc` 编码为 `u64`：高 32 位记录父目录起始簇号、低 32 位记录槽位索引，`0x1` 保留给根节点。

FUSE 协议要求实现带有偏移量的随机 I/O，而已有为 Shell 提供的接口则采用基于游标的顺序 I/O。由于底层的 FAT 簇链读取函数已经支持随机读写，只需将其以不同方式挂接到外部接口即可，可以快速提供两套正交的读写接口。

`myfs` FUSE 程序提供了两种启动方式。`-M` 在内存中创建全新文件系统后挂载，适用于快速演示。`-i` 则加载已有格式化镜像后挂载，展示文件系统的持久化能力。这种设计使同一套 FUSE 实现可以同时服务于暂态与持久两种场景。

== 单元测试与功能测试

单元测试按层次组织。底层测试验证数据结构的字节序列化正确性，如引导块与 FCB 的序列化、`FatEntry` 与 `u16` 的双向映射，这些测试确保了盘上表示与内存表示的一致性。中层测试验证文件系统配置校验逻辑和 FAT 大小计算的收敛性，后者在早期版本中曾因分母选择不当导致 `get_fat_block_count` 在特定磁盘参数下不收敛。上层测试则验证文件系统的操作语义，如目录的创建与删除链完整性检查、打开句柄上限、拒绝重复打开、拒绝删除打开文件、写入数据跨越块边界时的正确性、多簇目录的增长与遍历一致性等。

异常路径的覆盖是测试设计的重点。磁盘空间耗尽时的 `NoSpace` 错误、截断扩展在簇分配中途失败时的链回滚完整性、从空文件与非空文件两种状态出发的失败回滚均有专用测试。这些场景在正常使用中难以触发，但它们的正确行为恰是文件系统可靠性的基石。本测试套件在编写过程中借助了 GPT-5.4 辅助生成测试模板与边界用例，显著加快了覆盖率收敛的速度。

#img(
  image("assets/test_ok.png"),
  caption: [单元测试运行演示],
) <figure:test-ok>

如@figure:shell-demo 所示，交互式 Shell 支持完整的目录与文件操作命令集。

#img(
  image("assets/shell_demo.png", width: 70%),
  caption: [交互式 Shell 运行演示],
) <figure:shell-demo>

如@figure:fuse-demo 所示，通过 FUSE 挂接后的文件系统可被标准 Linux 工具直接访问。

#img(
  image("assets/fuse_demo.png"),
  caption: [FUSE 挂载演示],
) <figure:fuse-demo>

= 结果分析与实验体会

实验实现了功能较为全面的类 FAT16 文件系统，内存模式与文件镜像持久化模式均可正常工作。交互式 Shell 支持全部要求的基本文件系统操作，FUSE 挂接后可通过标准 Linux 命令访问文件系统内容。

通过本次实验，我对文件系统设计中的核心问题有了更深入的理解。其一，分层设备抽象使文件系统核心逻辑与存储细节解耦。物理块设备接口仅要求基本的块读写能力，逻辑块适配器处理物理块到逻辑块的映射，核心文件系统代码只需面向逻辑块接口编程，无需关心底层存储介质是内存还是磁盘文件。其二，FAT 大小的迭代计算是一个典型的反馈收敛问题：FAT 的寻址能力决定了可管理的数据簇数，而数据簇数又反过来决定了 FAT 所需的大小，这种相互依赖关系需要经过若干轮迭代才能确定唯一解。其三，FAT 链的分配与回收涉及多个元数据写操作，部分失败会导致文件系统处于不一致状态。实验中通过回滚机制保证分配或截断操作的事务性，体现了文件系统设计中数据一致性的重要性。其四，GPT-5.4 在测试环节显著加速了边界条件的发现与覆盖。截断回滚、空间耗尽等边角情况的测试使用 AI 辅助设计，使覆盖率能够快速收敛至满足可靠性验证需要的水平，弥补了人工测试设计中对异常路径的系统性盲区。本次实验极大加深了我对文件系统盘上结构设计、FAT 文件系统工作原理、 FUSE 虚拟文件系统接口概念的理解。

= 源代码

本报告源文件可以从 #link("https://github.com/CSharperMantle/hdu2026_os_exp/tree/main/exp_5") 处获取。实验代码包含三个模块：`myfs`（核心库与 FUSE 可执行文件）、`myfs_shell`（命令行交互 Shell）以及 `mkfs_myfs`（磁盘镜像格式化工具）。

#pagebreak()

#bibliography("bib.bib", style: "gb-7714-2015-numeric")
