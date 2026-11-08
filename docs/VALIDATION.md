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

## 2026-10-30：界面与交互迭代

日期按新加坡时间记录。本轮从 `4b93227` 开始，使用同一固定工具链
（Rust 1.99.1、Slint 1.18.1）和锁定依赖，在 Linux x86_64 中验证。

### 界面和实际操作

使用隔离的 Xvfb + Openbox、临时 HOME/配置、VS Code SQLite 历史和记录参数的
CLI fixture，运行真实宿主与 Slint 界面。README 的三张截图来自本轮原生窗口，
使用示例数据；它们不是 HTML 或图片生成的界面模型。

- 检查中文、英文的四个主页面，默认 1120×780 和最小 900×650 的布局。
- 检查应用编辑器的基本字段、滚动后的启动参数和工作目录、固定保存栏，以及中文确认框。
- 以 150% 缩放检查项目页，确认文字、图标、列表和底部操作仍正常布局。
- 实际点击导航、编辑、添加、保存和撤销按钮；单击项目只选择，双击打开。
- `Ctrl+1`–`4` 切页；`Ctrl+K` / `Ctrl+F` 聚焦搜索并打开正确的筛选结果；
  `Ctrl+S` 分别将设置和应用修改保存到真实宿主配置。
- 无搜索结果时按 Enter 不会打开旧条目。
- 切页保留应用草稿；撤销设置的确认操作保留应用草稿；确认框内 Tab / Shift+Tab
  保持焦点，后台保存快捷键不会越过弹窗生效。
- 原生 Zenity 文件选择器能打开，背景输入被阻止，取消后配置不变。
  本轮未将原生文件选择后的导入预览/合并流程计为通过；导入语义由已有 Rust 契约测试覆盖。

第一轮原生检查发现了自定义按钮焦点层吞掉鼠标点击、选中条位置偏移、图标未居中、
清空搜索按钮定位和 Slint 归属组件高度不足的问题，修复后重新检查。
新增软件窗口指针事件回归测试，直接发送鼠标按下/释放事件，验证按钮第一次点击即可执行、
禁用按钮不执行，没有通过直接调用按钮回调代替命中测试。

### 行为回归

新增 11 项回归测试，覆盖：从非项目页启动后读取历史、异步打开不收起新草稿、
重新载入不覆盖等待期间输入、独立撤销设置保留应用草稿、诊断失败使旧能力状态失效、
应用 ID 大小写不敏感去重、按钮鼠标命中，以及特殊 URI 路径、远程名称、根目录和反斜杠。
其中 4 项项目 URI 测试也在原始 `4b93227` 库上验证失败，修复后通过。

### 最终检查

| 检查 | 本轮结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | 通过 |
| `cargo test --locked --workspace` | 66 项通过；1 项 X11 原生用例在默认测试中忽略，另行执行 |
| 隔离 X11 原生窗口 smoke | 通过 |
| `cargo build --locked --release -p toolbox-host -p toolbox-ui` | 两个程序构建通过 |
| release 宿主/UI 生命周期 smoke | 通过：真实快捷键、解绑、配置事务、单实例、立即释放、退出及 socket 清理 |
| `git diff --check` | 通过 |

本地首次 release 链接遇到 `image-webp` 的零字节编译对象；只清理受影响的构建产物后，
按原锁文件、优化及 ThinLTO 设置重建成功，未通过更改依赖或关闭优化规避检查。

### 平台和资源边界

上述窗口、鼠标、快捷键和缩放证据均来自 Linux X11，不能代替 Windows/macOS
桌面权限、焦点策略、多显示器和原生文件对话框验收。三平台原生 CI 的实际结果以对应
提交的 Actions 为准。

本轮没有修改宿主架构、依赖版本或锁文件，也没有新增周期性轮询。
没有把本轮界面改版报告成已量化的内存或 CPU 性能提升。

## 2026-10-30：独立最近项目面板

本轮从 `61e7b3c` 开始，按
[Recentry `7eb5b8a`](https://github.com/sean2077/recentry/tree/7eb5b8ad2575fa58b3cdffc34a40381dd3311201)
的快捷键启动器交互实现独立 Slint 面板。保留四页管理窗口与已有配置。
工具链仍为 Rust 1.99.1、Slint 1.18.1；没有升级依赖版本。Linux UI 直接复用
已经锁定的 `x11rb 0.13.2`，`Cargo.lock` 只增加该 crate 在 UI 依赖列表中的一行。

### 真实面板与交互验收

使用真实宿主、Slint、隔离 Xvfb/Openbox、临时 HOME/配置、VS Code SQLite 历史与
记录 argv 的 CLI fixture。同一快捷键检查在改动前因唤出完整管理窗口而失败，
改动后确认打开标题为 `Sean Toolbox · Recent Projects` 的独立无边框面板。

- 默认逻辑尺寸 760×408；中英文界面、12 行可见结果和更长列表的滚动均检查。
- 真实 `Ctrl+Alt+Space` 冷启动和热唤起，自动聚焦、输入筛选、方向键选择、Enter 打开、
  Esc 收起、切到外部应用后收起均通过。首项处继续按向上键不会跳到末项。
- 面板已聚焦时重复快捷键，或按住快捷键 500 ms 后松开，面板保持可用，随后输入可以
  正常筛选；复用同一 UI 进程，没有产生第二个界面实例。
- 无匹配结果时 Enter 不打开旧条目；长名称、中文、远程 URI 与工作区结果均检查。
- 单击只选择、双击只发送一次打开请求；悬停显示完整名称和原样路径，提示不抢焦点。
- 将测试 CLI 暂时设为不可执行，打开失败时面板保留并显示错误；恢复权限后可重试成功。
- 从面板设置按钮打开管理窗口，输入未保存的应用配置，再唤起面板并打开项目；
  管理窗口的页面和草稿仍保留，随后保存可在宿主真实配置中读取到修改。
- 150% 缩放时面板为 1140×612 实际像素，在 2560×1800 测试显示中位于 (710, 594)，
  确认为居中；100% 时为 760×408，在 1600×1100 中位于 (420, 346)。

README 的 `project-launcher.png` 来自本轮最终原生窗口与示例数据。
检查中修复了占位文本位置、操作图标垂直对齐、悬停观察层被鼠标交互层遮挡，以及
X11 被动快捷键捕获被误认为真实失焦的问题。

### 回归与静态检查

新增 9 项 UI/controller 回归测试和 2 项协议兼容测试，覆盖两类管理草稿保留、
首次加载前输入与启动握手、可重试的打开失败、较早打开结果与新唤起会话、失焦保护、
重复唤起后的失焦处理、过期/空结果、现有确认框、负坐标显示区域居中，以及旧格式激活请求。

- `cargo fmt --all -- --check`：通过。
- `cargo clippy --locked --workspace --all-targets -- -D warnings`：通过。
- `cargo test --locked --workspace`：77 项通过，0 失败；1 项原生 X11 用例按原约定默认忽略。
- 上述原生 X11 用例在隔离环境另行执行：1 项通过。
- `cargo build --locked --release -p toolbox-host -p toolbox-ui`：两个优化构建通过。
- release 宿主/UI 生命周期 smoke：全部通过，覆盖真实快捷键、配置事务、单实例、
  立即释放、退出与 socket 清理。
- release 项目面板验收：冷/热唤起、连续与长按快捷键、搜索/打开、Esc/真实失焦、
  首项边界与打开失败后重试均通过。
- 独立代码审查未发现剩余的高、中等级正确性问题。
- `git diff --check`：通过。

第一次 release 构建出现 `E0463`：缓存的 `toolbox-windows` `.rmeta` 与 `.rlib`
时间不一致。只清理该 crate 的 7 个 release 生成文件（1.2 MiB），保持优化设置与依赖版本，
使用两个构建任务重建后通过，随后完成 release 原生验收。

### 验证范围

上述桌面交互证据来自 Linux X11。Windows 的前台显示器工作区与跨 DPI 放置、macOS
前台激活策略仍需对应系统实机验收；Linux 单显示器 150% 缩放不能替代混合 DPI 多显示器测试。
Windows 放置逻辑已通过源码审查，修复了先调整目标 DPI 尺寸再移动导致的重复缩放风险。
其他平台按面板所在显示器或主显示器居中，不宣称具备 Windows 的前台应用显示器选择行为。

CLI fixture 只证明打开目标和参数边界，不证明真实 VS Code 最终获得系统焦点。
本轮没有新增周期性的项目、状态或焦点轮询，也没有重新量化内存或 CPU 性能。

## 2026-10-30：明暗主题、应用图标、Quake 与界面内存释放

日期按新加坡时间记录。本轮从 `eeae7e1` 开始，保持 Rust 1.99.1、Slint 1.18.1
及锁定依赖版本。Quake 交互参考
[Windows Terminal Quake `5a30af2`](https://github.com/flyingpie/windows-terminal-quake/tree/5a30af25040de90dd833388036a15e93e3ead527)，
使用现有 Rust 原生后端独立实现，没有引入其 C# 运行时或焦点轮询。

### 配置与行为回归

- 旧配置继续使用普通窗口切换；主题默认跟随系统，Quake 按应用显式开启。
  未配置的界面缓存时间改为 0 秒，已有明确保存的缓存值保持不变。
- 主题即时预览覆盖管理窗口、最近项目面板和原生控件；保存后持久化，撤销恢复已保存主题，
  不丢弃另一个页面的应用草稿。
- Quake 覆盖四边停靠、对齐、工作区百分比尺寸、显示器选择、固定窗口重复唤起、
  句柄关闭后的重新绑定、重叠配置的窗口归属及临时置顶状态恢复。
- Windows 失焦收起使用系统前台窗口事件，通知合并；所属对话框保持显示。
  实际最小化前复查焦点，被取消的收起不会误改置顶状态。
- 手动关闭最后一个界面立即释放 UI，不等待自动收起缓存。另一可见界面、未保存草稿、
  对话框和进行中的操作仍受保护；退出已提交后的激活请求明确失败，宿主只在确认旧进程
  退出后启动新 UI，不杀死或复制仍然存活但暂时无响应的进程。
- 配置修订版本会阻止排队的旧窗口操作执行；窗口 worker 在宿主退出前完成临时样式清理。

### 编译、测试与打包

| 检查 | 本轮结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| Linux workspace / all-targets Clippy，`-D warnings` | 通过 |
| `cargo test --locked --workspace` | 108 项通过，0 失败；1 项 X11 原生用例按约定默认忽略 |
| 隔离 X11 原生窗口用例 | 另行执行，1 项通过 |
| `cargo build --locked --release -p toolbox-host -p toolbox-ui` | 两个优化构建通过 |
| release 宿主/UI 生命周期 smoke | 通过；真实快捷键、配置事务、解绑、单实例、UI 退出、socket 清理及宿主退出 |
| Windows GNU 全 workspace / all-targets check 与严格 Clippy | 通过，包含两个 EXE 的图标资源编译；最终行为修复后重新通过 Clippy |
| macOS Intel 的 `toolbox-windows`、`toolbox-host` / all-targets check 与严格 Clippy | 通过；使用独立交叉编译目录 |
| 图标生成器 `--check`、PNG/ICO/ICNS 解码与托盘 RGBA 数据检查 | 通过 |
| Linux release 许可收集和真实打包 | 通过；收集 441 个依赖包的许可，生成归档与 SHA-256 |
| Windows/macOS/Linux 打包布局检查 | 通过；非 Linux 布局使用替身程序，不计为原生可执行文件验收 |

`Cargo.lock` 只增加两个已锁定的 `cc` 构建依赖引用，没有改变依赖版本或校验值。
Windows 资源编译仅在构建时使用 SDK/MinGW；图标从单一 SVG 导出并提交，普通构建无需安装
Inkscape 或 Pillow。macOS 菜单栏使用单色模板图标，其余入口使用同一彩色应用标识。

本地 debug 构建曾遇到共享 target 中 `toolbox-windows` 的旧 `.rmeta` / `.rlib` 缓存不一致；
只清理该 crate 的 dev 构建产物后，按原锁文件重建成功。最终 release 构建未降低优化设置。
默认受限环境不允许测试绑定本地 socket，因此在允许隔离本地 IPC 的环境中执行完整测试，
没有弱化或跳过失败断言。

### 原生窗口与资源采样

新增 `scripts/smoke_appearance.py`，使用真实宿主、Slint、私有 Xvfb/Openbox、临时用户目录、
14 条示例项目及 VS Code SQLite/CLI fixture。脚本不访问现有桌面或真实项目历史；
原生颜色、图标和窗口尺寸通过 X11 读取，可选截图才需要 ImageMagick。

- 实际点击 System / Light / Dark，保存至宿主及磁盘，再退出并启动新的 UI 进程，验证主题保留。
  管理窗口和项目面板均检查原生像素，两者的 `_NET_WM_ICON` 内容一致，包含新的图标和透明背景。
- 未保存的主题预览同步影响两种界面；撤销恢复原主题，应用草稿仍能独立保存。
- Quake 编辑器开启模式并保存“右侧边栏”40% × 100% 预设，真实 F8 启动 xterm，
  第二次 F8 最小化同一个窗口，没有启动第二个实例。
- 60 秒缓存下，自动失焦收起保留同一 UI PID；Esc 或系统关闭按钮主动关闭最后一个面板、
  管理窗口后仍立即退出 UI 并清理 socket，宿主继续运行。
- 连续 8 次立即 Esc → Show 保持单一可响应 UI；对测试拥有的 UI 暂停进程后，宿主报告
  无响应而不复制或终止它，恢复后复用原 PID 并清除警告。

最终截图审查发现了原有放置逻辑在 150% 下的尺寸竞态：窗口管理器尚未完成尺寸请求，
面板已被锁定到旧的最小尺寸。新增尺寸断言在修复前真实失败，`WM_NORMAL_HINTS` 的
最小、最大尺寸均被错误锁定；修复改用 Slint 的在途尺寸跟踪，等待原生尺寸确认后才固定窗口，
并限制操作次数以避免窗口管理器拒绝尺寸时循环。同时将应用编辑器内容宽度约束为可视宽度，
修复缩放后的右侧裁切。

修复后的针对性 150% 检查覆盖 4 个冷启动 UI 进程、共 16 次唤起，面板均为 1140×612
实际像素，在 2560×1800 显示中位于 (710, 594)。应用编辑器的 1120×780 和 900×650
逻辑尺寸均检查，预览、预设、右侧字段和底部操作栏完整；需要更多内容时纵向滚动。

最后以修复后的 release 程序重新执行 100% 完整界面脚本及上述 150% 专项验收，两者均通过。
100% 面板为 760×408，位于 1600×1100 显示的 (420, 346)。尺寸和位置断言要求连续稳定
至少 160 ms，并已加入 CI 的 Linux 原生界面步骤。README 新增的浅色设置、深色设置、
深色项目面板及 Quake 编辑器四张截图均来自最终 release 原生窗口；没有修改截图内容。
最终代码重新通过格式、严格 Clippy、108 项 workspace 测试及 Windows GNU 严格 Clippy。

X11 原生用例在独立 Xvfb/Openbox 和临时用户目录中验证真实 xterm：普通窗口切换仍通过，
新增 Quake 的 60% × 50% 工作区停靠、已最大化窗口恢复后停靠、同一 XID 收起与恢复、
临时置顶在收起/重置后恢复，以及原本置顶的窗口保持原状态。

最终 release 宿主在 UI 完全退出后，使用 `scripts/smoke_host.py --measure`
采样 10.001 秒，共 49 次 RSS 读数：

| 指标 | 本次结果 |
| --- | --- |
| UI 进程 | 已退出，采样期间没有 UI 子进程 |
| 宿主 RSS 最小 / 平均 / 最大值 | 7.68 / 7.68 / 7.68 MiB |
| 宿主平均 CPU，占一个核心 | 0.7% |

这是本次 Linux X11/Xvfb 隔离环境中的宿主 PID 数据；环境没有真实桌面托盘宿主，
不包含显示服务、窗口管理器或目标应用。它证明 UI 已释放并给出剩余宿主的本次占用，
不构成与旧版的严格性能对照，也不能作为 Windows/macOS 内存或 CPU 承诺。

### 平台边界

Linux 原生桌面证据不能代替 Windows 的窗口焦点、所属对话框、任务栏、混合 DPI 和多显示器
实机验收。Windows GNU 检查不等于 MSVC 原生链接；macOS 本地检查没有 SDK，范围不含 UI、
Apple Silicon 或运行时权限。四平台原生构建、测试和打包的结果以该提交的 GitHub Actions 为准。
Linux 自动失焦收起、macOS 外部应用置顶及自动失焦收起没有实现，界面相应选项已禁用并说明。
目标应用最小尺寸及终端字符网格可能约束最终几何尺寸。

## 2026-10-30：CI 发现的 X11 失焦竞态

本轮从 `99167a4` 开始。
[该提交的原生 CI](https://github.com/sean2077/sean-toolbox/actions/runs/41038552971)
中 Windows MSVC、macOS ARM 和 macOS Intel 均完整通过构建、测试、许可收集及打包；
Linux 在新增的界面验收步骤发现项目面板未随真实失焦收起。此前本地带截图的验收没有触发
这一时序，Linux 的其他编译、测试及宿主生命周期步骤均通过。

### 复现与修复

在不截图的自然失焦流程中复现：X server 的真实输入焦点已经转到外部窗口，
`FocusOut` / `XI_FocusOut` 已送达，而窗口管理器稍后才更新 `_NET_ACTIVE_WINDOW`。
原实现只检查一次旧属性，误把这次失焦当作全局快捷键的被动抓取，之后没有新的事件补救。

另用完全私有的 Xvfb/Openbox，短暂暂停测试拥有的窗口管理器，保留旧活动窗口属性，
直接转移真实输入焦点，得到确定性的修复前失败；测试始终在 `finally` 中恢复该进程。

修复优先查询 `GetInputFocus`，识别窗口自身及原生子窗口；仅在 `PointerRoot` 没有具体
焦点窗口时使用窗口管理器的属性。延迟确认也优先采用原生查询，隐藏前复查可保护用户快速
切回面板的操作。Wayland 继续使用现有后端状态，没有新增常驻线程、周期性焦点轮询或依赖。

修复后的 debug 验收中，确定性延迟用例正确收起，另有连续 8 轮自然失焦及缓存复用通过。
原生脚本保留完整自动收起断言，并增加失败时的输入焦点、活动窗口和事件诊断；新增的
0.9 秒长按、3 次重复全局快捷键检查要求面板始终保持映射，防止靠关闭后重新打开掩盖误收起。
上述确定性窗口管理器延迟回归也纳入同一 CI 入口。

最终代码通过格式检查、workspace / all-targets 严格 Clippy 和 108 项 Rust 测试。
本地 release 链接首次遇到宿主的一个零字节 `core` LTO 生成对象；仅清理
`toolbox-host` 的 release 生成物后，按原锁文件与 ThinLTO/优化设置重建两个程序成功。
没有通过修改依赖或构建配置绕过该问题。

最后用重建后的 release 程序执行与 CI 相同的完整无截图入口，全部通过，包括新增的
确定性失焦回归、长按/重复快捷键保护及全部原有生命周期检查；私有测试进程均已清理。

## 2026-10-30：Windows WezTerm 独立下拉

本轮基于 `3c8d93b`。Windows WezTerm 的 Quake 配置在运行时使用专用 GUI 进程和
独立窗口类名；普通终端窗口不参与该配置的切换。收起改用 `SW_HIDE`，隐藏窗口仍可
按原身份找回，配置重置和正常退出宿主时恢复可见。已有配置只需开启对应应用的下拉选项。

对照 WezTerm 的 [start 参数](https://wezterm.org/cli/start.html)、
[窗口装饰](https://wezterm.org/config/lua/config/window_decorations.html) 和
[初始化源码](https://github.com/wezterm/wezterm/blob/e68a8ec27727fbbda15e07f0a0737cd5e16481f6/wezterm-gui/src/termwindow/mod.rs)
核对独立实例与启动时序：创建 HWND 后还需完成图形初始化，因此冷启动等待窗口实际显示，
已运行的隐藏终端则直接接管。默认启动等待为 10 秒；等待超时后的重试不会把正在初始化的
隐藏窗口当成旧会话，也不会在 20 秒启动冷却期内再创建进程。

本地 Linux 环境执行结果：

| 检查 | 结果与范围 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| workspace / all-targets 严格 Clippy | 通过，使用锁定依赖与离线缓存 |
| `cargo test --locked --offline --workspace` | 124 项通过，1 项 X11 桌面用例由下一项单独执行 |
| `run-x11-smoke.sh` | 通过；独立 Xvfb/Openbox 下验证真实 xterm，普通切换与 Quake 停靠行为保留 |
| Windows GNU 目标 core/windows / all-targets 严格 Clippy | 通过；包含新增 Windows 原生测试的编译检查 |

新增行为回归覆盖专用类名与普通终端隔离、隐藏后同一窗口复用、空标题、用户启动参数和
工作目录保留、初始化未完成、超时后的接管、失焦及所属对话框、显示失败回滚、隐藏失败和
清理重试。UI 测试覆盖候选编辑与程序浏览后的提示更新，保存时保留用户参数和快捷键。
首次完整测试被执行环境的 Unix socket 限制阻断；在允许测试临时本地 socket 的隔离执行中
重新运行全部测试通过，没有跳过 IPC 用例或调整断言。

Windows 增加两项使用私有 HWND 的原生测试，随 Windows CI 默认执行：检查隐藏/空标题
窗口、owned/tool 窗口过滤、普通进程匹配隔离，以及无激活的显示、隐藏、复用和退出恢复。
本地交叉检查没有运行这些 Windows 用例。真实 WezTerm 的前台激活、混合 DPI、多显示器
和任务栏行为仍需 Windows 桌面验收；本轮不包含滑入滑出动画或跨虚拟桌面迁移。

## 2026-10-30：0.1.0 发布准备与 Windows 原生测试消息循环

版本从 `0.1.0-beta.1` 提升为首个正式版 `0.1.0`。Cargo 生成的锁文件仅更新 6 个
workspace 包的版本，没有升级依赖。发布说明覆盖首次发布的完整能力。

`d473415` 的 [Windows CI](https://github.com/sean2077/sean-toolbox/actions/runs/41056740933)
发现生命周期测试没有为测试拥有的 HWND 运行消息循环：同一线程调用异步显示后进入等待，
无法处理 `ShowWindowAsync` 投递的窗口请求。隐藏窗口枚举测试通过，显示恢复测试超时。
修复后，窗口所属线程运行原生消息循环，独立 worker 调用真实后端，与实际宿主控制
外部应用窗口的线程关系一致。保留原来的超时和全部断言，没有修改生产显隐逻辑或跳过测试。

发布准备的本地检查：

| 检查 | 结果与范围 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| workspace / all-targets 严格 Clippy | 通过，锁定依赖 |
| `cargo test --locked --offline --workspace` | 124 项通过，1 项 X11 原生用例保留独立入口 |
| Windows GNU 目标 windows / all-targets 严格 Clippy | 通过，包含消息循环 fixture 的编译检查 |
| 发布脚本行为测试 | 27 项通过；使用临时 Git 仓库、文件和模拟 API，不访问真实发布 |
| 工作流 YAML 与发布连接检查 | 通过；四平台成功后才进入发布任务，PR 不进入发布任务 |

发布测试覆盖版本提升与预发行顺序、标签和说明不一致、未授权的事件入口、缺失或损坏的
资产、错误标签提交、首次发布、草稿中断与恢复、HTTP 错误分类，以及已公开版本只读校验。
发布脚本会在上传后下载全部 8 个资产，核对文件名、大小及 SHA-256，再公开草稿。
实际四平台原生测试、打包与 GitHub Release 的结果以本次 Actions 发布运行及其资产为准；
这些自动化检查仍不替代用户 Windows 桌面上的真实 WezTerm 多显示器与焦点体验验收。

## 2026-11-06：Skills 管理与每日更新

基于 `6f4a885` 实现 Skills 来源、发现、安装、本机导入、应用目录部署及宿主每日更新。
本地环境为 Linux x86_64、固定 Rust 1.99.1、Slint 1.18.1；测试均使用独立临时数据。

| 检查 | 结果与范围 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | 通过 |
| `cargo test --locked --workspace --no-fail-fast` | 155 项通过；3 项既有 IPC 用例在 Unix socket bind 处被环境以 `Operation not permitted` 拒绝；1 项既有 X11 用例仍由独立脚本运行。全量命令因此退出 101，不能记为全通过 |
| Skills 后端 | 19 项通过，包含真实临时 Git 仓库、更新、重启补跑、失败重试、本地修改保护和事务恢复 |
| 宿主 | 15 项通过，含后台接收/持久化、调度到期计算、初始化失败、IPC 状态大小和 CLI 契约 |
| UI | 39 项通过，含请求互斥、稳定 ID、草稿保留、操作确认、导入与关闭释放 |
| Windows GNU 目标的 Skills / all-targets 严格 Clippy | 通过；包含挂起启动、Job 纳管、恢复线程与 reparse point 分支的编译检查，不是 Windows 实机验证 |
| debug 宿主与 UI 构建 | 通过 |
| Python 发布脚本测试 | 27 项通过 |
| 中英文 Slint 布局 | 实际软件渲染器检查默认 1120×780 与最小 900×650、安装/发现/仓库视图；[截图](images/skills-manager.png)使用明确标注的示例数据 |
| `scripts/smoke_host.py` | 已扩展 Skills 的 IPC 导入、部署、每日开关、关闭界面、冲突保护与卸载流程；本地 Xvfb 因无法创建监听 socket 而启动失败，流程未执行完成 |

独立审查推动修复了未结束事务被后续空更新推进 generation、旧副本部分清理后恢复阻塞、
相同内容的未知目录被接管、大清单与聚合错误超过 IPC 限额、无关应用链接目录阻止管理器启动、
刷新后的旧发现条目、本机共享目录扫描及嵌套 Skill 示例边界。相应回归纳入测试。

首轮 [CI](https://github.com/sean2077/sean-toolbox/actions/runs/42233480183) 的 Linux 与
macOS ARM 全量测试通过；Windows 的真实 Git 测试发现 `git archive --output` 不接受
canonicalize 后的 Win32 verbatim 路径。现改为在受控临时工作目录中使用相对仓库与归档
参数，Rust 文件访问和限额监控仍保留完整路径，不剥离 UNC 或长路径前缀。既有 Git
测试继续经过 canonicalize，且临时目录增加空格，直接覆盖这一回归。

GitHub Actions 沿用 Windows x64、macOS ARM/Intel、Linux x64 的完整矩阵。IPC 与桌面
集成检查的实际远端结果以本 PR 对应提交的 Actions 为准；未降低、跳过或修改既有测试来
绕过本地 socket 限制。本次没有执行 Windows 实机的文件占用、junction、休眠恢复或文件
选择器验收。

## 2026-11-07：Skills PR 复审修复

对 PR #1 的复审修复了以下行为：显式开启每日更新不再被此前全量更新失败留下的重试回退
延迟，宿主接受命令时返回预览状态而不再让界面先回退；仓库根目录的 Skill 不再因
`git archive` 的 pax 全局头被判为阻塞；单个无法读取 `SKILL.md` 的条目会跳过并提示，
不再使整个仓库发现失败；单个不可读的应用根目录不再中止本机扫描；中断事务的应用目录被
删除后恢复会跳过该项并保留其余操作；删除或禁用不会重建用户移除的应用目录；启动时清理
中断遗留的 `checkout-*`、`skill-*` 暂存目录并保留其他文件；Windows 显示路径去掉
verbatim 前缀；来源标签去掉 `.git` 后缀；状态上限提高到 2 MiB；恢复日志容量按
`SkillApp::ALL` 推导；发现和本机列表标记已管理的同名目录；Git 环境变量按大小写不敏感
清理；frontmatter 中缩进的 `---` 不再提前结束；状态写入失败不再把已提交的操作误报为
失败；发现缓存只保留解压树。文档补充预置的官方 `anthropics/skills` 来源及跳过条目说明。

本地环境为 Windows 11、固定 Rust 1.99.1 MSVC 工具链，复用主工作树的 `target`：

| 检查 | 结果与范围 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | 通过 |
| `cargo test --locked -p toolbox-skills -p toolbox-host -p toolbox-ui` | 通过：Skills 29、宿主 15、UI 42 |
| `cargo test --locked --workspace --no-fail-fast` | 除既有 `toolbox-windows` 原生窗口用例 `native_quake_hides_reuses_and_recovers_without_activation` 外全部通过；该用例在本机以“窗口未完成置顶”失败，在未修改的 `main` 上同样复现，属于本机桌面/会话限制，未记为 PR 回归，命令退出 101 |
| 新增回归 | 仓库根 Skill；坏条目跳过；应用根不可读；删除目录后的恢复；删除/禁用不重建目录；重试回退与打开重试；接受状态预览；Windows 显示路径；暂存目录清理；状态上限与 2 MiB 保存；Git 环境变量大小写；缩进 `---`；普通扫描容错；来源标签与空状态；同名目录标记 |

Linux/macOS 与桌面 smoke 的结果以本 PR 对应提交的 Actions 为准；本次没有执行 Windows
实机的文件占用、junction、休眠恢复或文件选择器验收。

## 2026-11-07：0.2.0 发布准备

版本从 `0.1.0` 提升到 `0.2.0`（Skills 管理属于新功能）。Cargo 生成的锁文件只更新 7 个
workspace 包的版本，没有升级依赖。发布说明覆盖 Skills 管理、原有功能和 0.2.0 下载资产。

发布准备的本地检查：

| 检查 | 结果与范围 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | 通过，锁定依赖 |
| `cargo test --locked --workspace --no-fail-fast` | 除既有 `toolbox-windows` 原生窗口用例 `native_quake_hides_reuses_and_recovers_without_activation` 外全部通过；该用例在本机因“窗口未完成置顶”失败，在未修改的 `main` 上同样复现，属本机桌面/会话限制，命令退出 101 |
| 发布脚本行为测试 | 27 项通过；使用临时文件与模拟 API，不访问真实发布 |
| `cargo metadata --locked` | 通过；锁文件仅更新 workspace 包版本 |

四平台原生测试、打包与 GitHub Release 的结果以本次推送后的 Actions 发布运行及其资产为准。
发布工作流在 `main` 的版本变化推送后创建带注解的 `v0.2.0` 标签，四平台构建/测试通过后
上传并核对 8 个资产，再公开 Release。本次没有执行 Windows 实机的文件占用、junction、
休眠恢复或文件选择器验收。

## 2026-11-07：本机链接导入与链接条目

本机添加（文件夹选择或“本机技能”导入）增加“链接（不复制）”选项：

- 链接技能不创建主副本，保留源目录，并在所选应用目录创建符号链接；源目录修改立即生效，不参与远程更新。已有指向同一来源的应用链接会被接管，指向其他位置的路径会被拒绝。
- 停用或卸载只移除工具箱登记的链接，源目录始终保留；应用链接被替换时操作停止并报告。
- 本机扫描会把应用目录中的符号链接/reparse 条目标记为链接并显示目标。
- Windows 创建链接需要 Developer Mode 或符号链接权限；失败时返回明确错误。

本地门禁（Windows 11、Rust 1.99.1 MSVC）：

| 检查 | 结果与范围 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | 通过 |
| `cargo test --locked -p toolbox-skills -p toolbox-host -p toolbox-ui` | 通过：Skills 29（Windows；Unix 另有 2 项链接测试）、宿主 15、UI 44 |
| `cargo test --locked --workspace --no-fail-fast` | 除既有 `toolbox-windows` 原生窗口用例在本机因置顶确认失败外全部通过；该用例在未修改的 `main` 上同样失败，属本机桌面/会话限制，命令退出 101 |
| Windows 实机链接导入 | 通过：将 `ob` 以链接方式导入并接管 `.claude/skills/ob`、`.codex/skills/ob` 指向 Obsidian vault 的现有链接；状态为 5 个复制副本加 1 个链接技能，vault 未被复制或修改 |

该改动位于 `v0.2.0` 发布后的 `main`，不包含在 0.2.0 Release 资产中；后续版本将包含。
Windows 的符号链接权限、源目录被移动/删除后的恢复仍需在对应桌面环境验收。

## 2026-11-07：中英翻译资源与语言预览

基于 `main` 的 `1e4a70761db33833776146d6c85ebeb23432e636`，在独立工作树重构界面、托盘和 UI 自有消息。
环境为 Ubuntu 24.04 容器、Rust/Cargo 1.99.1、Slint 1.18.1；系统开发依赖解压在任务专用目录，未修改用户配置。
提交前已同步到 `75034efe57910c4db7c6782215c9c2a1de54f0d7`，保留上游 Skills 页面尺寸修复；
更新翻译源位置后重新执行 fmt、Clippy、工作区测试和翻译检查，结果与下表一致。

| 检查 | 结果与范围 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | 通过 |
| `cargo test --locked --workspace --no-fail-fast` | 205 通过、3 失败、1 ignored；失败均为下述既有 IPC 环境限制，命令退出 101 |
| 未修改 `main` 的同环境基线 | 187 通过、相同 3 项 IPC 失败、1 ignored；fmt 与 Clippy 通过 |
| `python scripts/translations.py check` | 通过；官方提取器核对 231 条界面源文，资源覆盖 126 条 Rust 消息与 231 条界面文案，`msgfmt` 检查通过 |
| 翻译更新流程 | 在隔离副本实跑 `update`，更新前后全部 357 个 gettext 条目相同（排除元数据）；新增未译项需补全后再通过检查 |
| `python -m unittest discover -s scripts -p 'test_*.py' -v` | 27 项通过 |
| `cargo tree --locked --offline -p toolbox-i18n --edges normal` | 仅该 crate 自身，无第三方运行时依赖 |
| `git diff --check` 与 Python 语法检查 | 通过 |

3 项失败为 `toolbox-ipc` 的 `owner_only_socket_round_trips_and_is_removed_on_drop`、
`stale_owner_socket_is_recovered_but_active_socket_is_rejected`、
`unix_transport_rejects_relative_and_oversized_messages`。均在创建 Unix socket 时返回 `EPERM / Operation not permitted`；
独立 Python `AF_UNIX` bind 也复现，未跳过或弱化这些用例。

新增 18 项回归覆盖资源缺失/重复/fuzzy/占位符错误、真实生成代码编译、英文 0/1/多项及中文复数、
英文语序调整后的参数 API 稳定性、Unicode/Windows 路径/字面括号、Slint 编译绑定的语言预览及撤销、
已有项目统计/导入摘要/消息的重新翻译、后台错误采用当前语言、Skills 请求错误保留及成功刷新后的清理。
UI 行为测试使用真实 Slint 组件与软件渲染测试平台，不访问本机用户数据。

本地未执行 Windows/macOS 实机桌面或 Linux 原生桌面 smoke。四平台构建、完整 IPC 与 Linux 原生窗口/界面 smoke
以 PR 对应提交的 Actions 结果为准；测试平台通过不代表完成跨系统字体、缩放或文件选择器的人工视觉验收。

## 2026-11-07：Skills 默认软链部署与配置分组

在 `feat/skills-symlink-config` 工作树验证。环境为 Windows 11 Pro 10.0.26200、Rust/Cargo 1.99.1 MSVC、Slint 1.18.1；
未修改用户配置，未执行真实 GitHub 网络下载。

| 检查 | 结果与范围 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | 通过 |
| `cargo test --locked --workspace --no-fail-fast` | Windows 下 208 项通过、0 失败 |
| 翻译目录检查 | 通过；126 条 Rust 消息、234 条界面文案在 en/zh-CN 下完整，占位符一致 |
| `python -m unittest discover -s scripts -p 'test_*.py' -v` | 27 项通过 |
| `git diff --check` | 通过 |

新增回归覆盖默认软链接安装、软链接随主副本更新、停用/重新启用、主副本缺失时的链接清理、
复制模式与混合部署卸载、部署方式持久化、配置分组反序列化及旧版扁平配置迁移、CLI 与宿主预览。
三个软链接用例在最终树中沿用仓库既有做法标为 `#[cfg(unix)]`；本次在具备符号链接权限的 Windows 环境临时
解除标记后单独运行通过。Windows CI 的符号链接权限仍需以具体工作流结果为准。

一次全量并行运行中 `toolbox-windows` 的原生窗口用例
`native_quake_hides_reuses_and_recovers_without_activation` 失败；单独重跑及随后的全量重跑均通过。
该用例与本次改动无关，但真实窗口焦点行为仍应在对应桌面环境复核。

`scripts/smoke_host.py` 与 `scripts/smoke_appearance.py` 已改为分组配置结构并通过语法检查；
两者需要隔离的 Linux X11 桌面，未在本机执行，由 CI 的 Linux 集成任务复核。

本地未执行完整界面人工验收、真实仓库下载和 macOS/Linux 实机运行；这些结果以 PR 对应提交的 CI 为准。

## 2026-11-07：桌面 UI/UX 全面优化

本轮按确认的桌面效率工具方向优化，开始于 `68c441c`，交付前同步至最新主分支
`f2d84fbd51c1240a1cadacfbff8a5777366e76a4`，保留上游配置分组 v2 和默认符号链接部署。
环境为 Linux x86_64 容器，Rust/Cargo 1.99.1、Slint 1.18.1，使用锁定依赖与离线缓存。
没有变更工具链、依赖版本或发布版本；测试与截图使用临时或明确标注的示例数据。

### 交互和布局

- 管理窗口统一浅深色层级、控件尺寸、焦点与操作反馈。默认 1120×780，最小 800×580，
  窄窗口使用图标导航；共享页面视口允许内容滚动，保存与撤销操作保持可见，没有限制最大化尺寸。
- 最近项目面板使用 760×432 默认尺寸、36 像素结果行、远程标记与可读路径；
  `Alt+Enter` 展开可选中的完整路径，`Esc` 先收起详情，再收起面板。
- 应用支持搜索、基础/高级字段分组、局部校验和保存后继续编辑。测试使用已保存配置，
  不丢弃其他草稿；设置保留主题/语言预览、显式保存与撤销。
- 快捷键录制先等待宿主确认，再接受已注册或普通按键。宿主使用带 token 的三秒短租约，
  不移除原生注册；界面按请求发出时间执行更早的两秒期限，即使 IPC 仍在等待也会结束录制。
  取消、切页、关闭及真实失焦会结束捕获，过期或旧 token 回复不会重新开启录制。
- Skills 使用全宽列表与按需详情，支持明确的批量启用/停用、隐藏选择数量、导入复核、
  来源和路径详情、具体错误及当前操作状态。默认部署方式与源目录复制/链接分别说明，
  切换全局方式不会自动转换现有部署，也不会覆盖导入中的源路径和应用选择。

### 最终门禁

| 检查 | 本轮结果与范围 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --locked --offline --workspace --all-targets -- -D warnings` | 通过 |
| `cargo test --locked --offline --workspace --no-fail-fast` | 254 通过、3 失败、1 ignored；命令退出 101，失败均为下述既有 IPC 环境限制 |
| UI 行为测试 | 96 项通过，包括真实 Slint 键盘/指针事件、录制期限、草稿保留、筛选索引、Skills 复核与部署模式 |
| `python3 scripts/translations.py check` | 通过；161 条 Rust 消息、285 条界面文案完整，官方提取器与 gettext 校验通过 |
| `python3 -m unittest discover -s scripts -p 'test_*.py' -v` | 27 项通过 |
| 生产 Slint 软件渲染 | 152 张截图成功；20 种页面/状态，中英文、浅深主题，默认与最小窗口尺寸；实际尺寸、像素缓冲区和布局约束一致 |
| 原生脚本共享控件坐标探针 | 12 个组合通过；英文、默认字体、浅深主题、100%/150%/200%，真实指针/文字输入覆盖应用选择、录制入口、Quake 预设、主题及撤销等 11 类目标 |
| `git diff --check` 与 Python 语法检查 | 通过 |

3 项失败为 `toolbox-ipc` 的 `owner_only_socket_round_trips_and_is_removed_on_drop`、
`stale_owner_socket_is_recovered_but_active_socket_is_rejected` 和
`unix_transport_rejects_relative_and_oversized_messages`。本环境创建 Unix socket 返回
`EPERM / Operation not permitted`；在改动前的 `68c441c` 基线上复现相同 3 项失败，
该基线有 205 项通过、1 项 ignored。未修改 IPC 实现、跳过用例或弱化断言。

### 截图与平台边界

README 的六张截图已更新为上述最终组件渲染，使用示例数据与任务内注册的 Noto Sans CJK SC，
不代表 0.2.0 已发布资产。检查重点包含默认和窄窗口的固定操作栏、Skills 导入/部署设置、
完整路径详情和长文本。软件渲染与指针探针不启动宿主，不访问用户配置，也不能证明原生焦点、
全局快捷键、文件选择器或真实多显示器缩放已通过。

`scripts/smoke_appearance.py` 同步了新尺寸、控制位置与配置分组，保留既有原生生命周期、
主题持久化、最大化和焦点断言；新增真实 F8 长按录制、不启动应用、取消后保留管理窗及恢复
原快捷键路由的检查。本环境因 Unix socket 限制无法启动 Xvfb，未完成该原生验收。
Windows/macOS 实机交互尚未在本轮验证；四平台构建、完整 IPC 与 Linux 原生窗口/宿主/UI smoke
以此 PR 对应提交的 GitHub Actions 结果为准。

## 2026-11-07：翻译目录去重、共享消息与生成 API 加固

基于合并 PR #2/#3 后的 `main`，继续处理复核建议，并同步最新的 Skills 部署与配置分组改动：

- 将 Rust 与 Slint 共用的 20 个无参数消息统一为 `.slint` 中的 `ui.common.*` 上下文；构建期为 Rust 生成 `messages::common_*`，`rust.pot` 移除 22 条重复条目，中文 PO 每个概念只保留一条译文。
- 四个及以上参数的消息改为具名参数结构体（`DiagnosticsSummaryArgs`、`ImportProfileArgs` 等）；以 `sync_diagnostics` 回归测试校验字段映射。
- `settings_changed` 只在语言实际变化时重渲染；`sync_diagnostics` 只渲染一次 `last_error` 并少一次 warnings 克隆；`Locale` 增加 `index()` / `from_index()`，UI 预览与宿主托盘共用同一映射。
- `scripts/translations.py` 的 `msgfmt -o` 改用暂存目录中的 `.mo` 文件，避免 MSYS2 Windows 下在仓库根目录生成 `nul` 文件。
- 小屏幕 900×650 夹取保持既有行为，并以常量与注释标明其与 `main.slint` 最小尺寸一致；低于该布局最小值的显示不在支持范围。

环境：Windows 11、Rust 1.99.1 MSVC（本机 GNU 工具链缺少 dlltool/windres）、`slint-tr-extractor 1.18.1`、MSYS2 gettext。

| 检查 | 结果与范围 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | 通过 |
| `cargo test --locked --workspace` | 通过；UI 61 项、`toolbox-i18n` 16 项 |
| `python scripts/translations.py check` | 通过；104 条 Rust + 230 条界面 = 334 条活动条目，20 条 `ui.common.*` 均有中文译文 |
| `python -m py_compile scripts/translations.py` | 通过 |
| 跨目录重复检查 | 共享英文标签的跨目录重复为 0；`ui.common.*` 共 20 条 |
| 生成 API 回归 | 10 项目录测试通过，含共享消息生成、拒绝占位符/复数、具名参数结构体编译执行 |

## 2026-11-08：UI/UX PR 的跨平台修复与主分支整合

继续处理 [PR #5](https://github.com/sean2077/sean-toolbox/pull/5)，保留已确认的 UI/UX 方向，
同步主分支 `132d0458225e4c47d6b5f163abd6a33090835ba5` 的翻译去重、共享消息和具名参数 API。
下表对应代码版本 `ca34dea03843579110944cd3d1d8e00f11f06492`；之后仅更新本记录与 Quake 截图。
环境为 Ubuntu 24.04 容器、Rust/Cargo 1.99.1、Slint 1.18.1，使用锁定依赖及任务目录中的开发库。

### CI 失败与修复

[首轮 Actions](https://github.com/sean2077/sean-toolbox/actions/runs/42381533397) 的两种 macOS 架构通过；
Windows 与 Linux 在测试阶段失败，Linux 原生 smoke 尚未执行。

- Windows 的 3 个新 Skills UI 用例把完整路径硬编码为 POSIX 分隔符。生产代码使用 `Path::join()`，
  本次将用例改为完整 `Path` 比较，保留来源、部署目标、应用提示及具体错误关联的断言。
- Linux 的 `stale_staging_directories_are_cleaned_on_open_without_touching_unknown_entries`
  在关闭服务后立即重开时遇到残留文件锁。本次以 RAII 守卫显式解锁；守卫只在成功取得锁后创建，
  并覆盖读取状态失败的提前返回，服务资源清理后才释放锁。
- 新增确定性的 Unix 重复描述符回归：保留旧锁文件的重复描述符时，服务关闭后仍可立即重开；
  服务存活时继续拒绝第二个实例，关闭旧描述符不会释放新实例的锁。此模型复现了共享描述符延长锁
  生命周期的边界，但首轮 CI 日志不足以确定当时持有描述符的具体子进程。
- 将 20 个共享界面标签集中在 `common-messages.slint`，同步生产界面及渲染 fixture 的调用，
  保留新的 Skills 布局、快捷键捕获协议、应用编辑行为和 800×580 最小尺寸。

### 提交前验证

| 检查 | 结果与范围 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --locked --offline --workspace --all-targets -- -D warnings` | 通过，包括渲染 example |
| `cargo test --locked --offline --workspace --no-fail-fast` | 262 通过、3 失败、1 ignored；命令退出 101，失败均为下述 IPC 环境限制 |
| UI / Skills / 国际化测试 | 分别为 98 / 41 / 16 项通过；首轮 CI 失败的相关用例均通过 |
| 文件锁回归敏感性 | 临时恢复旧的仅关闭句柄行为时，新用例确定失败；还原显式解锁后通过，源文件已完整恢复 |
| `python3 scripts/translations.py check` | 通过；139 条 Rust + 285 条界面文案，共 424 条活动中文译文，包含 20 个共享标签 |
| `python3 -m unittest discover -s scripts -p 'test_*.py' -v` | 27 项通过 |
| 整合后的 Slint 软件渲染 | 52 张预览成功，覆盖项目、应用、设置、Quake、Skills 列表/详情、启动器；中英文、浅深主题及默认/最小尺寸 |
| 原生 smoke 共用控件探针 | 12 组通过，覆盖 100%/150%/200% 缩放、浅深主题的 11 类关键目标 |
| `git diff --check` | 通过 |

3 项 IPC 失败仍为 `owner_only_socket_round_trips_and_is_removed_on_drop`、
`stale_owner_socket_is_recovered_but_active_socket_is_rejected` 和
`unix_transport_rejects_relative_and_oversized_messages`，均在创建 Unix socket 时返回 `EPERM`。
与前述同环境基线一致；未跳过用例或修改 IPC 实现。

已目视复核重点默认/最小窗口、共享文案和固定操作栏。README 的 5 张截图与新渲染逐字节相同；
Quake 截图更新了共享的“已启用／未启用”文案。截图使用示例数据，不表示已发布版本或原生桌面验收。
本地无法创建 Xvfb 所需 socket；这组修复的四平台构建、完整 IPC 与 Linux 原生 smoke 结果，
以同一 PR 的最新 Actions 检查项为准。Windows/macOS 的人工字体、缩放、文件选择器与多显示器检查
仍需在对应桌面环境执行。

## 2026-11-08：UI/UX PR 审阅修复

审阅 `58e6cce` 后修正三处交互问题，均附回归测试：

- 通用设置的最近项目快捷键和缓存时间在每次编辑后重新校验，冲突与格式错误在字段旁即时出现并随修正消失，不再等到保存或切换语言。
- 最近项目面板在搜索无结果、详情被隐藏时，`Esc` 直接收起面板，不再先消耗一次按键收起不可见的详情。
- Skills 详情页中的操作让技能离开当前筛选（例如“需要处理”筛选下修复错误）时，详情保持在该技能上；技能被移除后才关闭。

环境为 Windows 11 x86_64，Rust 1.99.1 MSVC 工具链（本机默认 GNU 工具链缺少 `dlltool`，按开发指南改用 MSVC）。

| 检查 | 结果与范围 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | 通过 |
| `cargo test --locked -p toolbox-ui` | 100 项通过，含新增 2 项和扩展的启动器 `Esc` 用例；临时撤销 Skills 修复时新用例确定失败 |
| `cargo test --locked --workspace`（修复前的 PR 头） | 除下述 1 项外全部通过 |

`toolbox-windows` 的 `native_quake_hides_reuses_and_recovers_without_activation` 在本机置顶确认失败；
同一机器上的 `main`（`132d045`）复现相同失败，PR 未改动该 crate，同一 PR 的 Windows CI 通过，判断为本机桌面环境限制。
未执行原生桌面人工验收。

## 2026-11-08：审阅建议跟进

完成 PR #5 审阅中列出的三项后续改进，均附回归测试：

- Skills 来源筛选按仓库 ID 记忆。移除排在前面的仓库时仍筛选原仓库；该仓库被移除后回到“全部”，不再静默改为其他仓库。
- 通过文件夹选择器导入时，与本机扫描一样按文件夹名（不区分大小写）核对已管理技能，同名时提供“查看已安装技能”，不提交导入。
- 界面在本地执行的配置校验（应用保存、设置保存、配置导入）改为按当前语言显示原因。`toolbox-core` 新增 `ConfigError`，`Config::validate` 仍返回原英文诊断，宿主、IPC 与日志文本不变；宿主返回的诊断继续原样显示。新增 11 条 Rust 消息及中文译文。

环境为 Windows 11 x86_64，Rust 1.99.1 MSVC 工具链。

| 检查 | 结果与范围 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | 通过 |
| `cargo test --locked --workspace --no-fail-fast` | 除已知的 `toolbox-windows` 本机置顶用例外全部通过；该用例在 `main` 上同样失败，见上一节 |
| `cargo test --locked -p toolbox-ui -p toolbox-core -p toolbox-i18n` | 通过；UI 102 项，core 27 项，翻译目录完整性测试通过 |
| 回归敏感性 | 临时撤销来源筛选与文件夹核对修复时，对应新用例确定失败 |

本机未安装 GNU gettext，`python scripts/translations.py check` 的 `msgfmt` 步骤以 CI 结果为准；
新增条目只在 `rust.pot` 与中文 PO 中手工添加，`ui.pot` 未变。
