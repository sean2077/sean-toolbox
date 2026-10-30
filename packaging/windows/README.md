# Windows 便携包

## 启动

完整解压后运行 `sean-toolbox.exe`，将 `sean-toolbox-ui.exe` 保留在同一目录。
默认按 `Ctrl+Alt+Space` 打开最近项目面板，应用配置和设置可从托盘菜单打开。
首次使用时，按本机安装情况调整应用路径、窗口匹配条件和快捷键。

当前包未做 Authenticode 签名，Windows 可能显示未知发布者或 SmartScreen 提示。
下载时核对来源，并使用归档旁的 `.sha256` 文件校验。

## 更新

从托盘菜单选择“退出工具箱”，再用新版本替换两个程序。
配置保存在 `%LOCALAPPDATA%\SeanToolbox\config.json`，更换程序后继续使用。
