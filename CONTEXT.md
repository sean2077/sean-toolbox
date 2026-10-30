# 项目术语

中文文档和界面使用中文术语；代码、协议和标识符保留英文原名。下列中英文名称指同一概念。

## 核心概念

**宿主（host）**：
常驻进程 `toolbox-host`，二进制名 `sean-toolbox`。负责单实例、全局快捷键、托盘、配置事务和窗口 worker；不链接 Slint、SQLite 或项目索引模块。
_Equivalent (en)_: host
_Avoid_: 服务端、后台界面

**界面（UI）**：
按需启动的 Slint 前端 `toolbox-ui`，二进制名 `sean-toolbox-ui`。由宿主管理单实例和生命周期，自动收起后按配置的缓存时间释放，主动关闭后释放。
_Equivalent (en)_: UI, frontend
_Avoid_: 主程序（主程序指宿主）

**应用窗口（application window）**：
由 `toolbox-windows` 控制的外部应用窗口，例如终端、编辑器或浏览器窗口。
_Equivalent (en)_: application window
_Avoid_: 窗口（单独使用时含义不清）

**窗口 worker（window worker）**：
宿主内执行 `WindowService` 的专门线程，收到请求后执行窗口操作。
_Equivalent (en)_: window worker

**启动候选（launch candidate）**：
应用配置中 `launch` 数组的一项，包含可执行文件、独立参数数组和工作目录。按顺序尝试；某个候选成功启动后，不再尝试后续候选。
_Equivalent (en)_: launch candidate

**最近项目（recent project）**：
从 VS Code Stable 标准用户目录读取的一条历史目标，包括本地文件夹、`.code-workspace` 或远程 URI。
_Equivalent (en)_: recent project

**修订版本（config revision）**：
配置保存使用的乐观并发版本号。保存必须携带宿主当前修订版本；过期或重复的保存会被拒绝，且不改动磁盘文件。
_Equivalent (en)_: config revision
_Avoid_: 版本号（版本号也指应用版本）

**循环位置（cycle position）**：
同一应用多个窗口之间的当前切换位置。切换到其他应用后保留，下一次按键继续选择后继窗口。
_Equivalent (en)_: cycle position

**连续循环（continuous cycle）**：
`cycle_continuous` 控制多个窗口切换到末尾后的行为：开启时回到第一个窗口，关闭时将全部窗口最小化。
_Equivalent (en)_: continuous cycle

**空闲释放（idle release）**：
界面自动收起后，等待配置的缓存时间再退出；`0` 表示立即释放。另一可见界面、草稿、对话框或进行中的操作会延后退出。主动关闭最后一个界面会跳过缓存时间。宿主继续运行。
_Equivalent (en)_: idle release

**工具箱（toolbox）**：
整个 Sean Toolbox 应用，包括宿主与按需界面。
_Equivalent (en)_: toolbox
