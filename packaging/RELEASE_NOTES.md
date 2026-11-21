# Sean Toolbox 0.4.0

本次更新新增跨机剪贴板和 KDE Plasma Wayland 窗口切换支持，Skills 管理增加通用 `.agents` 部署目标，并修复 Windows 窗口重绘、Skills 列表滚动和部署按钮点击问题。

## 新增：跨机剪贴板

- 支持 Windows 与 Linux Wayland 两台设备配对同步，在“跨机剪贴板”页面或设置中配置监听/连接角色、地址和传输上限，查看连接状态、传输进度、缓存占用与错误。功能默认关闭。
- 文字、HTML 和图片在复制后立即同步到另一端的剪贴板。文件和文件夹在复制时只发送清单，真正粘贴时才开始传输。
- 小文件可直接粘贴。较大文件的首次粘贴启动传输，收到“文件已就绪”通知后再次粘贴，即可从本机缓存取出。
- 连接使用 Noise NNpsk0 和独立保存的 32 字节预共享密钥。支持断线重连、休眠唤醒恢复和同步回环抑制，剪贴板写入失败会通知并记录错误。
- 文字/HTML 单项上限 16 MiB，图片上限 64 MiB，文件总量上限可配置、最大 1 GiB。接收文件缓存自动按保留时间和总量清理，也可在界面或通过 `sean-toolbox clipboard clear-cache` 清空。
- 两端须使用同一协议版本 v3。先前开发构建的 `mode` 配置仍可读取，保存时移除。Linux 需要合成器提供 `ext_data_control_manager_v1`，当前真机验收覆盖 KDE Plasma Wayland 与 Windows 11，记录见文末验证文档。

## 新增：KDE Plasma Wayland 窗口切换

- 在 KDE Plasma Wayland 下，通过 KWin 脚本注册全局快捷键、切换应用窗口，并在没有窗口时启动应用。需要 `qdbus6`，脚本由宿主自动安装和加载。
- 保存配置后同步更新 KGlobalAccel 绑定，托盘和 CLI 切换与物理快捷键使用同一动作。
- 修复管理窗口在启动后首次显示时无法出现的问题，并改进 Linux 窗口、任务栏和托盘的应用识别。
- Linux 开机启动增加异常退出后自动重启，主动退出仍会正常停止。
- 新配置中的 WezTerm 默认不绑定 F3，避免与原生 quake 下拉快捷键冲突。已有配置中的快捷键保持原值，可继续自行设置。
- 窗口切换与全局快捷键的 Wayland 支持限定 KDE Plasma，其他 Wayland 合成器仍不支持这些功能。

## 改进：Skills 管理

- 新增“通用 (.agents)”部署目标，将技能部署到 `~/.agents/skills`，支持符号链接、复制、安装、本机导入和批量操作。已有技能不会自动启用此目标。
- 修复已安装、发现和本机技能长列表无法滚动的问题。
- 修复部署图标首次点击未执行操作的问题。
- 链接技能的源目录被移除后，仍可停用或卸载工具箱创建的链接。被其他程序替换的链接继续受冲突保护。

## 修复：Windows 界面重绘

- 修复窗口底部、右侧空白及调整尺寸后的重绘缺口。
- 增加原生重绘回归检查，覆盖浅深主题、多种缩放和窗口恢复场景。

## 下载与升级

| 系统与处理器 | 压缩包 |
| --- | --- |
| Windows x64 | `sean-toolbox-0.4.0-windows-x86_64.zip` |
| macOS Apple 芯片 | `sean-toolbox-0.4.0-macos-aarch64.zip` |
| macOS Intel | `sean-toolbox-0.4.0-macos-x86_64.zip` |
| Linux x64 | `sean-toolbox-0.4.0-linux-x86_64.tar.gz` |

每份归档附同名 `.sha256` 校验文件。升级前从托盘退出工具箱，再完整替换程序。Windows 保留两个 `.exe` 在同一目录，macOS 保持整个 `.app` 完整。

已有配置和 Skills 管理数据可继续使用，版本 1 配置读取后自动迁移。跨机剪贴板默认关闭，首次启用需按使用指南为两端配置相同密钥和连接地址。如需保留回退能力，升级前备份配置和 Skills 数据，旧版不能直接读取本版保存的剪贴板配置和新增部署目标。

当前 Windows 包未做 Authenticode 签名，macOS 包未做 Developer ID 签名及 Apple 公证。跨机剪贴板未提供 macOS 或 Linux X11 后端，macOS 和 Linux X11 的原有功能继续保留。

[使用指南](https://github.com/sean2077/sean-toolbox/blob/v0.4.0/docs/USAGE.md) · [验证记录](https://github.com/sean2077/sean-toolbox/blob/v0.4.0/docs/VALIDATION.md)
