# Sean Toolbox Agent 指南

## 项目定位

跨平台桌面工具箱，Rust workspace + Slint，Windows 优先。产品行为、平台能力与已知限制以 `README.md` 为准；本文件只记录 Agent 高频入口和非显而易见边界。

常驻宿主 `toolbox-host`（二进制 `sean-toolbox`）负责单实例、全局快捷键、托盘、配置事务和窗口 worker，不链接 Slint、SQLite 或项目索引模块。界面 `toolbox-ui`（二进制 `sean-toolbox-ui`）按需启动、可空闲释放；两个程序必须成对分发。模块职责见 README 的“模块划分”。

## 常用命令（均在仓库根目录执行）

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release -p toolbox-host -p toolbox-ui
```

`Cargo.lock` 已提交，验证使用 `--locked`；不要为无关改动升级依赖或重写锁文件。

Linux 真实窗口/宿主测试不在默认 `cargo test` 中（X11 用例标记为 ignored），需要隔离的 Xvfb、Openbox、xterm 和临时 HOME：

```sh
bash crates/toolbox-windows/tests/run-x11-smoke.sh
python scripts/smoke_host.py --binary target/release/sean-toolbox
```

第二项要求先构建两个 release 程序；`--measure` 增加 10 秒资源采样。两者都不得针对当前用户桌面运行。

打包（Python 3.11+，仅构建期使用）：

```sh
cargo fetch --locked
python scripts/collect_licenses.py
python scripts/package.py
```

`collect_licenses.py` 只读本地 Cargo 缓存和 `licenses/dependencies/`，不联网；`package.py` 从根 `Cargo.toml` 读取版本并输出到 `dist/`，同时生成 `.sha256`。完整参数和效果见 `packaging/README.md`。

## 验证边界

- 最低门禁是 `cargo fmt --all -- --check`、`cargo clippy --locked --workspace --all-targets -- -D warnings` 和 `cargo test --locked --workspace`。CI 在 Windows x64、macOS ARM/Intel、Linux x64 原生矩阵上执行格式、Clippy、测试、release 构建、许可收集和打包，并在 Linux 上额外执行真实窗口 smoke；矩阵与 Linux 依赖清单以 `.github/workflows/ci.yml` 为准。
- Linux X11 smoke 与 `smoke_host.py` 提供真进程、真窗口的集成证据；Windows/macOS 的焦点策略、系统权限提示、缩放、多显示器和原生对话框仍需对应系统实机验收。交叉编译或 CI 通过不能替代桌面验收。
- `docs/VALIDATION.md` 是带日期的历史验证记录，不是当前行为承诺；新的验收结果按其现有格式追加，不要改写旧记录。
- Wayland 不支持任意外部窗口控制；缺少托盘宿主或文件选择后端属于已记录的环境边界，不是可以静默弱化的测试失败。

## 源码、生成物与归属

- 手写源码位于 `crates/*/src/`、`crates/toolbox-ui/ui/main.slint` 和 `scripts/`。`target/`、`dist/`、`target/THIRD_PARTY_LICENSES.txt` 以及 `build.rs` 从 `.slint` 生成的 Rust 都是生成物；通过对应命令重新生成，不直接修补。
- `vendor/global-hotkey/` 是仓库内补丁依赖，仅修复 Linux X11 解绑缓冲问题，通过根 `[patch.crates-io]` 接入；保留 `PATCH.md` 的来源记录，不把它当普通上游依赖修改。
- `licenses/dependencies/manifest.json` 按“包名 + 精确版本”映射缺失的许可证原文和 SHA-256；升级依赖时按新源码提交核对，不要只改版本号。
- `.agents/**`、`.claude/**`、`.codex/**`、`CLAUDE.md` 由 agent-scaffold 管理。不要手改生成投影或 scaffold runtime；使用 `agent-scaffold upgrade` 和 `verify`。项目自有 Agent 指导写在本文件 managed block 之外。
- `Cargo.lock` 是已提交的锁定结果，不是手写文件；依赖变更后由 `cargo` 更新并审查 diff。

## 测试

- 测试位置跟随 crate：模块内测试用 `#[cfg(test)]`，跨模块/契约测试放 `crates/*/tests/`。`toolbox-projects/tests/core_contract.rs` 保留自 recentry 的契约测试；`toolbox-windows/tests/run-x11-smoke.sh` 驱动真实 X11 窗口测试；`scripts/smoke_host.py` 驱动真实宿主/UI 生命周期测试。
- 测试必须隔离用户状态：使用临时 HOME、临时配置和独立显示服务；开发/测试可用绝对路径 `SEAN_TOOLBOX_CONFIG` 指定配置文件。不要读写真实用户配置或操作当前桌面。
- 项目未规定强制 TDD；是否先写测试由任务决定。行为变更仍需能暴露风险的测试，或明确由哪项集成/实机检查承担。
- 项目测试中的 VS Code SQLite fixture 和记录参数的 CLI fixture 只证明数据语义与参数边界，不证明真实 VS Code 窗口焦点；不要把它报告为系统级验收。

## 文档、规格与术语

- 文档所有权：`README.md` 是用户可见的产品、平台范围、命令和配置契约；`docs/VALIDATION.md` 是验证证据；`packaging/README.md` 是构建/打包/发布流程；`packaging/RELEASE_NOTES.md` 是发布说明正文；各 crate 的 `README.md` 是模块契约；`THIRD_PARTY_NOTICES.md` 与 `licenses/` 记录来源和许可。
- 新的开发、设计或运维文档放 `docs/`，并从 README 或本文件建立路由；一次性计划留在 PR/issue，不新建永久页面。
- 语言：根 README、`docs/` 和本文件使用简体中文；crate README、代码注释和提交信息使用英文。术语使用根 `CONTEXT.md` 的正式名称。
- `toolbox-windows` 指控制“应用窗口”的模块，不是 Windows 平台，也不是工具箱自己的界面窗口。宿主与界面、启动候选、修订版本等术语见 `CONTEXT.md`。
- 没有独立的产品规格/RFC 体系：当前行为契约以 README、crate README 和代码为准，验证证据在 `docs/VALIDATION.md`，设计理由留在 PR/提交历史。需要新规格时放 `docs/` 并明确状态，不要发明审批状态。

## Git 与发布

- 默认分支 `main`；CI 在 push `main`、pull request 和 `v*` tag 上运行。提交信息使用英文，常见 `feat:`、`fix(scope):`、`docs:` 等前缀，也有直接描述式标题；仓库没有提交信息 lint，不添加 `Co-Authored-By`。
- 版本权威是根 `Cargo.toml` 的 `[workspace.package].version`。tag 必须是 `v<version>`，`scripts/package.py` 在 tag 构建中校验；含 `-` 的版本由 CI 发布为 prerelease。
- 发布说明正文由 `packaging/RELEASE_NOTES.md` 提供，CI 的 release job 在全部平台构建成功后创建或更新 GitHub Release。
- 任何版本号变更、tag 或发布前，先读 `.agents/tools/release/README.md`。不要移动或重建已发布 tag；发布完成以远端 tag、CI release job 和 Release 资产状态为准，不以本地命令成功为准。
- 当前没有 Windows Authenticode、macOS Developer ID 签名或 Apple 公证；不要声称安装包已签名。

## 环境

- 工具链由 `rust-toolchain.toml` 固定（channel 1.99.1，edition 2024，minimal + clippy/rustfmt）。Linux 开发库安装清单见 `.github/workflows/ci.yml`。
- Windows 是优先平台；macOS 宿主需要辅助功能权限；Linux 需要 EWMH 兼容窗口管理器。Wayland 的外部窗口控制明确不支持。
- 主程序和 `sean-toolbox-ui` 必须保持在一起；单实例按当前用户隔离。构建期 Python 只用于脚本，不进入应用运行时。

<!-- agent-scaffold:start (managed; edit outside) profile=default domains=docs,tools,testing,specs,terminology,git,release,environment -->
## Agent Harness

`.agents/` is the harness source; `.claude/` and `.codex/` hold generated projections. `CLAUDE.md` links to this contract.

### Session and task context

Honor the user's session entry; prefer task-local implementation/review. Resolve the task checkout and revision; use its `AGENTS.md` chain, terminology, skills, and tool working directories. Pass peers its absolute path and review revision. A shell `cd` does not reload host instructions or permissions; resolve access or guidance conflicts explicitly.

### Worktree-per-change (hard rule)

Never edit the primary worktree, including docs. Reuse an assigned linked worktree; otherwise, if the scaffold owns creation, run `bash .agents/tools/worktree.sh new <name>`.

Keep one lifecycle owner. `done --dir <absolute-wt>` merges, ff-only pushes, and removes the worktree; it requires explicit authorization and scaffold ownership, runs outside that worktree, and is not a PR/MR handoff. Leave externally managed worktrees to their owner instead of merging or cleaning them up. Bypass the trunk guard only with explicit user approval.

### Authority documents (hard rules)

`AGENTS.md` is the canonical repository-level Agent contract; read the applicable nested chain before acting. Keep it lean and current; route detail to project docs and nest only for real local differences. Repair durable guidance drift in the same change; follow higher-priority instructions and surface material disagreement instead of guessing. Interpret document status and freshness alongside repository evidence and user intent.

### Project terminology (hard rule)

Every Agent, project skill, and subagent uses the declared glossary, else root `CONTEXT-MAP.md`, then `CONTEXT.md`; read only relevant contexts before using project terms. A term and each language equivalent are equally valid names for one concept — use whichever is clearest and do not force one language. Reserve avoided names for history or compatibility. Resolve durable term changes with evidence and owner intent; update the glossary in the same change. Adopt an existing glossary; add definitions as durable concepts are resolved.

### Convention guides

Before the matching work, read its generic guide; project documentation and nested contracts win where they are more specific.

- Documentation: `.agents/conventions/docs.md`
- Commands and tools: `.agents/conventions/tools.md`
- Tests: `.agents/conventions/testing.md`
- Specifications and design records: `.agents/conventions/specs.md`
- Glossary changes: `.agents/conventions/terminology.md`
- Commits and delivery: `.agents/conventions/git.md`
- Version bumps, tags, and publication: `.agents/tools/release/README.md`
- Setup and shared environments: `.agents/conventions/environment.md`

### Sources and projections

- Edit skills in `.agents/skills/`, then run `bash .agents/relink-skills.sh`; commit source and symlinks.
- Edit subagents in `.agents/subagents/`, then run `python .agents/tools/generate-subagents.py`; commit source and projections.
- Do not hand-edit host projections or scaffold runtime (`.agents/tools/**`, `.agents/conventions/**`, `.agents/relink-skills.sh`, `.agents/symlink-manager.py`); use `agent-scaffold upgrade`, then `agent-scaffold verify`.
- **Third-party skills** follow project-owned placement and installation policy; preserve unrelated entries.

For Codex, confirm project trust, agent discovery, and `/hooks` approval; re-review changed definitions. Restore symlink/hardlink targets with Git, not Claude checkpoints.
<!-- agent-scaffold:end -->
