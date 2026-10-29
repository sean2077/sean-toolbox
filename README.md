# Sean Toolbox · 个人工具箱

一个独立的跨平台桌面工具箱，使用 **Rust + Slint**。首批功能是应用窗口快捷键和 VS Code 最近项目搜索，逐步把日常需要的小工具放进同一个应用。

不需要 uTools、AutoHotkey 或 Node.js 运行环境。当前版本为 **0.1.0-beta.1**，Windows 优先；macOS 和 Linux 使用各自的原生后端。

[![Made with Slint](https://raw.githubusercontent.com/slint-ui/slint/master/logo/MadeWithSlint-logo-whitebg.png)](https://slint.dev)

## 功能

### 应用窗口快捷键

- 应用未运行时按顺序尝试启动候选；某个候选成功启动后，等待窗口出现，不因窗口出现慢而再次启动另一个候选。
- 一个窗口时切换显示和最小化；多个窗口时按稳定顺序循环，末尾全部最小化，再次按下重新开始。
- 可选择切换到其他应用后继续上次的循环位置。
- 可选择只最大化新启动的窗口，保留已有窗口的大小。
- 应用配置可增删改、启停，分别指定平台、进程或应用标识、快捷键、启动程序、独立参数数组和工作目录。
- 启动过程中如果用户主动切到别的窗口，停止自动抢焦点。系统拒绝激活时显示真实错误。

默认示例：WezTerm `F3`、VS Code `F4`、Chrome `F5`。请按实际安装路径调整。Windows 会尝试这些应用的常用安装目录；macOS 使用 `/Applications`；Linux 使用 `PATH`。

### 最近项目

- 读取 **VS Code Stable** 标准用户目录中的历史记录，保留本地文件夹、`.code-workspace` 和远程 URI 的不同含义。
- 搜索、去重和排序；支持 `↑` / `↓` 选择、`Enter` 打开、`Esc` 收起。
- 数据库按只读方式打开，不修改 VS Code 状态。
- 只读取最近项目，不扫描全盘，不收集普通最近文件。
- 可指定 VS Code 可执行文件路径。打开请求直接传递参数，不拼接 shell 命令。

### 工具箱本身

- 默认 `Ctrl+Alt+Space` 唤出项目页，也可从托盘打开四个页面。
- 中文默认界面，可切换英文；设置包含模块启停、开机启动和界面闲置释放时间。
- INI / JSON 导入有预览，支持合并或替换应用配置；支持 JSON 导出。
- 快捷键冲突不会覆盖当前配置；保存有版本校验和原子文件替换。
- 诊断页显示后端能力、快捷键与托盘状态、最近操作及错误，并提供关于和第三方来源。

## 平台范围

| 平台 | 窗口控制 | 全局快捷键 | 界面和项目搜索 |
| --- | --- | --- | --- |
| Windows | Win32：进程名、顶层窗口枚举、显示/最小化/最大化 | 原生注册 | Slint 原生界面 |
| macOS | Accessibility API + AppKit；按 bundle ID 匹配，需要辅助功能权限 | 原生注册；可能与系统快捷键冲突 | Slint 原生界面 |
| Linux X11 | X11 / EWMH；按 WM_CLASS 或进程名匹配，需要兼容的窗口管理器 | X11 注册 | Slint 原生界面 |
| Linux Wayland | 本版不支持任意外部窗口控制 | 本版未实现桌面 Portal 全局快捷键 | 项目搜索、设置及支持 StatusNotifierItem 的托盘可用 |

**实现和验证是两件事：** 仓库提供三平台原生构建 CI 和 Linux X11 真窗口测试。Windows/macOS 的焦点策略、权限提示、系统缩放和桌面交互仍需要对应系统实机验收；交叉编译通过不能替代这些检查。

Windows 普通权限进程无法保证控制管理员权限窗口。macOS 在“系统设置 → 隐私与安全性 → 辅助功能”授权后可在诊断页重新检查；应用路径或签名变化可能需要重新授权。Linux 桌面没有托盘宿主时仍可通过命令行打开界面。

## 获取与运行

在 [Actions](https://github.com/sean2077/sean-toolbox/actions/workflows/ci.yml) 中下载成功构建的对应平台 artifact。版本标签 `v*` 通过全部构建后会生成 [Release](https://github.com/sean2077/sean-toolbox/releases)。每个压缩包包含安装说明和 SHA-256 文件。

- Windows：完整解压后运行 `sean-toolbox.exe`。
- macOS：解压并将 `Sean Toolbox.app` 放到固定位置，再打开应用。
- Linux：解压后运行 `bin/sean-toolbox`；桌面入口安装方法见包内 `INSTALL.md`。

**主程序和 `sean-toolbox-ui` 必须保持在一起。** 首次打开会启动托盘宿主并显示项目页；收起界面后，默认 60 秒释放界面进程，设为 `0` 可立即释放。退出整个应用请使用托盘“退出工具箱”或 `sean-toolbox quit`。

当前构建没有 Windows Authenticode 签名或 macOS Developer ID 签名及公证。下载完成并不代表本机所有桌面权限已就绪，请先检查诊断页。

## 配置

配置位置：

| 系统 | 路径 |
| --- | --- |
| Windows | `%LOCALAPPDATA%\SeanToolbox\config.json` |
| macOS | `~/Library/Application Support/SeanToolbox/config.json` |
| Linux | `$XDG_CONFIG_HOME/sean-toolbox/config.json`，未设置时为 `~/.config/sean-toolbox/config.json` |

推荐使用界面编辑。手动改文件前先退出宿主；无效配置会报错并保留原文件，不会静默重置。配置文件上限 1 MiB，每个应用最多 8 个启动候选。开发和测试可通过绝对路径环境变量 `SEAN_TOOLBOX_CONFIG` 指定配置文件；单实例仍以当前用户为范围。

一个应用配置示例：

```json
{
  "id": "vscode",
  "name": "VS Code",
  "platform": "windows",
  "matcher": "Code.exe",
  "hotkey": "F4",
  "enabled": true,
  "maximize": false,
  "cycle_continuous": true,
  "launch": [
    {
      "executable": "%LOCALAPPDATA%\\Programs\\Microsoft VS Code\\Code.exe",
      "args": ["--new-window"],
      "cwd": null
    }
  ]
}
```

参数填写为 JSON 字符串数组，例如 `["--new-window", "D:\\Work\\My Project"]`。程序路径和工作目录支持 `~`、`%VAR%`、`${VAR}` 展开，参数保持原样。Windows 请指向 `.exe`，不要指向 `code.cmd`；最近项目模块可将受支持布局中的 `code.cmd` 定位到真实的 `Code.exe`。

旧 WinGuake INI 导入会转换常用 AHK 热键表示法，但不会执行 AHK。导入的 Windows 配置保留平台标签，在其他系统上不会自动启用。

## 命令行

```text
sean-toolbox                              打开最近项目
sean-toolbox show windows                 打开窗口配置
sean-toolbox show settings                打开设置
sean-toolbox show diagnostics             打开诊断
sean-toolbox --background                 仅启动常驻宿主
sean-toolbox toggle vscode                切换指定应用
sean-toolbox status                       JSON 诊断
sean-toolbox config                       当前配置与修订版本
sean-toolbox config-path                  配置文件位置
sean-toolbox quit                         退出宿主和其界面
```

打开界面的命令可自动启动宿主，`status` / `config` / `toggle` / `quit` 要求宿主已运行。后台启动日志写到配置目录的 `host.log`。

## 开发

工具链固定在 `rust-toolchain.toml`，依赖版本保存在 `Cargo.lock`。需要 Rust、目标平台 C/C++ 链接工具，以及 Linux 的 X11/Wayland、字体和键盘开发库。Ubuntu 的完整依赖安装命令见 [CI](.github/workflows/ci.yml)。

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release -p toolbox-host -p toolbox-ui
./target/release/sean-toolbox
```

Linux 真窗口测试使用独立 Xvfb、Openbox 和 xterm，清理由脚本负责：

```sh
bash crates/toolbox-windows/tests/run-x11-smoke.sh
```

构建完成后收集第三方许可并打包：

```sh
python scripts/collect_licenses.py
python scripts/package.py
```

### 模块划分

| crate | 责任 |
| --- | --- |
| `toolbox-core` | 配置模型、校验、原子存储、INI 导入、IPC 协议 |
| `toolbox-ipc` | 当前用户的本地 socket / named pipe，消息大小与等待时间限制 |
| `toolbox-windows` | 窗口状态机与 Win32 / AX / X11 适配；这里的 windows 指应用窗口 |
| `toolbox-projects` | VS Code 数据源、项目身份、排序和打开策略 |
| `toolbox-host` | 单实例、快捷键、托盘、配置事务、窗口 worker、界面生命周期 |
| `toolbox-ui` | Slint 四页界面、交互状态、按需项目查询和文件选择 |

常驻宿主不链接 Slint、SQLite 或项目索引模块。UI 进程按需启动；窗口控制仅在触发时工作，不持续枚举所有窗口。宿主主事件循环等待事件；系统库和不同桌面的实际资源占用应分别测量，不把一个平台的结果作为所有平台保证。

新增功能优先作为独立 Rust crate，通过明确的命令和数据模型接入宿主或界面。首版使用内置模块，尚无第三方插件 ABI、脚本运行时或插件商店。需要独立安装插件时再选择有版本协议的进程接口，避免提前绑定不稳定的 Rust 动态库 ABI。

## 已知范围

- 最近项目支持 VS Code Stable 的标准用户目录；暂不自动发现 Insiders、便携数据目录、自定义 `--user-data-dir` 和自定义 profile。
- VS Code 已打开项目的判断来自其最后保存的窗口状态，因此只表达打开偏好，不能保证操作系统最终焦点。
- 窗口匹配以应用为单位，不按浏览器标签页、窗口标题正则或虚拟桌面规则匹配。
- 无自动更新、云同步、遥测、远程服务或第三方扩展加载。开机启动默认关闭。
- 两个模块可以分别停用；Wayland 的外部窗口限制会明确显示，不伪装成已支持。

## 来源和许可

应用自有代码采用 [MIT](LICENSE)。项目历史读取及 IPC 源自 [Recentry](https://github.com/sean2077/recentry)，窗口行为和 INI 迁移参考 [WinGuake](https://github.com/sean2077/winguake)，保留其 MIT 许可与来源记录。

Slint 在本应用中按 Royalty-free Desktop, Mobile, and Web Applications License 2.0 使用，保留界面中的 `AboutSlint` 和本页归属徽章。详见 [第三方说明](THIRD_PARTY_NOTICES.md)；发行包另附依赖许可汇总。
