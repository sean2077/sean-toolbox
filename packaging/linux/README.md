# Linux 便携包

## 运行环境

发布工作流在 Ubuntu 22.04 上构建，面向 glibc 2.35 或更新的桌面系统。

- 系统运行库：X11/Wayland、xkbcommon、Fontconfig 等。
- 托盘：桌面环境需要支持 StatusNotifierItem。
- 文件导入和导出：需要 `xdg-desktop-portal` 及适合当前桌面的文件选择后端，或 `zenity`。

X11 下支持窗口控制和全局快捷键。Wayland 下可使用最近项目、设置和托盘；
外部窗口控制与全局快捷键暂未支持。

## 启动

在图形会话中运行 `./bin/sean-toolbox`，将 `sean-toolbox-ui` 保留在同一目录。
X11 下默认按 `Ctrl+Alt+Space` 打开最近项目面板；也可从托盘菜单或命令行打开界面。

## 加入应用菜单

在解压后的目录执行以下命令。这会复制两个程序及桌面入口到当前用户目录：

```sh
mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications" \
  "$HOME/.local/share/icons/hicolor/scalable/apps" "$HOME/.local/share/icons/hicolor/256x256/apps"
cp bin/sean-toolbox bin/sean-toolbox-ui "$HOME/.local/bin/"
chmod +x "$HOME/.local/bin/sean-toolbox" "$HOME/.local/bin/sean-toolbox-ui"
cp share/applications/sean-toolbox.desktop "$HOME/.local/share/applications/"
cp share/icons/hicolor/scalable/apps/sean-toolbox.svg "$HOME/.local/share/icons/hicolor/scalable/apps/"
cp share/icons/hicolor/256x256/apps/sean-toolbox.png "$HOME/.local/share/icons/hicolor/256x256/apps/"
```

桌面会话的 PATH 需要包含 `~/.local/bin`。若未包含，请将安装后 `.desktop` 文件的
`Exec=` 改为程序的绝对路径，路径有空格时用双引号包围。

## 更新

从托盘退出工具箱，或运行 `./bin/sean-toolbox quit`，再替换两个程序。
配置保存在 `$XDG_CONFIG_HOME/sean-toolbox/config.json`；未设置该变量时为
`~/.config/sean-toolbox/config.json`。
