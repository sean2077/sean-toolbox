# Sean Toolbox Agent 指南

## 项目与入口

Rust workspace + Slint 桌面工具箱，Windows 优先。[README](README.md) 介绍产品与下载，[使用指南](docs/USAGE.md) 说明当前行为和配置，[开发指南](docs/DEVELOPMENT.md) 说明构建、测试与模块结构。术语见 [CONTEXT.md](CONTEXT.md)。

常驻宿主 `toolbox-host`（`sean-toolbox`）负责单实例、快捷键、托盘、配置保存和窗口 worker；Slint、SQLite 与项目查询位于按需启动的 `toolbox-ui`（`sean-toolbox-ui`）中。两个程序配套分发。

## 开发与验证

仓库根目录的标准检查：

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
```

- 工具链由 `rust-toolchain.toml` 固定，验证使用 `--locked`。依赖变更时由 Cargo 更新锁文件；无关修改保持现有依赖。
- 上述三项是最低门禁。CI 的平台矩阵与依赖安装见 [.github/workflows/ci.yml](.github/workflows/ci.yml)。Windows 本地构建的 MSVC/GNU 选择见开发指南。
- X11 用例标为 ignored，由 `crates/toolbox-windows/tests/run-x11-smoke.sh` 运行。宿主与界面测试入口是 `scripts/smoke_host.py`、`scripts/smoke_appearance.py`，参数见开发指南。
- 桌面集成测试使用独立 Xvfb、Openbox、xterm 和临时用户目录。测试配置用绝对路径 `SEAN_TOOLBOX_CONFIG` 指定，保持用户现有配置和桌面状态。
- 窗口焦点、系统授权、缩放、多显示器和文件对话框在对应系统验收。VS Code fixture 测试覆盖历史数据和启动参数，报告中注明验证范围。
- 单元测试随模块放置，跨模块测试放 `crates/*/tests/`。按行为风险选择测试或桌面验收，项目未要求强制 TDD。
- `docs/VALIDATION.md` 保存带日期的历史结果；新结果追加，保留旧记录的环境、数据和结论。

## 源码与文档

- 手写代码位于 `crates/*/src/`、`crates/toolbox-ui/ui/main.slint` 和 `scripts/`。`target/`、`dist/`、许可汇总及 Slint 生成代码通过对应命令更新。
- `vendor/global-hotkey` 是 X11 解绑修复的补丁依赖，来源与改动记录在 `PATCH.md`。依赖许可补充文本按精确版本记录在 `licenses/dependencies/manifest.json`，升级时核对原文和哈希。
- `.agents/**`、`.claude/**`、`.codex/**` 和 `CLAUDE.md` 由 agent-scaffold 管理，使用其 `upgrade`、`verify` 更新。项目指导写在本文件 managed block 之外。
- 根 README 面向新用户；操作、CLI 与配置由 `docs/USAGE.md` 维护；构建和架构由 `docs/DEVELOPMENT.md` 维护；模块细节放对应 crate README。打包与发布正文分别在 `packaging/README.md`、`packaging/RELEASE_NOTES.md`。
- 新增长期文档放 `docs/` 并从现有入口链接，一次性计划和设计讨论留在 issue、PR 或提交记录。规格文档标明当前状态。
- 根 README、`docs/` 和本文件使用简体中文；crate README、代码注释和提交信息使用英文。来源和许可记录在 `THIRD_PARTY_NOTICES.md` 与 `licenses/`。

## Git 与发布

- 默认分支为 `main`。提交信息使用英文，可用 `feat:`、`fix(scope):`、`docs:` 等前缀，不添加 `Co-Authored-By`。
- 版本以根 `Cargo.toml` 的 `[workspace.package].version` 为准，tag 为 `v<version>`；带 `-` 的版本由 CI 发布为 prerelease。
- 版本号、tag 或发布操作先读 `.agents/tools/release/README.md`。已发布 tag 保持不动，发布结果以远端 tag、CI release job 和资产状态为准。
- 当前发行包未进行 Windows Authenticode、macOS Developer ID 签名或 Apple 公证。

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
