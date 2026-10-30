# Linux 便携包

在图形会话中运行 `./bin/sean-toolbox`。主程序与 `sean-toolbox-ui` 必须保留在同一目录。
默认使用 `Ctrl+Alt+Space` 打开工具箱。

包在 Ubuntu 22.04 上构建，面向 glibc 2.35 或更新的桌面系统。
需要系统提供 X11/Wayland、xkbcommon、Fontconfig 等桌面运行库。
托盘使用 StatusNotifierItem，需要桌面环境支持该协议；文件选择对话框需要
`xdg-desktop-portal` 以及适合当前桌面的 portal 后端。

窗口显示/隐藏和全局快捷键的首版 Linux 支持面向 X11。
Wayland 下的全局窗口操作受合成器限制，项目浏览/打开仍可使用；具体限制见主 README。

## 可选：加入应用菜单

在解压后的目录执行以下命令。这会复制两个程序及桌面入口到当前用户目录：

```sh
mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications" "$HOME/.local/share/icons/hicolor/scalable/apps" "$HOME/.local/share/icons/hicolor/256x256/apps"
cp bin/sean-toolbox bin/sean-toolbox-ui "$HOME/.local/bin/"
chmod +x "$HOME/.local/bin/sean-toolbox" "$HOME/.local/bin/sean-toolbox-ui"
cp share/applications/sean-toolbox.desktop "$HOME/.local/share/applications/"
cp share/icons/hicolor/scalable/apps/sean-toolbox.svg "$HOME/.local/share/icons/hicolor/scalable/apps/"
cp share/icons/hicolor/256x256/apps/sean-toolbox.png "$HOME/.local/share/icons/hicolor/256x256/apps/"
```

桌面会话的 PATH 需要包含 `~/.local/bin`。若未包含，请将安装后 `.desktop` 文件的
`Exec=` 改为程序的绝对路径，路径有空格时用双引号包围。此包不包含自更新程序。
更新前从托盘退出，再替换两个二进制文件。AHK、uTools、Node.js 和 Python 都不是运行依赖。
