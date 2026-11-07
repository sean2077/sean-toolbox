#[path = "../build/catalog.rs"]
pub mod catalog;
#[path = "../build/generate.rs"]
pub mod generate;

use std::path::Path;

const RUST: &str = r#"msgctxt "rust.greeting"
msgid "Hello, {name}!"
msgstr ""

msgctxt "rust.items"
msgid "{name} has one item"
msgid_plural "{name} has {count} items"
msgstr[0] ""
msgstr[1] ""
"#;

const UI: &str = r#"msgctxt "ui.selection"
msgid "Select {0} from {1}"
msgstr ""
"#;

const CHINESE: &str = r#"msgid ""
msgstr ""
"Language: zh-CN\n"
"Plural-Forms: nplurals=1; plural=0;\n"
"Content-Type: text/plain; charset=UTF-8\n"

msgctxt "rust.greeting"
msgid "Hello, {name}!"
msgstr "你好，{name}！"

msgctxt "rust.items"
msgid "{name} has one item"
msgid_plural "{name} has {count} items"
msgstr[0] "{name} 有 {count} 项"

msgctxt "ui.selection"
msgid "Select {0} from {1}"
msgstr "从 {1} 选择 {0}"
"#;

fn fixture(rust: &str, ui: &str, chinese: &str) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("zh-CN/LC_MESSAGES")).unwrap();
    std::fs::write(directory.path().join("rust.pot"), rust).unwrap();
    std::fs::write(directory.path().join("ui.pot"), ui).unwrap();
    std::fs::write(
        directory.path().join("zh-CN/LC_MESSAGES/toolbox-ui.po"),
        chinese,
    )
    .unwrap();
    directory
}

#[test]
fn parses_catalogs_and_allows_translators_to_reorder_arguments() {
    let directory = fixture(RUST, UI, CHINESE);
    let messages = catalog::load(directory.path()).unwrap();
    assert_eq!(messages.rust.len(), 2);
    assert_eq!(messages.ui.len(), 1);
    let plural = messages
        .rust
        .iter()
        .find(|message| message.id == "items")
        .unwrap();
    assert_eq!(plural.parameters, ["count", "name"]);
}

#[test]
fn catalog_validation_rejects_translation_regressions() {
    let cases = [
        (
            CHINESE.replace("你好，{name}！", ""),
            "translation is empty",
        ),
        (
            CHINESE.replace("你好，{name}！", "你好，{username}！"),
            "placeholder mismatch",
        ),
        (
            CHINESE.replace("你好，{name}！", "你好，{name:?}！"),
            "invalid named placeholder",
        ),
        (
            CHINESE.replace("从 {1} 选择 {0}", "选择 {0}"),
            "placeholder mismatch",
        ),
        (
            CHINESE.replace("从 {1} 选择 {0}", "选择 {} {1}"),
            "cannot be mixed",
        ),
        (
            CHINESE.replace("你好，{name}！", "你好，{name！"),
            "unclosed placeholder",
        ),
        (
            CHINESE.replace(
                "msgctxt \"rust.greeting\"",
                "#, fuzzy\nmsgctxt \"rust.greeting\"",
            ),
            "fuzzy translation",
        ),
        (
            CHINESE.replace("Hello, {name}!", "Welcome, {name}!"),
            "PO source differs",
        ),
        (
            CHINESE.replace("rust.greeting", "rust.unexpected"),
            "catalog keys differ",
        ),
        (
            CHINESE.replace("nplurals=1; plural=0;", "nplurals=2; plural=(n != 1);"),
            "Plural-Forms",
        ),
        (
            format!(
                "{CHINESE}\nmsgctxt \"rust.greeting\"\nmsgid \"Hello, {{name}}!\"\nmsgstr \"重复 {{name}}\"\n"
            ),
            "duplicate translation",
        ),
        (
            CHINESE.replace(
                "msgstr[0] \"{name} 有 {count} 项\"",
                "msgstr[0] \"{name} 有 {count} 项\"\nmsgstr[1] \"{name} 有 {count} 项\"",
            ),
            "exactly msgstr[0]",
        ),
    ];
    for (chinese, expected_error) in cases {
        let directory = fixture(RUST, UI, &chinese);
        let error = catalog::load(directory.path()).unwrap_err();
        assert!(
            error.contains(expected_error),
            "expected {expected_error:?}, got {error:?}"
        );
    }
}

#[test]
fn rejects_duplicate_and_invalid_source_ids_and_inconsistent_plural_arguments() {
    for (rust, expected_error) in [
        (format!("{RUST}\n{RUST}"), "duplicate message context"),
        (RUST.replace("rust.greeting", "rust.self"), "snake_case"),
        (
            RUST.replace("{name} has {count} items", "{username} has {count} items"),
            "singular/plural placeholders differ",
        ),
    ] {
        let directory = fixture(&rust, UI, CHINESE);
        let error = catalog::load(directory.path()).unwrap_err();
        assert!(
            error.contains(expected_error),
            "expected {expected_error:?}, got {error:?}"
        );
    }
}

#[test]
fn obsolete_translations_are_ignored_and_semantic_extraction_drift_is_detected() {
    let directory = fixture(
        RUST,
        UI,
        &format!("{CHINESE}\n#~ msgctxt \"rust.old\"\n#~ msgid \"Old\"\n#~ msgstr \"旧\"\n"),
    );
    catalog::load(directory.path()).unwrap();
    let extracted = directory.path().join("extracted.pot");
    std::fs::write(
        &extracted,
        format!("# Different extraction date and line numbers\n{UI}"),
    )
    .unwrap();
    catalog::compare_ui(&directory.path().join("ui.pot"), &extracted).unwrap();
    std::fs::write(
        &extracted,
        UI.replace("Select {0} from {1}", "Choose {0} from {1}"),
    )
    .unwrap();
    assert!(
        catalog::compare_ui(&directory.path().join("ui.pot"), &extracted)
            .unwrap_err()
            .contains("stale")
    );
}

#[test]
fn format_subset_handles_braces_and_rejects_unsupported_specifiers() {
    let parsed = catalog::template("{{JSON}}：{name} → {name}", catalog::Syntax::Rust).unwrap();
    assert_eq!(parsed.parameters, ["name"]);
    assert_eq!(
        catalog::template("{{}}", catalog::Syntax::Rust)
            .unwrap()
            .literal,
        "{}"
    );
    for invalid in [
        "{}",
        "{0}",
        "{name:08}",
        "{name:?}",
        "{other.name}",
        "{fn}",
        "name}",
    ] {
        assert!(
            catalog::template(invalid, catalog::Syntax::Rust).is_err(),
            "accepted {invalid}"
        );
    }
}

#[test]
fn generated_code_preserves_literal_braces_and_reorders_named_arguments() {
    let directory = tempfile::tempdir().unwrap();
    let messages = [catalog::RustMessage {
        id: "example".into(),
        english: "{{JSON}} {first} then {second}".into(),
        plural: None,
        chinese: "{{JSON}} {second}，再 {first}".into(),
        parameters: vec!["first".into(), "second".into()],
    }];
    let generated = generate::rust(&messages);
    let source = format!(
        r#"
        #[derive(Clone, Copy)] pub enum Locale {{ En, ZhCn }}
        {generated}
        fn main() {{
            let message = messages::example("{{second}}", "工具");
            assert_eq!(message.render(Locale::En), "{{JSON}} {{second}} then 工具");
            assert_eq!(message.render(Locale::ZhCn), "{{JSON}} 工具，再 {{second}}");
        }}
    "#
    );
    compile_and_run(directory.path(), &source);
}

#[test]
fn editing_english_word_order_preserves_constructor_signatures_and_values() {
    let extra_rust = r#"
msgctxt "rust.pair"
msgid "{first} then {second}"
msgstr ""

msgctxt "rust.plural_pair"
msgid "{second} gave {apple} one item"
msgid_plural "{second} gave {apple} {count} items"
msgstr[0] ""
msgstr[1] ""
"#;
    let extra_chinese = r#"
msgctxt "rust.pair"
msgid "{first} then {second}"
msgstr "{first} / {second}"

msgctxt "rust.plural_pair"
msgid "{second} gave {apple} one item"
msgid_plural "{second} gave {apple} {count} items"
msgstr[0] "{apple} / {second}: {count}"
"#;
    let before = fixture(
        &format!("{RUST}\n{extra_rust}"),
        UI,
        &format!("{CHINESE}\n{extra_chinese}"),
    );
    let reword = |text: &str| {
        text.replace("{first} then {second}", "{second} then {first}")
            .replace(
                "{second} gave {apple} one item",
                "{apple} received one item from {second}",
            )
            .replace(
                "{second} gave {apple} {count} items",
                "{apple} received {count} items from {second}",
            )
    };
    let after = fixture(
        &format!("{RUST}\n{}", reword(extra_rust)),
        UI,
        &format!("{CHINESE}\n{}", reword(extra_chinese)),
    );
    let before_code = generate::rust(&catalog::load(before.path()).unwrap().rust);
    let after_code = generate::rust(&catalog::load(after.path()).unwrap().rust);
    for signature in [
        "pub fn pair(first: impl std::fmt::Display, second: impl std::fmt::Display) -> Message",
        "pub fn plural_pair(count: usize, apple: impl std::fmt::Display, second: impl std::fmt::Display) -> Message",
    ] {
        assert!(
            before_code.contains(signature),
            "missing original signature {signature}"
        );
        assert!(
            after_code.contains(signature),
            "rewording changed signature {signature}"
        );
    }
    let source = format!(
        r#"
        #[derive(Clone, Copy)] pub enum Locale {{ En, ZhCn }}
        mod before {{ use super::Locale; {before_code} }}
        mod after {{ use super::Locale; {after_code} }}
        fn main() {{
            let old_pair = before::messages::pair("A", "Z");
            let new_pair = after::messages::pair("A", "Z");
            assert_eq!(old_pair.render(Locale::En), "A then Z");
            assert_eq!(new_pair.render(Locale::En), "Z then A");
            assert_eq!(old_pair.render(Locale::ZhCn), "A / Z");
            assert_eq!(new_pair.render(Locale::ZhCn), "A / Z");

            let old_plural = before::messages::plural_pair(3, "A", "Z");
            let new_plural = after::messages::plural_pair(3, "A", "Z");
            assert_eq!(old_plural.render(Locale::En), "Z gave A 3 items");
            assert_eq!(new_plural.render(Locale::En), "A received 3 items from Z");
            assert_eq!(old_plural.render(Locale::ZhCn), "A / Z: 3");
            assert_eq!(new_plural.render(Locale::ZhCn), "A / Z: 3");
        }}
    "#
    );
    compile_and_run(before.path(), &source);
}

fn compile_and_run(directory: &Path, source: &str) {
    let source_path = directory.join("generated.rs");
    std::fs::write(&source_path, source).unwrap();
    let binary = directory.join(format!("generated{}", std::env::consts::EXE_SUFFIX));
    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let result = std::process::Command::new(compiler)
        .args(["--edition=2024", "--crate-name", "generated_messages"])
        .arg(&source_path)
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    run(&binary);
}

fn run(binary: &Path) {
    let result = std::process::Command::new(binary).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
