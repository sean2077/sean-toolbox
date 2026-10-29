# 项目术语

规范语言：`zh-CN`、`en`。中文文档和界面使用中文术语；代码、协议和标识符保留英文原名。一个概念的中文名和英文名是等价名称，按语境选用，不强制统一语言。

## 核心概念

**宿主（host）**：
常驻进程 `toolbox-host`，二进制名 `sean-toolbox`。负责单实例、全局快捷键、托盘、配置事务和窗口 worker；不链接 Slint、SQLite 或项目索引模块。
_Equivalent (en)_: host
_Avoid_: 服务端、后台界面

**界面（UI）**：
按需启动的 Slint 前端 `toolbox-ui`，二进制名 `sean-toolbox-ui`。由宿主管理单实例和生命周期，隐藏后可按空闲时间释放。
_Equivalent (en)_: UI, frontend
_Avoid_: 主程序（主程序指宿主）

**应用窗口（application window）**：
由 `toolbox-windows` 控制的外部目标窗口。不是工具箱自己的界面窗口，也不是操作系统平台名。
_Equivalent (en)_: application window
_Avoid_: 窗口（单独使用时含义不清）

**窗口 worker（window worker）**：
宿主内执行 `WindowService` 的专门线程。窗口操作按需触发，不持续枚举所有窗口，也没有空闲轮询循环。
_Equivalent (en)_: window worker

**启动候选（launch candidate）**：
应用配置中 `launch` 数组的一项，包含可执行文件、独立参数数组和工作目录。按顺序尝试；某个候选成功启动后，不再尝试后续候选。
_Equivalent (en)_: launch candidate

**最近项目（recent project）**：
从 VS Code Stable 标准用户目录读取的一条历史目标，可能是本地文件夹、`.code-workspace` 或远程 URI。不是全盘扫描结果，也不包含普通最近文件。
_Equivalent (en)_: recent project

**修订版本（config revision）**：
配置保存使用的乐观并发版本号。保存必须携带宿主当前修订版本；过期或重复的保存会被拒绝，且不改动磁盘文件。
_Equivalent (en)_: config revision
_Avoid_: 版本号（版本号也指应用版本）

**循环位置（cycle position）**：
同一应用多个窗口之间的稳定循环状态。可配置为切换到其他应用后继续上次位置。
_Equivalent (en)_: cycle position

**空闲释放（idle release）**：
界面隐藏后等待指定秒数再退出界面进程；设为 `0` 表示立即释放。宿主继续运行。
_Equivalent (en)_: idle release

**工具箱（toolbox）**：
整个应用，即宿主与按需界面的组合。“工具箱本身”的页面和设置不属于某个被控应用。
_Equivalent (en)_: toolbox