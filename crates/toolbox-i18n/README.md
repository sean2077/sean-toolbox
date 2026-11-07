# toolbox-i18n

Typed application messages generated from gettext resources. The library has
no runtime dependencies and can be linked by the resident host without Slint.

## Resources and API

- `../../locales/rust.pot` owns the English Rust messages and stable
  `rust.snake_case` contexts.
- `../../locales/ui.pot` is extracted by the matching official
  `slint-tr-extractor`; edit `.slint` sources rather than this generated template.
- `../../locales/zh-CN/LC_MESSAGES/toolbox-ui.po` contains Chinese translations
  for both templates. Slint bundles this same PO into the UI.

The build script uses the already locked `rspolib` parser to validate these
resources and generate `Message` variants and `messages::*` constructors:

```rust
use toolbox_i18n::{Locale, messages};

let message = messages::projects_count(3, 2);
assert_eq!(message.render(Locale::En), "2 shown · 3 recent projects");
assert_eq!(message.render(Locale::ZhCn), "显示 2 项 · 共 3 个最近项目");
```

Keep messages and their arguments until presentation, rather than storing their
rendered text. Rendering takes an explicit locale and does not mutate global
state. Configuration tags remain `en` and `zh-CN`; unknown tags use English.

Rust templates accept named `{argument}` placeholders and literal `{{` / `}}`.
Other format specifiers are rejected. Translations may reorder or repeat named
arguments. Arguments are captured with `Display`, then passed to generated Rust
`format!` expressions; argument text is never parsed as a template.

Constructor parameters are sorted alphabetically by their placeholder names.
This is a stable API rule, independent of the order of placeholders in either
language: editing a sentence's word order cannot swap the values passed by Rust
callers. Renaming a placeholder changes the API and requires reviewing callers.

A Rust plural has `msgid` and `msgid_plural` in the POT, and one Chinese
`msgstr[0]`. Its constructor takes `count: usize` first, followed by the remaining
arguments sorted alphabetically by name. English selects the singular
only for one; Chinese uses its single form. Other languages are not yet supported
by this generator. Slint templates use Slint's own `{0}` / `{1}` and `{n}` syntax.

## Maintenance

Install GNU gettext tools and the exact `slint-tr-extractor` version pinned in
`crates/toolbox-ui/Cargo.toml`. Normal builds do not need these command-line tools.

From the repository root:

```sh
python scripts/translations.py update
# Complete/review new Chinese PO entries.
python scripts/translations.py check
cargo test --locked -p toolbox-i18n
```

`update` extracts UI messages and uses `msgcat` / `msgmerge` to combine templates
and preserve existing translations. It disables fuzzy matching, so changes need
explicit translator review. `check` compares the freshly extracted UI entries
semantically and validates catalogs. Cargo builds also reject duplicate/missing
IDs, empty/fuzzy translations, stale source text, invalid Chinese plural headers,
and mismatched placeholders. Obsolete gettext entries may remain for history.

The checker and generator tests include malformed catalogs, reordered arguments,
singular/plural selection, literal braces, and arguments containing Windows paths
and placeholder-like text.
