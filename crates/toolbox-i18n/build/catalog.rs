//! Shared build-time validation. The application library does not link this code.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use rspolib::{POEntry, POFile};

/// Parameterless messages owned by the Slint catalog but shared with Rust.
/// Slint uses the context directly; Rust receives a generated `common_*`
/// constructor from the same translation.
pub const SHARED_PREFIX: &str = "ui.common.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub context: String,
    pub english: String,
    pub plural: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RustMessage {
    pub id: String,
    pub english: String,
    pub plural: Option<String>,
    pub chinese: String,
    /// Stable API order: alphabetical, with count first for plurals.
    pub parameters: Vec<String>,
}

#[derive(Debug)]
pub struct Catalog {
    pub rust: Vec<RustMessage>,
    pub ui: BTreeMap<String, Source>,
}

#[derive(Clone, Copy, Debug)]
pub enum Syntax {
    Rust,
    Slint { plural: bool },
}

#[derive(Debug)]
pub struct Template {
    pub parameters: Vec<String>,
    /// Decoded literal text; useful only when there are no parameters.
    pub literal: String,
}

/// Parse only the documented subset, rejecting format specifiers and expressions.
pub fn template(text: &str, syntax: Syntax) -> Result<Template, String> {
    let mut chars = text.chars().peekable();
    let mut parameters = Vec::new();
    let mut literal = String::new();
    let mut automatic = 0;
    let mut has_automatic = false;
    let mut has_position = false;
    while let Some(ch) = chars.next() {
        match ch {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                literal.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                literal.push('}');
            }
            '{' => {
                let mut name = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some('{') => return Err("nested opening brace in placeholder".into()),
                        Some(ch) => name.push(ch),
                        None => return Err("unclosed placeholder".into()),
                    }
                }
                match syntax {
                    Syntax::Rust if !identifier(&name) => {
                        return Err(format!("invalid named placeholder {{{name}}}"));
                    }
                    Syntax::Slint { .. } if name.is_empty() => {
                        has_automatic = true;
                        name = automatic.to_string();
                        automatic += 1;
                    }
                    Syntax::Slint { .. } if name.bytes().all(|ch| ch.is_ascii_digit()) => {
                        has_position = true;
                        name = name
                            .parse::<usize>()
                            .map_err(|_| "placeholder index is too large")?
                            .to_string();
                    }
                    Syntax::Slint { plural: true } if name == "n" => {}
                    Syntax::Slint { .. } => {
                        return Err(format!("invalid Slint placeholder {{{name}}}"));
                    }
                    Syntax::Rust => {}
                }
                if !parameters.contains(&name) {
                    parameters.push(name);
                }
            }
            '}' => return Err("unescaped closing brace".into()),
            other => literal.push(other),
        }
    }
    if has_automatic && has_position {
        return Err("automatic and indexed placeholders cannot be mixed".into());
    }
    Ok(Template {
        parameters,
        literal,
    })
}

pub fn identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z'))
        && bytes.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == b'_')
        && !value.ends_with('_')
        && !value.contains("__")
        && !matches!(
            value,
            "as" | "async"
                | "await"
                | "break"
                | "const"
                | "continue"
                | "crate"
                | "dyn"
                | "else"
                | "enum"
                | "extern"
                | "false"
                | "fn"
                | "for"
                | "if"
                | "impl"
                | "in"
                | "let"
                | "loop"
                | "match"
                | "mod"
                | "move"
                | "mut"
                | "pub"
                | "ref"
                | "return"
                | "self"
                | "static"
                | "struct"
                | "super"
                | "trait"
                | "true"
                | "type"
                | "unsafe"
                | "use"
                | "where"
                | "while"
                | "abstract"
                | "become"
                | "box"
                | "do"
                | "final"
                | "gen"
                | "macro"
                | "override"
                | "priv"
                | "try"
                | "typeof"
                | "unsized"
                | "virtual"
                | "yield"
        )
}

fn read(path: &Path) -> Result<POFile, String> {
    // Reject invalid UTF-8 before parsing; byte input is used only by rspolib's MO reader.
    let content =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    rspolib::pofile(content.as_str()).map_err(|error| format!("{}: {error}", path.display()))
}

fn context(entry: &POEntry) -> Result<&str, String> {
    entry
        .msgctxt
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("line {}: missing explicit msgctxt", entry.linenum))
}

pub fn sources(path: &Path, prefix: &str) -> Result<BTreeMap<String, Source>, String> {
    let catalog = read(path)?;
    let mut result = BTreeMap::new();
    for entry in catalog.entries.iter().filter(|entry| !entry.obsolete) {
        let key = context(entry)?;
        let id = key
            .strip_prefix(prefix)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| format!("{key}: context must start with {prefix}"))?;
        if prefix == "rust." && !identifier(id) {
            return Err(format!(
                "{key}: Rust message ID must be a snake_case identifier"
            ));
        }
        if entry.msgid.trim().is_empty() {
            return Err(format!("{key}: source message is empty"));
        }
        if entry.flags.iter().any(|flag| flag == "fuzzy") {
            return Err(format!("{key}: source message is fuzzy"));
        }
        if entry.msgstr.as_ref().is_some_and(|value| !value.is_empty())
            || entry.msgstr_plural.iter().any(|value| !value.is_empty())
        {
            return Err(format!("{key}: translations belong in the PO, not the POT"));
        }
        let source = Source {
            context: key.into(),
            english: entry.msgid.clone(),
            plural: entry.msgid_plural.clone(),
        };
        parameters(&source, prefix == "rust.")?;
        if result.insert(key.into(), source).is_some() {
            return Err(format!("{key}: duplicate message context"));
        }
    }
    Ok(result)
}

fn parameters(source: &Source, rust: bool) -> Result<Vec<String>, String> {
    let syntax = if rust {
        Syntax::Rust
    } else {
        Syntax::Slint {
            plural: source.plural.is_some(),
        }
    };
    let mut names = template(&source.english, syntax)
        .map_err(|error| format!("{}: {error}", source.context))?
        .parameters;
    if let Some(plural) = &source.plural {
        if plural.trim().is_empty() {
            return Err(format!("{}: plural source is empty", source.context));
        }
        let plural_names = template(plural, syntax)
            .map_err(|error| format!("{} plural: {error}", source.context))?
            .parameters;
        let count = if rust { "count" } else { "n" };
        let non_count = |names: &[String]| {
            names
                .iter()
                .filter(|name| *name != count)
                .cloned()
                .collect::<BTreeSet<_>>()
        };
        if non_count(&names) != non_count(&plural_names) {
            return Err(format!(
                "{}: singular/plural placeholders differ",
                source.context
            ));
        }
        for name in plural_names {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    if rust {
        // Editing English word order must not silently reorder positional Rust
        // arguments that all implement Display and would still type-check.
        names.sort();
        if source.plural.is_some() {
            // A plural always has a count even when both forms spell it out.
            names.retain(|name| name != "count");
            names.insert(0, "count".into());
        }
    }
    Ok(names)
}

fn translated(entry: &POEntry, source: &Source) -> Result<String, String> {
    let key = &source.context;
    if entry.msgid != source.english || entry.msgid_plural != source.plural {
        return Err(format!(
            "{key}: PO source differs from its POT; run translations.py update"
        ));
    }
    if entry.flags.iter().any(|flag| flag == "fuzzy") {
        return Err(format!("{key}: fuzzy translation must be reviewed"));
    }
    let text = if source.plural.is_some() {
        if entry.msgstr_plural.len() != 1
            || entry.msgstr.as_ref().is_some_and(|value| !value.is_empty())
        {
            return Err(format!("{key}: Chinese plurals require exactly msgstr[0]"));
        }
        &entry.msgstr_plural[0]
    } else {
        if !entry.msgstr_plural.is_empty() {
            return Err(format!(
                "{key}: singular translation cannot contain plural forms"
            ));
        }
        entry.msgstr.as_deref().unwrap_or_default()
    };
    if text.trim().is_empty() {
        return Err(format!("{key}: translation is empty"));
    }
    let rust = key.starts_with("rust.");
    let syntax = if rust {
        Syntax::Rust
    } else {
        Syntax::Slint {
            plural: source.plural.is_some(),
        }
    };
    let translated = template(text, syntax).map_err(|error| format!("{key}: {error}"))?;
    let expected = parameters(source, rust)?
        .into_iter()
        .collect::<BTreeSet<_>>();
    let actual = translated.parameters.into_iter().collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(format!(
            "{key}: placeholder mismatch: expected {expected:?}, got {actual:?}"
        ));
    }
    Ok(text.into())
}

pub fn load(root: &Path) -> Result<Catalog, String> {
    let rust = sources(&root.join("rust.pot"), "rust.")?;
    let ui = sources(&root.join("ui.pot"), "ui.")?;
    let translated_catalog = read(&root.join("zh-CN/LC_MESSAGES/toolbox-ui.po"))?;
    if translated_catalog
        .metadata
        .get("Language")
        .map(String::as_str)
        != Some("zh-CN")
    {
        return Err("Chinese catalog must declare Language: zh-CN".into());
    }
    let plurals = translated_catalog
        .metadata
        .get("Plural-Forms")
        .map(|value| {
            value
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .collect::<String>()
        });
    if plurals.as_deref() != Some("nplurals=1;plural=0;") {
        return Err("Chinese catalog must declare Plural-Forms: nplurals=1; plural=0;".into());
    }
    if translated_catalog.metadata_is_fuzzy {
        return Err("Chinese catalog header is fuzzy".into());
    }
    let mut translations = BTreeMap::new();
    for entry in translated_catalog
        .entries
        .iter()
        .filter(|entry| !entry.obsolete)
    {
        let key = context(entry)?;
        if translations.insert(key.to_owned(), entry).is_some() {
            return Err(format!("{key}: duplicate translation context"));
        }
    }
    let expected = rust.keys().chain(ui.keys()).collect::<BTreeSet<_>>();
    let actual = translations.keys().collect::<BTreeSet<_>>();
    if expected != actual {
        return Err(format!(
            "catalog keys differ; missing: {:?}; unexpected: {:?}",
            expected.difference(&actual).collect::<Vec<_>>(),
            actual.difference(&expected).collect::<Vec<_>>()
        ));
    }
    let mut messages = Vec::new();
    let mut variants = BTreeSet::new();
    for source in rust.values().chain(ui.values()) {
        let chinese = translated(translations[&source.context], source)?;
        if let Some(id) = source.context.strip_prefix("rust.") {
            if !variants.insert(variant(id)) {
                return Err(format!(
                    "{}: generated enum variant collides",
                    source.context
                ));
            }
            messages.push(RustMessage {
                id: id.into(),
                english: source.english.clone(),
                plural: source.plural.clone(),
                chinese,
                parameters: parameters(source, true)?,
            });
        } else if let Some(id) = source.context.strip_prefix(SHARED_PREFIX) {
            let id = format!("common_{id}");
            if source.plural.is_some() {
                return Err(format!(
                    "{}: shared messages must not be plural",
                    source.context
                ));
            }
            if !parameters(source, false)?.is_empty() {
                return Err(format!(
                    "{}: shared messages must not contain placeholders",
                    source.context
                ));
            }
            if !identifier(&id) {
                return Err(format!(
                    "{}: generated Rust message ID must be a snake_case identifier",
                    source.context
                ));
            }
            if !variants.insert(variant(&id)) {
                return Err(format!(
                    "{}: generated enum variant collides",
                    source.context
                ));
            }
            messages.push(RustMessage {
                id,
                english: source.english.clone(),
                plural: None,
                chinese,
                parameters: Vec::new(),
            });
        }
    }
    Ok(Catalog { rust: messages, ui })
}

pub fn variant(id: &str) -> String {
    id.split('_')
        .map(|part| {
            let mut chars = part.chars();
            let first = chars
                .next()
                .expect("validated message identifier")
                .to_ascii_uppercase();
            format!("{first}{}", chars.as_str())
        })
        .collect()
}

/// Compare semantic entries, ignoring extraction timestamps and source line shifts.
pub fn compare_ui(expected: &Path, extracted: &Path) -> Result<(), String> {
    let expected = sources(expected, "ui.")?;
    let extracted = sources(extracted, "ui.")?;
    if expected != extracted {
        let changed = expected
            .keys()
            .chain(extracted.keys())
            .filter(|key| expected.get(*key) != extracted.get(*key))
            .collect::<BTreeSet<_>>();
        return Err(format!(
            "UI template is stale for {changed:?}; run scripts/translations.py update"
        ));
    }
    Ok(())
}
