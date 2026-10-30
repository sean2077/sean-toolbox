# Sean Toolbox · 个人工具箱

用快捷键切换应用窗口、唤出停靠面板，快速打开 VS Code 最近项目。

Sean Toolbox 使用 Rust + Slint，支持 Windows、macOS 和 Linux，优先面向 Windows。

[![Made with Slint](https://raw.githubusercontent.com/slint-ui/slint/master/logo/MadeWithSlint-logo-whitebg.png)](https://slint.dev)

## 界面

![最近项目面板](docs/images/project-launcher-dark.png)

按快捷键打开项目面板，输入名称或路径即可搜索。应用快捷键、停靠方式和通用设置在管理窗口中调整。

| 浅色主题 | 深色主题 |
| --- | --- |
| ![浅色设置](docs/images/settings-light.png) | ![深色设置](docs/images/settings-dark.png) |

更多截图：[项目管理](docs/images/projects.png) · [Quake 停靠](docs/images/quake-editor.png)。

## 功能

- **应用快捷键**：启动应用、切回窗口或将窗口最小化；同一应用有多个窗口时依次切换。
- **下拉与停靠**：Windows 可用独立的 WezTerm 下拉终端，收起后任务继续运行；其他应用也可停靠到屏幕一侧。可设置宽高、对齐方式和显示器。
- **最近项目**：搜索 VS Code Stable 的文件夹、工作区和远程项目历史，用键盘或鼠标打开。
- **界面与配置**：中文或英文界面，跟随系统、浅色和深色主题；支持配置导入、导出和开机启动。

关闭界面后，界面进程默认退出；托盘和全局快捷键继续运行。再次唤起时会重新打开界面。

## 下载与开始使用

当前构建从 [Actions](https://github.com/sean2077/sean-toolbox/actions/workflows/ci.yml) 下载：打开最近一次成功的运行记录，在 Artifacts 中选择对应系统的压缩包。版本发布后也可从 [Releases](https://github.com/sean2077/sean-toolbox/releases) 下载。

| 系统 | 启动方式 |
| --- | --- |
| Windows | 完整解压后运行 `sean-toolbox.exe` |
| macOS | 解压后将 `Sean Toolbox.app` 放到固定位置，再打开应用 |
| Linux | 解压后运行 `bin/sean-toolbox`；桌面入口安装步骤见包内 `INSTALL.md` |

保留压缩包的目录结构，主程序和界面程序配套使用。包内附有安装说明，压缩包旁提供 `.sha256` 校验文件。当前构建尚未进行 Windows 代码签名或 macOS 签名与公证。

首次启动后：

1. 按 `Ctrl+Alt+Space` 打开最近项目，输入关键词搜索，按 `Enter` 打开。
2. 从托盘打开“窗口快捷键”，确认程序路径和快捷键。默认示例为 WezTerm `F3`、VS Code `F4`、Chrome `F5`；Windows 新配置的 `F3` 默认唤出 [WezTerm 下拉终端](docs/USAGE.md#wezterm-下拉终端windows)。
3. 在“设置”中选择主题、语言和开机启动。开机启动默认关闭。

退出整个应用可使用托盘“退出工具箱”或 `sean-toolbox quit`。

## 平台支持

| 平台 | 应用窗口控制与全局快捷键 | 使用条件 |
| --- | --- | --- |
| Windows | 支持；Quake 可置顶、失焦收起 | 控制管理员权限的应用窗口需要相应权限 |
| macOS | 支持；Quake 可调整位置和尺寸 | 在系统设置中授予辅助功能权限 |
| Linux X11 | 支持；Quake 可置顶 | EWMH 窗口管理器；Quake 需要 XRandR 1.5 |
| Linux Wayland | 外部窗口控制与全局快捷键暂未实现 | 可使用项目搜索和设置；托盘需要 StatusNotifierItem 支持 |

macOS 和 Linux 的 Quake 窗口暂不支持失焦自动收起；最近项目面板支持失焦收起。Linux 文件导入、导出需要 XDG Desktop Portal 文件选择后端或 `zenity`。

最近项目读取 VS Code Stable 的标准用户目录。Insiders、便携版、自定义数据目录和自定义 profile 暂不支持自动发现。

## 文档

- [使用指南](docs/USAGE.md)：快捷键、窗口切换、Quake、配置、命令行和常见问题。
- [开发指南](https://github.com/sean2077/sean-toolbox/blob/main/docs/DEVELOPMENT.md)：构建、测试和模块结构。
- [打包说明](https://github.com/sean2077/sean-toolbox/blob/main/packaging/README.md)：各平台归档、图标和发布流程。
- [验证记录](https://github.com/sean2077/sean-toolbox/blob/main/docs/VALIDATION.md)：按日期记录的测试环境、结果和资源采样。

## 来源与许可

应用自有代码采用 [MIT](LICENSE)。项目历史读取和进程通信源自 [Recentry](https://github.com/sean2077/recentry)；窗口行为和配置导入参考 [WinGuake](https://github.com/sean2077/winguake)；Quake 停靠交互参考 [Windows Terminal Quake](https://github.com/flyingpie/windows-terminal-quake)。

Slint 按 Royalty-free Desktop, Mobile, and Web Applications License 2.0 使用。来源、版本和许可文本见 [第三方说明](THIRD_PARTY_NOTICES.md)。
