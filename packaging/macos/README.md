# macOS 应用包

解压后将 `Sean Toolbox.app` 移到 `/Applications` 或 `~/Applications`，然后打开它。
应用以菜单栏工具运行；默认使用 `Ctrl+Alt+Space` 打开工具箱。
后台主程序和按需界面均在 `.app/Contents/MacOS/` 内，请保持整个应用包完整。

下载与处理器匹配的包：Apple 芯片使用 `aarch64`，Intel 使用 `x86_64`。
应用声明的最低系统版本为 macOS 11。

窗口控制需要在系统“隐私与安全性 → 辅助功能”中授权应用。
当前二进制包没有 Developer ID 发布者签名，也没有 Apple 公证。
系统可能要求在“隐私与安全性”中确认后才能打开，组织管理的设备策略也可能限制运行。

配置保存在用户配置目录。更新前从菜单栏退出，替换整个 `.app`；更新后如窗口控制
权限失效，请在系统设置中重新确认。AHK、uTools、Node.js 和 Python 都不是运行依赖。
