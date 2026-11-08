#[path = "../../packaging/build-version.rs"]
mod build_version;
#[path = "../../packaging/windows/icon-resource.rs"]
mod icon_resource;

fn main() {
    build_version::emit_build_version();
    icon_resource::embed_icon();
}
