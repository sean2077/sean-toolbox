#[path = "build/catalog.rs"]
pub mod catalog;
#[path = "build/generate.rs"]
pub mod generate;

fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../locales");
    for path in ["rust.pot", "ui.pot", "zh-CN/LC_MESSAGES/toolbox-ui.po"] {
        println!("cargo:rerun-if-changed={}", root.join(path).display());
    }
    let messages =
        catalog::load(&root).unwrap_or_else(|error| panic!("translation catalog: {error}"));
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo OUT_DIR"))
        .join("messages.rs");
    std::fs::write(output, generate::rust(&messages.rust)).expect("write generated messages");
}
