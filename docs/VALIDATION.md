# 0.1.0-beta.1 验证记录

日期：2026-10-29。验证环境为 Linux x86_64 容器，Rust 1.99.1、Slint 1.18.1。
GUI 测试运行于隔离的 Xvfb + Openbox；测试使用临时 HOME、配置和显示服务，不改用户桌面。

## 已完成

| 检查 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| Linux workspace Clippy，全部 targets，`-D warnings` | 通过 |
| Rust 单元 / contract 测试 | 55 项通过 |
| 独立 X11 原生窗口测试 | 通过；默认测试中标为 ignored，由隔离脚本单独执行 |
| Linux release 两个程序构建及真实打包 | 通过；归档内程序的哈希与构建结果一致 |
| Windows x86_64 GNU 全 workspace / all-targets 检查及严格 Clippy | 通过，包含 UI、SQLite、宿主、窗口和 IPC |
| macOS ARM / Intel 宿主及窗口、IPC 的交叉编译检查 | 通过 |
| macOS ARM 宿主、Intel 宿主的严格 Clippy | 通过 |
| 四个平台的离线依赖许可收集 | 通过 |

Windows 的上述结果是 GNU 目标上的类型与静态检查，不是 MSVC 链接验证或真实 Windows 桌面验收。
macOS 完整 GUI 的原生构建由仓库 CI 执行，权限、焦点和原生文件对话框仍需对应桌面验证。
GitHub Actions 的状态以具体提交下的工作流结果为准。

## 真实窗口及宿主

`crates/toolbox-windows/tests/run-x11-smoke.sh` 创建真实 xterm 窗口，验证枚举、激活、
最大化、最小化/恢复、双窗循环、末尾全部最小化、关闭后的旧句柄拒绝，以及 Engine 启动新窗。

`scripts/smoke_host.py --binary target/release/sean-toolbox --measure` 验证：

1. 通过真实 F8 按键启动应用，第二次按键最小化窗口。
2. 重复快捷键和过期修订版本保存被拒，磁盘文件和当前配置不变。
3. F8 改为 F9 后，新快捷键生效；独立 X11 客户端能够立即注册旧 F8，证明确实释放。
4. 再次启动保留原宿主 PID。
5. 打开诊断、Esc 收起、idle=0 退出界面；无 UI 僵尸进程或 socket 残留，宿主仍工作。
6. 再次打开 UI 后退出宿主；收到完整 JSON 回复，宿主及其 UI 退出，socket 清理完成。

第 3 项发现并推动修复了 global-hotkey 0.9.0 的 X11 解绑缓冲问题；补丁及来源记录见
`vendor/global-hotkey/PATCH.md`。该补丁只更改 Linux X11 解绑函数。

## 界面交互

已用原生 Slint 窗口检查中文、英文的项目、设置、应用编辑、诊断和导入预览页面。
验证搜索、↑↓/Enter、Esc、设置冲突拒绝、多启动候选参数与 cwd 保存、失败启动恢复 UI、
同进程重复唤起、idle 2 秒及 idle 0 释放、INI 预览合并，以及 JSON 导出与宿主配置一致。
原生文件选择测试使用 Zenity；缺少文件选择后端时也验证了错误提示。

项目测试使用临时 VS Code SQLite 历史和记录 argv 的 CLI fixture，以验证数据语义及参数边界；
不把它等同于真实 VS Code 窗口焦点的系统验收。

## 资源采样

release 宿主在 UI 完全退出后采样 10 秒，共 51 次：

| 指标 | 本次结果 |
| --- | --- |
| RSS 最小值 | 7.480 MiB |
| RSS 平均值 | 7.565 MiB |
| RSS 最大值 | 7.594 MiB |
| 占单核 CPU | 本次 10 秒窗口内 `/proc` tick 增量为 0 |

此环境为 Linux X11 / Xvfb，没有可用的桌面托盘宿主。测量只覆盖宿主 PID，不包含 UI、
Xvfb、窗口管理器或被启动应用。采样分辨率、真实托盘、系统库和平台均会影响结果；
不能将短窗口内的零 tick 推断为永久零 CPU，或作为 Windows/macOS 的资源承诺。

复测方法：先构建两个 release 程序，再运行上述 `smoke_host.py --measure`。
