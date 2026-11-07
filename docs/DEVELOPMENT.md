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
| [toolbox-i18n](../crates/toolbox-i18n/README.md) | 共享 PO 资源校验、构建期消息生成、显式语言选择；运行时不依赖 Slint |
| `toolbox-ipc` | 当前用户的本地 socket / named pipe 通信、消息大小和超时控制 |
| [toolbox-windows](../crates/toolbox-windows/README.md) | 应用窗口切换和 Quake 状态、Win32 / Accessibility / X11 适配 |
| [toolbox-projects](../crates/toolbox-projects/README.md) | VS Code 历史读取、项目身份、排序和打开方式 |
| [toolbox-skills](../crates/toolbox-skills/README.md) | Skills 来源、发现、集中存储、按应用部署、更新与独立状态 |
| `toolbox-host` | 单实例、全局快捷键、托盘、配置保存、窗口 worker、Skills worker 和界面进程管理 |
| [toolbox-ui](../crates/toolbox-ui/README.md) | Slint 项目面板与管理窗口、编辑状态、项目查询和文件选择 |

`sean-toolbox` 是常驻宿主，`sean-toolbox-ui` 是按需启动的界面进程。Slint、SQLite 和项目查询位于界面一侧，宿主负责快捷键、托盘及窗口控制。窗口 worker 接到请求后执行操作，宿主主循环等待事件。

模块通过配置、命令和数据模型协作。新增功能可放在独立 Rust crate 中，按需要接入宿主或界面。当前模块随应用一起构建和分发。

Skills 的网络和文件操作在宿主的独立 worker 串行执行，桌面线程通过有界队列投递命令，立即返回接受状态；`GetSkills` 读取缓存快照，不等待下载。每日调度在关闭界面后继续运行，关闭自动更新时阻塞等待命令，开启后按持久化期限触发；定期复核系统时间以处理休眠恢复。退出宿主会取消网络子进程并等待 worker 收尾。它不链接 Slint 或 SQLite，也不添加 OS 定时任务。

## 国际化与翻译维护

界面与托盘支持简体中文 `zh-CN` 和英文 `en`，配置默认值与已有文件兼容。Slint 使用原生 `@tr` 与编译期打包的 gettext PO；Rust 使用 `toolbox-i18n` 从同一份 PO 生成带参数的 `Message`。宿主不链接 Slint，运行时不解析资源，也不要求用户安装 gettext 或携带翻译目录。

| 资源 | 维护方式 |
| --- | --- |
| `.slint` 中的 `@tr("ui.context" => "English source")` | 静态界面的英文源文与稳定上下文；以语义命名，组件改名不影响翻译；`ui.common.*` 表示 Rust 与 Slint 共用的无参数消息 |
| [`locales/ui.pot`](../locales/ui.pot) | 官方 `slint-tr-extractor` 提取的界面模板，不手工修改 |
| [`locales/rust.pot`](../locales/rust.pot) | Rust 专属消息的英文模板；`msgctxt` 使用 `rust.snake_case`，生成同名构造函数；共享标签放在 `ui.common.*`，不在此重复 |
| [`locales/zh-CN/LC_MESSAGES/toolbox-ui.po`](../locales/zh-CN/LC_MESSAGES/toolbox-ui.po) | 界面、Rust 消息和托盘共用的简体中文翻译；可用 PO 编辑器维护 |

首次维护翻译需安装与项目 Slint 版本一致的提取器及 GNU gettext 工具（`msgcat`、`msgmerge`、`msgfmt`）：

```sh
cargo install slint-tr-extractor --version 1.18.1 --locked
python scripts/translations.py update
# 在中文 PO 中补全新增/更改的条目
python scripts/translations.py check
```

也可使用 `make translations` 和 `make translations-check`。普通构建不需要上述命令行工具。每次构建都会检查重复、缺失、空白、fuzzy 条目和占位符契约；CI 额外重新提取界面源文，检查 `ui.pot` 是否过期。`update` 不使用模糊匹配自动采纳旧译文；检查只比较翻译语义，不因提取日期和源行号变化报错。

新增 Rust 消息时，在 `rust.pot` 定义完整句子，再更新中文 PO。通过 `messages::config_exported(path)` 等生成函数取得 `Message`，在呈现时调用 `.render(locale)`；不要在业务逻辑中传入成对的中英文字符串。Rust 模板只接受 `{name}` 命名参数与 `{{` / `}}` 字面括号；四个及以上参数的消息生成具名参数结构体（例如 `DiagnosticsSummaryArgs`），调用方按字段名构造，重命名占位符不会静默换参；更小的消息保持按名字排序的位置参数。含 `msgid_plural` 的消息把 `count: usize` 固定为首参数，其余参数仍按名字排序；英文区分 1 与其他数量，中文使用单一复数形式。

Rust 与 Slint 都需要显示的无参数标签（例如 Enabled、Quake 边缘、主题名）定义在 `.slint` 的 `ui.common.*` 上下文中，由提取器写入 `ui.pot`，构建时为 Rust 生成 `messages::common_*` 构造函数。这样同一概念只有一条 PO 条目，两个界面不会各自漂移；共享条目不得含占位符或复数。

Slint 使用 `{0}`、`{1}` 等参数和原生复数语法。整句话属于同一模板，翻译可以调整参数顺序；用户路径、Skill 描述、仓库地址等值不参与模板解析。英文源文是默认回退；受支持的中文条目必须完整，缺翻译会使构建失败。增加新语言还需扩展 `Locale`、配置校验与对应复数规则。

语言预览统一通过 UI 控制器更新：组件创建后、显示前选择 Slint bundled locale，并重新呈现保留了消息标识与参数的状态、确认框、本地错误，以及由原始模型生成的项目统计、导入摘要和 Skills 详情。后台任务返回结构化的 UI 本地消息，到达界面时才选择语言；放弃设置恢复已保存语言，保存成功后宿主重建托盘。用户输入及应用草稿不会因此重置。

当前边界：CLI 帮助、底层配置/INI 校验、宿主和窗口后端的协议诊断仍使用现有原文，其中包含项目自身的中英并列提示。界面保留这些诊断，不按文本或 `" / "` 分隔符猜测语言。后续若迁移这些消息，应在生产者和 IPC 中引入稳定消息 code/args，同时保留原始 detail，避免破坏诊断内容及协议兼容性。

方案依据：[Slint 翻译机制](https://docs.slint.dev/latest/docs/slint/guide/development/translations/)与[官方翻译最佳实践](https://docs.slint.dev/latest/docs/slint/guide/development/best-practices/)。构建期 PO 解析复用项目已锁定的 `rspolib 0.1.2`；没有新增翻译运行时。

## 修改与维护

- 手写代码位于 `crates/*/src/`、`crates/toolbox-ui/ui/main.slint` 和 `scripts/`；界面生成代码通过构建更新。
- 单元测试随模块放置，跨模块测试放在 `crates/*/tests/`。
- `vendor/global-hotkey` 是带 X11 修复的依赖副本，来源和改动见 [补丁记录](../vendor/global-hotkey/PATCH.md)。
- 依赖许可补充文本与版本映射位于 [licenses/dependencies](../licenses/dependencies/README.md)。升级依赖时同步核对。
- 产品操作与配置写在 [使用指南](USAGE.md)，模块接口写在对应 crate README，测试环境和结果写在 [验证记录](VALIDATION.md)。开发约定见 [AGENTS.md](../AGENTS.md)。
