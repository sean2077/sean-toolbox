# 开发指南

[返回首页](../README.md) · [打包说明](../packaging/README.md) · [验证记录](VALIDATION.md) · [项目术语](../CONTEXT.md)

## 环境与构建

工具链由 `rust-toolchain.toml` 固定，依赖版本保存在 `Cargo.lock`。以下命令均在仓库根目录执行。

| 平台 | 构建环境 |
| --- | --- |
| Windows | Rust MSVC 工具链、Visual Studio C++ 构建工具和 Windows SDK |
| macOS | Rust、Xcode Command Line Tools；CI 的最低部署目标为 macOS 11 |
| Linux | Rust、C/C++ 链接工具，以及 X11/Wayland、字体、键盘和 D-Bus 开发库；Ubuntu 安装命令见 [CI](../.github/workflows/ci.yml) |

Windows CI 使用 MSVC。若本地 rustup 默认 host 为 `x86_64-pc-windows-gnu`，可为仓库设置与固定 channel 同版本的 MSVC override；继续使用 GNU 工具链则需要包含 `dlltool` 的完整 MinGW-w64。

```sh
cargo build --locked -p toolbox-host -p toolbox-ui
./target/debug/sean-toolbox
```

发布构建使用：

```sh
cargo build --locked --release -p toolbox-host -p toolbox-ui
```

两个可执行文件配套运行。构建产物位于 `target/`，发行归档位于 `dist/`。打包脚本需要 Python 3.11+，步骤见 [打包说明](../packaging/README.md)。

根目录的 `Makefile` 提供 `make check`、`make build-release`、`make run` 和 `make package` 等目标，运行 `make help` 查看列表。Makefile 使用 POSIX shell，Windows 可在 Git Bash 中运行。

## 检查与测试

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
```

CI 在 Windows x64、macOS ARM/Intel、Linux x64 上执行格式检查、Clippy、测试、release 构建、许可收集和打包。Linux 另有窗口与界面集成测试，完整矩阵见 [工作流](../.github/workflows/ci.yml)。

### Linux 桌面集成测试

测试使用独立的 Xvfb、Openbox、xterm 和临时用户目录。先构建 release 宿主与界面，再运行：

```sh
bash crates/toolbox-windows/tests/run-x11-smoke.sh
python scripts/smoke_host.py --binary target/release/sean-toolbox
python scripts/smoke_appearance.py --binary target/release/sean-toolbox
```

| 入口 | 覆盖内容 | 可选参数 |
| --- | --- | --- |
| `run-x11-smoke.sh` | 窗口枚举、激活、最小化、循环与 Quake 停靠 | — |
| `smoke_host.py` | 全局快捷键、配置保存、单实例、界面释放和退出清理 | `--measure`：采样空闲宿主资源 10 秒 |
| `smoke_appearance.py` | 主题、图标、草稿、关闭与重新唤起、Quake 预设及焦点处理 | `--scale 1.5`：150% 缩放；`--screenshots <目录>`：保存截图，需要 ImageMagick |

默认 `cargo test` 会跳过标为 ignored 的 X11 用例，由上述窗口测试脚本单独运行。测试中的 VS Code SQLite 数据和 CLI fixture 用于检查历史解析、打开目标及参数；桌面焦点、权限提示和多显示器交互在相应系统上单独验收。按日期记录的结果与资源采样见 [验证记录](VALIDATION.md)。

开发时可用绝对路径环境变量 `SEAN_TOOLBOX_CONFIG` 指定独立配置。宿主单实例按当前用户隔离，桌面集成测试使用临时用户目录和独立显示服务。

## 模块结构

| crate | 职责 |
| --- | --- |
| `toolbox-core` | 配置模型、校验、原子存储、INI 导入和 IPC 消息定义 |
| `toolbox-ipc` | 当前用户的本地 socket / named pipe 通信、消息大小和超时控制 |
| [toolbox-windows](../crates/toolbox-windows/README.md) | 应用窗口切换和 Quake 状态、Win32 / Accessibility / X11 适配 |
| [toolbox-projects](../crates/toolbox-projects/README.md) | VS Code 历史读取、项目身份、排序和打开方式 |
| `toolbox-host` | 单实例、全局快捷键、托盘、配置保存、窗口 worker 和界面进程管理 |
| [toolbox-ui](../crates/toolbox-ui/README.md) | Slint 项目面板与管理窗口、编辑状态、项目查询和文件选择 |

`sean-toolbox` 是常驻宿主，`sean-toolbox-ui` 是按需启动的界面进程。Slint、SQLite 和项目查询位于界面一侧，宿主负责快捷键、托盘及窗口控制。窗口 worker 接到请求后执行操作，宿主主循环等待事件。

模块通过配置、命令和数据模型协作。新增功能可放在独立 Rust crate 中，按需要接入宿主或界面。当前模块随应用一起构建和分发。

## 修改与维护

- 手写代码位于 `crates/*/src/`、`crates/toolbox-ui/ui/main.slint` 和 `scripts/`；界面生成代码通过构建更新。
- 单元测试随模块放置，跨模块测试放在 `crates/*/tests/`。
- `vendor/global-hotkey` 是带 X11 修复的依赖副本，来源和改动见 [补丁记录](../vendor/global-hotkey/PATCH.md)。
- 依赖许可补充文本与版本映射位于 [licenses/dependencies](../licenses/dependencies/README.md)。升级依赖时同步核对。
- 产品操作与配置写在 [使用指南](USAGE.md)，模块接口写在对应 crate README，测试环境和结果写在 [验证记录](VALIDATION.md)。开发约定见 [AGENTS.md](../AGENTS.md)。
