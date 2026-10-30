#[path = "../../packaging/windows/icon-resource.rs"]
mod icon_resource;

fn main() {
    icon_resource::embed_icon();
}
