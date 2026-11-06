# toolbox-skills

Host-owned agent skill storage, GitHub discovery, safe per-app copy deployment,
and persistent update scheduling. This crate has no GUI, SQLite, HTTP runtime,
or resident network loop. The host owns the worker and exposes the models from
`toolbox_core::skills` through IPC.

## Runtime contract

`SkillsService::open(storage_root, home, cancel)` opens one manager protected by
an OS file lock. `execute(command, unix_seconds)` performs one operation;
`snapshot()` returns display state. `next_due()` and `run_daily(unix_seconds)`
let the host schedule work without polling repositories. All calls except
reading the host's cached snapshot belong on its skills worker.

Git is required on `PATH` for repository discovery and updates. Inputs accept
`owner/repo` or a credential-free `https://github.com/owner/repo` URL, plus an
optional branch. Git uses existing non-interactive credentials; it does not
prompt for login. Sources are shallow bare clones at an immutable revision.
An explicit refresh or update replaces the cached snapshot and invalidates its
old discovery rows. No repository checkout, skill script, hook, filter, or
submodule is executed. Git authentication helpers configured by the user remain
part of their trusted Git installation.

Discovery stops at a `SKILL.md` package boundary and retains all package files,
including nested examples. The metadata reader supports common scalar and
multiline frontmatter name/description fields, with the directory name as a
fallback; it is not a general YAML evaluator. Install from a repository or a
concrete local skill directory. Local scanning includes the five app roots,
`~/.cc-switch/skills`, and `~/.agents/skills`. Local imports keep their source
folder and have no remote update source.

## Storage and deployment

The host supplies `<configuration-directory>/skills` as the private root:

- `state.json`: source repositories, installed packages, ownership fingerprints,
  enabled app copies, update timestamps, and diagnostics.
- `managed/<directory>`: one master copy per installed skill.
- `temporary/`: disposable repository and skill staging directories.
- `manager.lock`: single-writer exclusion.
- `pending.json`: an interrupted filesystem transaction, when present.

The default app destinations are `~/.claude/skills`, `~/.codex/skills`,
`~/.gemini/skills`, `~/.config/opencode/skills`, and `~/.grok/skills`.
These are per-user default paths, not project-local roots or custom harness
configuration directories. Copies work on Windows without Developer Mode.
The shared `.agents/skills` directory is only a read-only import source.
Some harnesses also discover another harness's directory: switches report
Sean Toolbox's managed copies, rather than guaranteeing a skill cannot be
discovered through another location.

New deployments require an absent destination, including when an existing
unmanaged folder has identical content. Existing files are never silently
adopted. Imported originals therefore survive uninstall. Updating or removing
an owned copy checks its SHA-256 tree fingerprint first; local edits, links,
reparse points, and conflicting names produce errors without overwriting them.
POSIX executable bits are preserved and included in the fingerprint.

Each skill transaction stages all requested copies before changing any live
directory, records a journal, swaps directories by rename, then atomically
saves state. A failed save restores the old directories. Startup recovers an
interrupted transaction before allowing mutations. Recovery refuses to discard
unexpected edits. Temporary-copy cleanup failures preserve the leftover path
and report a warning; they do not invalidate committed state or permanently
block the manager. Only one skill is transacted at a time, so a batch can retain
successful updates while reporting failed skills and retrying later.

## Daily update behavior

Daily updates are disabled initially. Enabling schedules an immediate check;
a successful check schedules the next one 24 hours later. Any failed skill or
source schedules another attempt after one hour. Attempts and successful
checks are persisted separately. A host restart catches up overdue work once;
the host must be running for scheduled work, and no OS scheduler is installed.
Only repository-backed installed skills participate; disabled app copies stay
disabled and local imports are skipped. Manual update-all also advances the
daily schedule; updating one skill does not postpone the next full check.

## Resource limits

There are at most 16 repositories and 250 managed skills. Discovery and local
scan lists each contain at most 250 rows. A serialized display snapshot is
limited to 700 KiB, with `truncated` set when rows must be omitted for IPC.
State is limited to 1 MiB. Skill packages are limited to 32 MiB, 8 MiB per file,
4096 entries, 20 directory levels, and portable paths. Repository archives are
limited to 64 MiB and 20000 entries; links, submodules, invalid paths, and
oversized package files are rejected before installation.

Git commands have a 90-second deadline, bounded output, and a sampled 256 MiB
working-directory budget. On Unix they own a process group; on Windows they
start suspended, join a kill-on-close job, then resume. Cancellation terminates
the command and its helpers. Brief filesystem growth between budget samples
is possible; this is a bounded-work safeguard, not an OS disk quota.

## Verification

Run `cargo test --locked -p toolbox-skills` from the workspace. Fixtures create
real local Git repositories and exercise the production archive and file
transaction paths without contacting GitHub. Coverage includes installation,
per-app disable, updates and restart, offline retry, local edits, identical
unmanaged collisions, local imports, source changes, path/link rejection,
executable files, lock/corrupt-state handling, IPC limits, and interrupted or
partially cleaned transactions. These tests do not certify real GitHub
authentication or platform-specific desktop behavior.
