#[path = "../../packaging/build-version.rs"]
mod build_version;
#[path = "../../packaging/windows/icon-resource.rs"]
mod icon_resource;

fn main() {
    build_version::emit_build_version();
    icon_resource::embed_icon();
    slint_build::compile_with_config(
        "ui/main.slint",
        slint_build::CompilerConfiguration::new()
            .with_style("fluent".into())
            .with_bundled_translations("../../locales"),
    )
    .expect("compile Sean Toolbox UI");
}
