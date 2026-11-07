#[path = "../build/catalog.rs"]
pub mod catalog;

fn run() -> Result<(), String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../locales");
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [] => {
            let messages = catalog::load(&root)?;
            println!(
                "Validated {} Rust and {} UI messages in en/zh-CN",
                messages.rust.len(),
                messages.ui.len()
            );
            Ok(())
        }
        [command, extracted] if command == "compare-ui" => {
            catalog::compare_ui(&root.join("ui.pot"), std::path::Path::new(extracted))
        }
        _ => Err(
            "usage: cargo run --locked -p toolbox-i18n --example catalogs -- [compare-ui EXTRACTED.pot]"
                .into(),
        ),
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Translation validation failed: {error}");
        std::process::exit(1);
    }
}
