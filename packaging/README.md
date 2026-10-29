# 构建与打包

先在目标平台使用仓库固定的 Rust 工具链构建，再使用 Python 3.11+ 打包：

```sh
cargo build --locked --release -p toolbox-host -p toolbox-ui
cargo fetch --locked
python scripts/collect_licenses.py
python scripts/package.py
```

收集脚本通过 `cargo metadata --locked --offline --filter-platform <host>` 获取当前
Rust 编译目标的已解析依赖，复制本地 Cargo 缓存里的许可证原文到
`target/THIRD_PARTY_LICENSES.txt`。可用 `--target <triple>` 为交叉编译指定目标。
缺失文本时列出包名并失败；它不会联网补全或修改依赖源码。构建时需确保依赖已完整下载。
`cargo fetch --locked` 会补齐锁文件中的其他平台依赖，确保 metadata 的完整包清单也可离线读取。

少数 crates 发布时未包含许可文件，补充原文位于 `licenses/dependencies/`。
其中 `manifest.json` 按精确的包名和版本映射文件，记录上游固定提交的来源链接、
所选许可及 SHA-256；收集器验证校验和后才加入发布包。升级依赖时需要为新的
缺失项核对对应源码提交，不能只把旧版本映射的版本号改掉。该清单和原文随发布包分发。

打包脚本从根 `Cargo.toml` 读取版本，将两个程序、安装说明和许可证打包到 `dist/`，
同时生成 `.sha256` 校验文件。Python 仅在打包时使用，不进入最终应用的运行依赖。

可通过 `--platform windows|macos|linux`、`--arch x86_64|aarch64`、
`--binary-dir <path>` 和 `--out-dir <path>` 指定已经编译好的目标二进制与输出位置。
这些参数不执行交叉编译；调用者必须确保二进制与标签匹配。

GitHub Actions 对 Windows x64、macOS Apple 芯片/Intel、Linux x64 分别原生编译。
所有提交先通过格式、Clippy 和测试，再构建可下载的工作流 artifact。
手动运行 workflow 也会打包。推送与 Cargo 版本完全一致的 `v*` tag 后，工作流汇总
所有平台的包并创建 GitHub Release；包含 `-` 的预发行版本会标记为 prerelease。

发布文件是便携 ZIP、包含 `.app` 的 ZIP 或 Linux tar.gz。当前没有 Windows
Authenticode、macOS Developer ID 签名或 Apple 公证。构建成功只说明编译/测试通过，
不能代替实际桌面上的快捷键、托盘、多显示器、焦点与系统权限验收。
