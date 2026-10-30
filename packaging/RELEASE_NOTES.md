# Sean Toolbox 0.1.0

首个正式版本。用快捷键切换常用应用、唤出下拉终端，快速回到 VS Code 最近项目。

## 主要功能

- **应用快捷键**：按需启动应用，切回或最小化窗口；同一应用有多个窗口时依次切换。默认示例为 WezTerm `F3`、VS Code `F4`、Chrome `F5`。
- **下拉与停靠**：窗口可停靠到四边，按工作区比例设置宽高，选择显示器与对齐位置。Windows 支持显示时置顶、失焦自动收起；Linux X11 支持置顶。
- **最近项目**：`Ctrl+Alt+Space` 打开独立搜索面板，按名称或路径搜索 VS Code Stable 的文件夹、工作区与远程项目，使用键盘或鼠标打开。
- **界面与配置**：跟随系统、浅色和深色主题，中文和英文界面，配置导入与导出，可选开机启动。
- **按需界面**：主界面关闭后默认退出界面进程，宿主继续处理托盘和全局快捷键。

## Windows：独立的 WezTerm 下拉终端

新配置默认用 `F3` 在鼠标所在显示器顶部唤出 WezTerm，占工作区全部宽度和一半高度。终端在前台时再按 `F3` 隐藏；再次唤出继续同一个 shell，后台命令和会话保持运行。

下拉终端采用独立进程和窗口身份，与普通 WezTerm 窗口分开管理。收起使用 Windows 窗口隐藏（`SW_HIDE`），下次显示前重新定位。退出工具箱或停用该配置后，隐藏窗口恢复可见。

沿用已有 WezTerm 的 shell、字体和配色；专用窗口去掉标题栏、保留可调整边框，改变字号时保持外框尺寸。宽高、显示器、置顶与失焦收起可在应用编辑区调整。

**已有配置的用户**：选择 WezTerm，勾选“启用 WezTerm 独立下拉”并保存。快捷键、启动参数和工作目录沿用原配置；程序路径需指向本机 WezTerm。

## 下载与升级

| 系统与处理器 | 压缩包 |
| --- | --- |
| Windows x64 | `sean-toolbox-0.1.0-windows-x86_64.zip` |
| macOS Apple 芯片 | `sean-toolbox-0.1.0-macos-aarch64.zip` |
| macOS Intel | `sean-toolbox-0.1.0-macos-x86_64.zip` |
| Linux x64 | `sean-toolbox-0.1.0-linux-x86_64.tar.gz` |

每份归档附同名 `.sha256` 校验文件。完整解压后按 `INSTALL.md` 启动；Windows 保留两个 `.exe` 在同一目录，macOS 保持整个 `.app` 完整。升级前从托盘退出工具箱，再替换程序；已有配置继续保留。

当前 Windows 包未做 Authenticode 签名；macOS 包未做 Developer ID 签名及 Apple 公证。macOS 窗口控制需要辅助功能权限，Linux 外部窗口控制使用 X11。

[使用指南](https://github.com/sean2077/sean-toolbox/blob/v0.1.0/docs/USAGE.md) · [验证记录](https://github.com/sean2077/sean-toolbox/blob/v0.1.0/docs/VALIDATION.md)
