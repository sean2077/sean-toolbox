# macOS 应用包

## 安装与启动

下载与处理器匹配的包：Apple 芯片使用 `aarch64`，Intel 使用 `x86_64`。
最低系统版本为 macOS 11。

解压后将完整的 `Sean Toolbox.app` 移到 `/Applications` 或 `~/Applications`，然后打开。
应用以菜单栏工具运行；默认按 `Ctrl+Alt+Space` 打开最近项目面板，应用配置和设置可从菜单栏打开。

## 系统权限

窗口控制需要辅助功能权限：在“系统设置 → 隐私与安全性 → 辅助功能”中授权 Sean Toolbox，
再到工具箱的运行状态页重新检查。

当前包未做 Developer ID 签名及 Apple 公证。系统可能要求在“隐私与安全性”中确认后才能打开；
受管理的设备按组织策略运行应用。

## 更新

从菜单栏退出工具箱后，替换整个 `.app`。配置保存在
`~/Library/Application Support/SeanToolbox/config.json`。
更新或移动应用后，如窗口控制权限失效，在系统设置中重新授权。
