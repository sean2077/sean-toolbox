# Sean Toolbox 0.2.0

新增 Skills 管理：集中安装、部署并每日更新 AI 编程工具的 Skills。原有应用快捷键、下拉/停靠、最近项目和按需界面继续可用。

## 新增：Skills 管理

- 从 GitHub 仓库发现和安装 Skills，可指定分支；也可扫描本机已有目录，或导入包含 `SKILL.md` 的文件夹。
- 支持 Claude Code、Codex、Gemini CLI、OpenCode、Grok Build 的默认 Skills 目录；私有主副本加复制部署，不需要 Windows 开发者模式。
- “已安装”页可按应用启用/停用、单项或全部更新、卸载；本机导入保留原目录，不接管已有同名目录。
- 每日自动更新默认关闭；开启后立即检查，成功后间隔 24 小时，失败后 1 小时重试。宿主运行时执行，关闭界面后继续，下次启动补跑。
- 更新和卸载前校验 SHA-256 指纹；检测到本地修改会停止并报告，不覆盖用户文件。Git 只读取裸仓库和归档，不执行 Skill 代码、hooks、filters 或 submodule。
- 私有仓库沿用已配置的非交互 Git 凭据；需要 `git` 位于 `PATH`。当前支持 GitHub 默认用户路径，不含 skills.sh 市场、ZIP 导入或自定义应用目录。
- 入口：托盘“Skills 管理”、`sean-toolbox show skills`，或管理窗口左侧“技能管理”（`Ctrl+5`）。CLI 支持 `skills list|status|discover|update|daily on|off`。

| 应用 | 默认部署目录 |
| --- | --- |
| Claude Code | `~/.claude/skills` |
| Codex | `~/.codex/skills` |
| Gemini CLI | `~/.gemini/skills` |
| OpenCode | `~/.config/opencode/skills` |
| Grok Build | `~/.grok/skills` |

## 主要功能

- **应用快捷键**：按需启动应用，切回或最小化窗口；同一应用有多个窗口时依次切换。默认示例为 WezTerm `F3`、VS Code `F4`、Chrome `F5`。
- **下拉与停靠**：窗口可停靠到四边，按工作区比例设置宽高，选择显示器与对齐位置。Windows 支持独立的 WezTerm 下拉终端、显示时置顶与失焦自动收起；Linux X11 支持置顶。
- **最近项目**：`Ctrl+Alt+Space` 打开独立搜索面板，按名称或路径搜索 VS Code Stable 的文件夹、工作区与远程项目，使用键盘或鼠标打开。
- **Skills 管理**：统一管理 Claude Code、Codex、Gemini CLI、OpenCode 和 Grok Build 的 Skills，支持仓库发现、本机导入、按应用复制部署、单项/全部更新和每日自动更新。
- **界面与配置**：跟随系统、浅色和深色主题，中文和英文界面，配置导入与导出，可选开机启动。
- **按需界面**：主界面关闭后默认退出界面进程，宿主继续处理托盘和全局快捷键。

## 下载与升级

| 系统与处理器 | 压缩包 |
| --- | --- |
| Windows x64 | `sean-toolbox-0.2.0-windows-x86_64.zip` |
| macOS Apple 芯片 | `sean-toolbox-0.2.0-macos-aarch64.zip` |
| macOS Intel | `sean-toolbox-0.2.0-macos-x86_64.zip` |
| Linux x64 | `sean-toolbox-0.2.0-linux-x86_64.tar.gz` |

每份归档附同名 `.sha256` 校验文件。完整解压后按 `INSTALL.md` 启动；Windows 保留两个 `.exe` 在同一目录，macOS 保持整个 `.app` 完整。升级前从托盘退出工具箱，再替换程序；已有配置和 Skills 管理数据继续保留。

当前 Windows 包未做 Authenticode 签名；macOS 包未做 Developer ID 签名及 Apple 公证。macOS 窗口控制需要辅助功能权限，Linux 外部窗口控制使用 X11。

[使用指南](https://github.com/sean2077/sean-toolbox/blob/v0.2.0/docs/USAGE.md) · [验证记录](https://github.com/sean2077/sean-toolbox/blob/v0.2.0/docs/VALIDATION.md)