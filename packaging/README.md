# 构建与打包

在仓库根目录运行以下命令，使用固定的 Rust 工具链构建目标平台程序，并用 Python 3.11+ 打包。
开发环境准备见 [开发指南](../docs/DEVELOPMENT.md)。

```sh
cargo fetch --locked
cargo build --locked --release -p toolbox-host -p toolbox-ui
python scripts/collect_licenses.py
python scripts/package.py
```

## 输出文件

打包脚本从根 `Cargo.toml` 读取版本，输出到 `dist/`：

| 平台 | 归档格式 | 程序位置 |
| --- | --- | --- |
| Windows | `.zip` | 根目录中的两个 `.exe` |
| macOS | `.zip` | `Sean Toolbox.app/Contents/MacOS/` |
| Linux | `.tar.gz` | `bin/` |

归档包含宿主和界面程序、`INSTALL.md`、README、使用指南、界面截图、验证记录及许可证。
每份归档旁另有同名的 `.sha256` 校验文件，例如 `sean-toolbox-<version>-linux-x86_64.tar.gz.sha256`。

打包已有的其他目标程序时，可指定以下参数：

| 参数 | 用途 |
| --- | --- |
| `--platform windows\|macos\|linux` | 选择打包布局 |
| `--arch x86_64\|aarch64` | 设置归档的处理器架构标签 |
| `--binary-dir <path>` | 读取已编译好的两个程序，默认为 `target/release/` |
| `--out-dir <path>` | 设置输出目录，默认为 `dist/` |

交叉编译先用 Cargo 构建对应目标，再将二进制目录、平台和架构传给打包脚本。

## 第三方许可

`collect_licenses.py` 通过 `cargo metadata --locked --offline --filter-platform <host>`
取得当前编译目标的依赖清单，将本地 Cargo 缓存中的许可原文汇总到 `target/THIRD_PARTY_LICENSES.txt`。
交叉编译时使用 `--target <triple>`。前面的 `cargo fetch --locked` 会补齐锁文件中的各平台依赖，
供离线读取；缺少许可原文时，收集脚本列出包名并退出。

部分依赖的许可补充文件位于 [licenses/dependencies](../licenses/dependencies/README.md)。
其中 `manifest.json` 按包名和精确版本记录文件、上游源码链接、所选许可和 SHA-256，
收集器核验后将其加入汇总。升级依赖时，按新版本对应的上游源码更新映射及原文。

## CI 与发布

[GitHub Actions](../.github/workflows/ci.yml) 在 Windows x64、macOS Apple 芯片/Intel 和 Linux x64
上分别构建，执行格式检查、Clippy、测试和打包；Linux 另有窗口与界面集成测试。
推送 `main`、提交 PR 或手动运行工作流后，可下载构建产物。

版本标签使用 `v<version>`，与根 `Cargo.toml` 中的版本一致。标签构建全部成功后，
工作流汇总各平台归档及校验文件，按 [发布说明](RELEASE_NOTES.md) 创建 GitHub Release。
含 `-` 的版本标记为预发行版。

当前 Windows 包未做 Authenticode 签名；macOS 包未做 Developer ID 签名及 Apple 公证。
各平台的安装和权限步骤见包内 `INSTALL.md`，桌面测试结果见 [验证记录](../docs/VALIDATION.md)。

## 应用图标

图标源文件是 `packaging/sean-toolbox.svg`。同一图形用于管理窗口、项目面板、
系统托盘和桌面入口；macOS 菜单栏使用去除背景色的模板图标，跟随系统明暗外观。
普通构建直接使用 `packaging/icons/` 中已提交的 PNG、ICO、ICNS 和托盘 RGBA 文件。

- Windows 的两个 EXE 都通过共享构建脚本嵌入 ICO，包含 16、24、32、48、64、128、256 像素。
  MSVC 构建使用 Windows SDK 的 `rc.exe`，自动从 Visual Studio/SDK 安装位置查找；
  GNU 构建使用 MinGW `windres`。可用 `RC` 指定资源编译器的完整路径。
- macOS 包将 ICNS 放入 `.app/Contents/Resources/`，由 `CFBundleIconFile` 引用。
- Linux 包同时提供可缩放 SVG 和 256 像素 PNG，桌面入口使用 `sean-toolbox` 图标名。

修改绘图源后，使用 **Python 3.11+、Pillow 和 Inkscape** 重新导出：

```sh
python scripts/generate_icons.py
python scripts/generate_icons.py --check
```

导出器使用临时 Inkscape 配置和缓存，生成固定尺寸的透明 RGBA，并去除图片时间戳和元数据。
`--check` 在内存中重新生成并逐字节比较。当前导出使用 Inkscape 1.2.2；
校验时保持 Inkscape/Pillow 版本一致，以免抗锯齿差异改变输出。
