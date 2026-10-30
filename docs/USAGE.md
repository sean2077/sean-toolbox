# 使用指南

[返回首页](../README.md)

## 最近项目

按 `Ctrl+Alt+Space` 唤出项目面板。每次打开都会刷新历史、清空搜索并聚焦输入框；空搜索按最近访问顺序显示，输入后按项目名和路径模糊匹配。

| 操作 | 快捷键或鼠标动作 |
| --- | --- |
| 选择项目 | `↑` / `↓`，或单击；到首尾时停止 |
| 打开项目 | `Enter` 或双击 |
| 聚焦并全选搜索 | `Ctrl+K` / `Ctrl+F` |
| 刷新历史 | `Ctrl+R` |
| 打开管理窗口 | `Ctrl+,` 或右上角设置按钮 |
| 收起面板 | `Esc` 或切换到其他窗口 |

面板内的 `Ctrl` 组合键在 macOS 上也支持对应的 `⌘` 组合键。打开成功后面板收起；失败时保留搜索结果并显示错误。

项目列表来自 VS Code Stable 标准用户目录的历史记录，包括本地文件夹、`.code-workspace` 文件和远程 URI。数据库以只读方式打开，普通最近文件不列入项目列表。可在设置中指定 VS Code 可执行文件；Insiders、便携版、自定义数据目录和自定义 profile 暂不支持自动发现。

## 应用快捷键

在管理窗口的“应用快捷键”页添加或编辑应用，填写匹配标识、快捷键和启动方式。

| 平台 | 匹配标识示例 |
| --- | --- |
| Windows | 进程文件名，如 `Code.exe` |
| macOS | bundle ID、应用名或可执行文件名，如 `com.microsoft.VSCode` |
| Linux X11 | WM_CLASS 或进程文件名，如 `org.wezfurlong.wezterm` |

普通切换模式的行为：

- **没有窗口**：按顺序尝试启动候选。一个候选启动成功后等待窗口出现。
- **一个窗口**：窗口在前台时最小化，否则切回该窗口。
- **多个窗口**：按稳定顺序依次切换。开启连续循环时，末尾回到第一个窗口；关闭时，末尾将全部窗口最小化，再次按键从头开始。

切换到其他应用后，再按快捷键会接着上次的位置选择下一个窗口。“仅最大化新窗口”用于启动时的窗口大小；已有窗口保留原尺寸。应用启动期间主动切换到其他窗口，会停止自动聚焦。

默认配置包含 WezTerm `F3`、VS Code `F4` 和 Chrome `F5`。Windows 会尝试常用安装目录，macOS 使用 `/Applications`，Linux 使用 `PATH`；安装位置不同时，在编辑区修改启动程序。

### Quake 停靠

在应用编辑区开启“Quake 停靠模式”，将一个应用窗口放在屏幕边缘。同一快捷键始终唤回这个窗口；窗口在前台时再按一次收起。

- 停靠方向：上、下、左、右。
- 尺寸：显示器可用工作区的 10–100%，宽高分别设置。
- 对齐：起始、居中、末端。
- 显示器：鼠标所在显示器、前台窗口所在显示器、主显示器。
- 预设：顶部半屏、右侧边栏、铺满工作区。编辑区可预览布局，保存后下次唤起应用。

定位使用显示器工作区，避开任务栏或 Dock；最终尺寸还受目标应用最小尺寸和终端字符网格影响。

| 选项 | 支持平台 |
| --- | --- |
| 显示时置顶 | Windows、Linux X11；收起或退出工具箱后恢复原置顶状态 |
| 失去焦点时自动收起 | Windows；所属对话框获得焦点时保持显示 |

macOS 的窗口定位需要辅助功能权限；Linux 需要 X11、EWMH 窗口管理器和 XRandR 1.5。Quake 模式使用自己的窗口和尺寸设置，普通模式的连续循环、启动最大化选项在关闭 Quake 后使用。

## 设置与界面关闭

设置中可切换中文或英文、选择跟随系统或固定明暗主题，分别启停最近项目和应用快捷键，并设置开机启动。主题选择会即时预览，保存后保留，撤销时恢复。

应用与通用设置分别保留草稿，导航会标记尚未保存的修改。`Ctrl+S` 保存当前编辑页，`Ctrl+1`–`4` 切换页面；macOS 也支持对应的 `⌘` 组合键。

“自动收起后缓存”控制最近项目面板在失焦或打开项目后收起时，界面进程保留多久。默认 `0` 秒，可设为 `1–3600` 秒以加快再次唤起。已有配置沿用保存的数值。

主动点击关闭按钮、选择“关闭界面”或按 `Esc` 关闭最后一个界面时，进程立即退出。另一界面仍可见、存在草稿、文件对话框或操作尚未完成时，会继续保留界面进程。关闭有修改的管理窗口时，会询问是否放弃修改。托盘和全局快捷键由常驻宿主继续处理。

## 配置文件

日常设置可直接在界面修改。配置文件位置如下，也可运行 `sean-toolbox config-path` 查看。

| 系统 | 路径 |
| --- | --- |
| Windows | `%LOCALAPPDATA%\SeanToolbox\config.json` |
| macOS | `~/Library/Application Support/SeanToolbox/config.json` |
| Linux | `$XDG_CONFIG_HOME/sean-toolbox/config.json`；变量未设置或不是绝对路径时，使用 `~/.config/sean-toolbox/config.json` |

手动修改前先退出工具箱，修改后重新启动。解析失败时会保留文件并报告错误。配置文件上限 1 MiB，最多 100 个应用，每个应用包含 1–8 个启动候选。

### 通用字段

| 字段 | 含义与默认值 |
| --- | --- |
| `schema_version` | 配置格式版本，当前为 `1` |
| `language` | `"zh-CN"` 或 `"en"`，默认 `"zh-CN"` |
| `theme` | `"system"`、`"light"` 或 `"dark"`，默认 `"system"` |
| `launcher_hotkey` | 项目面板快捷键，默认 `"Ctrl+Alt+Space"` |
| `autostart` | 开机启动，默认 `false` |
| `projects_enabled` / `windows_enabled` | 最近项目 / 应用快捷键开关，默认均为 `true` |
| `ui_idle_seconds` | 自动收起后的界面缓存秒数，默认 `0`，范围 `0–3600` |
| `vscode_path_override` | VS Code 可执行文件路径，默认 `null`，由程序查找 |
| `profiles` | 应用配置数组 |

### 应用配置示例

下面是一项 Windows 应用配置，放在 `profiles` 数组中：

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

`platform` 可取 `windows`、`macos`、`linux`，应用配置只在对应平台启用。`id` 用于命令行切换和导入合并；`cycle_continuous` 控制多个窗口切换到末尾后是否回到第一个。

启动参数使用 JSON 字符串数组，例如 `["--new-window", "D:\\Work\\My Project"]`。程序路径和工作目录支持 `~`、`%VAR%`、`${VAR}` 展开；参数按原样逐项传递。Windows 应用启动项填写 `.exe` 路径。最近项目模块还支持从标准安装布局中的 `code.cmd` 定位到 `Code.exe`。

### Quake 字段

在应用配置中加入 `quake` 对象；省略该字段时使用普通切换模式：

```json
{
  "enabled": true,
  "edge": "top",
  "alignment": "center",
  "width_percent": 100,
  "height_percent": 50,
  "monitor": "cursor",
  "always_on_top": false,
  "hide_on_focus_loss": false
}
```

`edge` 支持 `top` / `bottom` / `left` / `right`；`alignment` 支持 `start` / `center` / `end`；`monitor` 支持 `cursor` / `foreground` / `primary`。置顶和失焦收起的适用平台见前面的 Quake 选项表。

### 导入与导出

设置页可导出当前已保存的 JSON 配置，也可导入工具箱 JSON 或 WinGuake 格式的 INI。选择文件后先预览，检查应用列表、快捷键和提示，再应用。

- **合并**：按 `id` 更新同名应用并追加新应用，`id` 比较不区分大小写；通用设置保持当前值。
- **替换 JSON**：采用文件中的应用列表和通用设置。
- **替换 INI**：采用导入的应用列表，通用设置保持当前值。INI 导入项属于 Windows 配置。

无法转换的快捷键会留空并提示手动设置。保存时校验配置和快捷键冲突，通过后写入文件并应用。

## 命令行

```text
sean-toolbox                              打开最近项目面板
sean-toolbox show projects                打开最近项目面板
sean-toolbox show windows                 打开应用快捷键配置
sean-toolbox show settings                打开设置
sean-toolbox show diagnostics             打开运行状态
sean-toolbox --background                 启动常驻宿主
sean-toolbox toggle vscode                切换 id 为 vscode 的应用
sean-toolbox status                       输出 JSON 运行状态
sean-toolbox config                       输出当前配置与修订版本
sean-toolbox config-path                  输出配置文件路径
sean-toolbox quit                         退出宿主和界面
sean-toolbox --version                    显示版本
sean-toolbox --help                       显示帮助
```

打开界面的命令会按需启动宿主。`status`、`config`、`toggle`、`quit` 需要宿主正在运行；`config-path`、`--version` 和 `--help` 可直接使用。后台启动日志位于配置目录的 `host.log`。

## 常见问题

| 现象 | 检查方法 |
| --- | --- |
| 快捷键没有响应 | 在“运行状态”检查注册结果，调整与系统或其他应用冲突的快捷键 |
| 应用可以启动但无法切回窗口 | 核对匹配标识；Windows 检查目标应用的权限，macOS 检查辅助功能授权 |
| macOS 更换安装位置后无法控制窗口 | 在“系统设置 → 隐私与安全性 → 辅助功能”重新授权，再到运行状态页重新检查 |
| Linux 没有托盘图标 | 确认桌面提供托盘宿主；也可通过命令行打开界面 |
| Linux 无法选择导入或导出文件 | 安装并启用 XDG Desktop Portal 文件选择后端，或安装 `zenity` |
| 最近项目列表为空 | 先在 VS Code Stable 中打开项目，再刷新；确认使用的是标准用户数据目录 |
| 关闭面板后仍有界面进程 | 检查缓存时间、另一个可见窗口、未保存草稿及尚未完成的操作 |
