#[path = "../../packaging/windows/icon-resource.rs"]
mod icon_resource;

fn main() {
    icon_resource::embed_icon();
    slint_build::compile_with_config(
        "ui/main.slint",
        slint_build::CompilerConfiguration::new().with_style("fluent".into()),
    )
    .expect("compile Sean Toolbox UI");
}
