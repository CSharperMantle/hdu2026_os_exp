# hdu2026_os_exp

HDU (2025-2026-2)-S0512250-12 《操作系统课程实践》实验

* [exp_1/](exp_1/) - 实验1：Linux 内核添加系统调用
  * csm_hostname(2) - `int csm_hostname(int is_set, char *name, int len)`
  * 基分支：[v6.19.6-arch1](https://github.com/archlinux/linux/tree/v6.19.6-arch1)
  * [report.typ](exp_1/report.typ)
* [exp_3/](exp_3/) - 实验3：模拟 Shell 及进程通信
  * Zig
  * [myshell/](exp_3/myshell/) - 模拟 Shell
    * 多级管道
    * 重定向：fd 到文件、fd 到 fd、文件到 stdin、追加模式
    * 内置命令：`cd`、`pwd`、`exit`
  * [mychat/](exp_3/mychat/) - 聊天室
    * 1+N 聊天室
      * [fifo(7)](https://www.man7.org/linux/man-pages/man7/fifo.7.html)
      * [mq_overview(7)](https://www.man7.org/linux/man-pages/man7/mq_overview.7.html)
      * [shm_overview(7)](https://www.man7.org/linux/man-pages/man7/shm_overview.7.html)
    * P2P 聊天室
      * [shm_overview(7)](https://www.man7.org/linux/man-pages/man7/shm_overview.7.html)
  * [report.typ](exp_3/report.typ)
* [exp_5/](exp_5/) - 实验5：简单文件系统设计与实现
  * Rust
  * [myfs/](exp_5/myfs/) - 文件系统核心 libmyfs 与 [FUSE](https://www.kernel.org/doc/html/next/filesystems/fuse/fuse.html) user land 应用 myfs(1)
  * [myfs_shell/](exp_5/myfs_shell/) - 模拟文件系统交互 Shell
  * [mkfs_myfs](exp_5/mkfs_myfs/) - 格式化工具 mkfs.myfs(1)
  * [report.typ](exp_5/report.typ)

## 许可

```plain-text
// SPDX-License-Identifier: Apache-2.0 AND CC-BY-SA-4.0

Copyright (C) 2026 Rong Bao

Licensed under the Apache License, Version 2.0 (the "Apache-2.0 License")
AND the Creative Commons Attribution-ShareAlike 4.0 International Public
License (the "CC-BY-SA-4.0 License"); collectively the "Licenses". You may
not use this program except in compliance with the Licenses.

You may obtain a copy of the Apache-2.0 License at

        https://www.apache.org/licenses/LICENSE-2.0

You may obtain a copy of the CC-BY-SA-4.0 License at

        https://creativecommons.org/licenses/by-sa/4.0/legalcode

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the Licenses for the specific language governing permissions and
limitations under the Licenses.
```

**注意**：[assets/](assets/) 目录下的文件为第三方提供的素材、教案、模板等，依照每项素材内描述转载使用，不适用于上述许可协议。
