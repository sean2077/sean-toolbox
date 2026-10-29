//! Recent VS Code projects, adapted from sean2077/recentry (MIT).
//! Discovery is synchronous: the UI must call it on an on-demand worker.

mod model;
mod opener;
mod ranking;
mod service;
mod vscode;

pub use model::*;
pub use opener::*;
pub use ranking::*;
pub use service::{ProjectItem, discover, open, search};
pub use vscode::*;
