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

= 实验内容

= 实验方法

== 类 FAT16 文件系统结构设计

#paragraph([设备抽象与盘上组织])[
  #lorem(50)
]

#paragraph([引导信息扇区结构])[
  #lorem(50)
]

#paragraph([文件控制块结构])[
  #lorem(50)
]

#paragraph([文件分配表结构与大小])[
  #lorem(50)
]

== 基本文件操作

== 用户态文件系统（FUSE）接口

= 实验过程

== Rust 语言特性

== 实现文件系统核心库 libmyfs

== 实现简单 Shell 交互

== 挂接 FUSE 接口

== 功能测试

= 结果分析与实验体会

= 源代码

本报告源文件可以从 #link("https://github.com/CSharperMantle/hdu2026_os_exp/tree/main/exp_5") 处获取。

#pagebreak()

#bibliography("bib.bib", style: "gb-7714-2015-numeric")
