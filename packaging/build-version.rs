use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

/// Expose the checkout's `git describe` as `SEAN_TOOLBOX_VERSION`, for example
/// `v0.2.0`, `v0.2.0-26-gaf25bd0` or `v0.2.0-26-gaf25bd0-dirty`.
///
/// Without Git or a reachable `v*` tag (source archives and the shallow CI
/// build checkouts that produce release assets), fall back to the package
/// version, so published binaries keep their exact `v<version>`.
pub fn emit_build_version() {
    println!("cargo:rerun-if-changed=../../packaging/build-version.rs");
    let manifest =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"));
    watch_git(&manifest);
    let version = git(
        &manifest,
        &["describe", "--tags", "--match", "v[0-9]*", "--dirty"],
    )
    .unwrap_or_else(|| {
        format!(
            "v{}",
            env::var("CARGO_PKG_VERSION").expect("package version")
        )
    });
    println!("cargo:rustc-env=SEAN_TOOLBOX_VERSION={version}");
}

fn git(directory: &Path, arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (output.status.success() && !text.is_empty()).then(|| text.to_owned())
}

/// Re-run when commits, checkouts, tags or staged changes alter the result.
/// Unstaged edits are noticed once the index is next refreshed.
fn watch_git(manifest: &Path) {
    let Some(directories) = git(
        manifest,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
        ],
    ) else {
        return;
    };
    let mut lines = directories.lines().map(PathBuf::from);
    let (Some(git_dir), Some(common)) = (lines.next(), lines.next()) else {
        return;
    };
    let mut paths = vec![
        git_dir.join("HEAD"),
        git_dir.join("index"),
        common.join("packed-refs"),
        common.join("refs").join("tags"),
    ];
    if let Some(branch) = git(manifest, &["symbolic-ref", "-q", "HEAD"]) {
        paths.push(common.join(branch));
    }
    // Cargo treats a missing path as always changed, so watch existing ones only.
    for path in paths.into_iter().filter(|path| path.exists()) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}
