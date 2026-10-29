# toolbox-projects

The VS Code provider, identity policy, ranking and launch-request logic were adapted
from [`sean2077/recentry`](https://github.com/sean2077/recentry), commit
`7eb5b8ad2575fa58b3cdffc34a40381dd3311201`. Original MIT copyright and license
are retained in [LICENSE.recentry](LICENSE.recentry). The original contract tests
are retained in `tests/core_contract.rs` with the crate import renamed.

`discover`, `search` and `open` provide the UI-facing interface. `ProjectItem`
retains the typed original target internally, so displayed text is never reparsed
as a launch command. Discovery and opening are synchronous and belong on the UI's
on-demand worker; the background host does not load SQLite or scan for projects.

VS Code databases are opened read-only, with a 100 ms lock timeout. Ordinary recent
files are excluded. Local folders, `.code-workspace` files and VS Code remote URIs
retain their distinct launch arguments. Explicit executable arguments are passed
directly to `Command` without constructing a shell command line.

Known scope: VS Code Stable and its standard per-user data locations. Custom
`--user-data-dir`, custom profiles, portable data layouts and Code Insiders are not
yet discovered. Existing-window detection reads VS Code's last persisted window
state and expresses a launch preference, not a guarantee that the native window
manager focused the requested window.
