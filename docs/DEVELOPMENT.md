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

界面侧栏、运行状态页和 `sean-toolbox --version` 显示构建时的 `git describe --tags --dirty`，例如某次构建的 `v0.2.0-26-gaf25bd0`；正好位于发布 tag 时显示该版本号。没有 Git 信息或可达 tag 时（源码包、CI 的浅克隆构建）回退为 `v<Cargo 版本>`，因此发布资产仍显示准确版本。构建脚本在提交、切换分支、tag 或暂存区变化后重新计算；仅修改未暂存文件时，`-dirty` 标记在下次 Git 刷新暂存区后更新。

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

### 无桌面的组件布局预览

[`scripts/render_ui.py`](../scripts/render_ui.py) 编译并调用
[`render_ui` 示例](../crates/toolbox-ui/examples/render_ui.rs)，通过 Slint 软件渲染器直接绘制
生产组件。数据为显式示例，不读取用户配置或本机 Skills，不启动宿主、IPC 或外部应用。
Python 入口仅使用标准库，把渲染结果保存为 PNG，并记录源提交、语言、主题、逻辑尺寸、缩放和
Slint 声明的尺寸约束。默认拒绝超出组件最小/最大尺寸的截图，并核对实际窗口与像素缓冲区尺寸。

```sh
python scripts/render_ui.py --output target/ui-previews
python scripts/render_ui.py --scenes skills settings launcher --languages en zh-CN --themes light dark --sizes 800x580 --scales 1.5 2 --offline
python scripts/render_ui.py --scenes apps settings --languages en --sizes 1120x780 --verify-appearance-controls --offline
```

默认覆盖管理窗口各页、项目面板及导入/确认弹窗，中英文、浅深色、1120×780 与 800×580。
Skills 场景包括详情、问题恢复、本机导入、链接来源、带隐藏选择的批量操作和操作状态，
其中示例部署数量与应用勾选保持一致。`apps-advanced` 与 `quake` 展示已展开或滚动到对应内容的状态。
项目面板预览最大为 760×432，`launcher-details` 展开完整路径。可用 `--scenes`、`--languages`、`--themes`、`--sizes` 和
`--scales` 缩小检查范围。系统缺少中文字形时，使用 `--font-family` 选择已安装的 CJK 字体，
或结合 `--font-file` 仅为本次预览注册一个本地字体；工具不会下载或修改系统字体。
对比旧版未支持的尺寸时才使用 `--allow-unsupported-size`；清单会明确记录该尺寸是否超出组件约束，
这类对照图不能作为该尺寸适配通过的证据。

`--verify-appearance-controls` 读取 `smoke_appearance.py` 的同一套逻辑坐标，向真实 Slint
组件发送指针事件，核对主题、放弃设置、应用选择及停靠预设等关键目标。清单记录通过的控件，
用于尽早发现布局变更让原生脚本点错位置；它不调用宿主，也不替代原生持久化和焦点验收。
该检查使用英文、1120×780 和平台默认字体，与 Linux 原生脚本一致；`--font-family` 会改变文字度量，
因此自选字体的布局预览应与原生坐标检查分别运行。

这些预览用于发现文字裁切、层级和布局问题，不证明原生焦点、屏幕阅读器、文件选择器、
真实显示器缩放或宿主交互已经通过验收；对应平台仍使用前述集成测试及实机检查。

开发时可用绝对路径环境变量 `SEAN_TOOLBOX_CONFIG` 指定独立配置。宿主单实例按当前用户隔离，桌面集成测试使用临时用户目录和独立显示服务。

### Windows 软件渲染重绘回归

管理窗口与项目面板共用 winit 软件渲染后端。Slint 1.18.1 未包含上游
[#13568](https://github.com/slint-ui/slint/pull/13568) 的 Windows 失效区域修复，
窗口尺寸、缩放或恢复后可能残留未绘制的底部或右侧区域。根 Cargo patch 仅替换
`i-slint-backend-winit`，回移该修复，并在原生尺寸变化后通过事件队列合并一次
客户区失效通知，让后续原生绘制覆盖新区域。重复的相同尺寸通知不会追加重绘；
最小化后的恢复也纳入处理。来源、校验值和移除条件见
[`vendor/i-slint-backend-winit/PATCH.md`](../vendor/i-slint-backend-winit/PATCH.md)。

在具备桌面的 Windows 会话运行：

```sh
cargo build --locked -p toolbox-ui --example repaint_probe
python scripts/smoke_windows_repaint.py --binary target/debug/examples/repaint_probe.exe
```

该脚本用独立静态窗口检查真实 Win32 客户区：先直接写入哨兵色，确认无失效区域的内部
重绘不会覆盖它，再分别请求底部与右侧区域重绘并核对原像素恢复。每组还覆盖窗口放大和
最小化后恢复，默认检查浅深主题及 100% / 150% / 200% 缩放，共 6 组、18 个阶段。
脚本仅操作同时匹配随机标题与自建进程 PID 的窗口，不启动宿主、不读取用户配置；失败或
超时会回收测试进程。非 Windows 平台明确失败，不把未执行记为通过。

CI 的 Windows 构建在普通测试后执行此检查。缩放矩阵通过 `SLINT_SCALE_FACTOR` 提供，
它覆盖物理失效区域与逻辑坐标的换算，不代替两台真实显示器之间的 DPI 切换验收。
无桌面组件截图也不能替代这项原生后端检查。

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

Rust 与 Slint 都需要显示的无参数标签（例如 Enabled、Quake 边缘、主题名）统一定义在 [`common-messages.slint`](../crates/toolbox-ui/ui/common-messages.slint) 的 `ui.common.*` 上下文中。Slint 通过 `SharedMessages` 属性读取，由提取器写入 `ui.pot`，构建时为 Rust 生成 `messages::common_*` 构造函数。独立于具体控件的定义可保留 Rust 格式化模型使用的文案，避免布局调整时误删生成 API。同一概念只有一条 PO 条目；共享条目不得含占位符或复数。

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
