# toolbox-projects

Reads VS Code's recent-project history, ranks search results and opens the selected
project. The UI calls this crate on its worker thread.

## Interface

| Function | Behavior |
| --- | --- |
| `discover` | Reads history from the current user's VS Code installation. An optional path selects the VS Code executable. |
| `search` | Ranks the supplied items in memory. An empty query preserves recent-use order. |
| `open` | Sends the item's original target to VS Code with the appropriate arguments. |

`ProjectItem` stores display fields alongside the typed original target. Opening
uses that target, including its folder, workspace or remote-URI type.

## Data and launching

Discovery uses **VS Code Stable** and its standard per-user data locations. It
opens SQLite databases read-only, with a 100 ms lock timeout, and reads project
entries. Insiders, portable layouts, custom `--user-data-dir` directories and
custom profiles require additional discovery support.

Folders use `--folder-uri`; `.code-workspace` files use `--file-uri`. VS Code
remote URIs retain their scheme and authority. If the target appears in VS Code's
saved window state, the open request omits `--new-window`; otherwise it requests a
new window. VS Code and the desktop window manager handle the resulting focus.
The executable and each argument are passed separately to `Command`.

## Tests and source

Run from the repository root:

```sh
cargo test --locked -p toolbox-projects
```

Tests cover target identity, URI parsing, ranking, history reads and launch
arguments. The provider, identity policy, ranking and opening logic were adapted
from [Recentry](https://github.com/sean2077/recentry), commit
`7eb5b8ad2575fa58b3cdffc34a40381dd3311201`. Its MIT notice is retained in
[LICENSE.recentry](LICENSE.recentry), and its original contract tests in
`tests/core_contract.rs`.
