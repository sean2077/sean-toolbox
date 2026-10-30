use std::{env, fs, path::PathBuf, process::Command};

/// Embed the same multi-resolution icon in both Windows executables.
/// Ordinary builds consume the committed ICO, without an image toolchain.
pub fn embed_icon() {
    println!("cargo:rerun-if-changed=../../packaging/windows/icon-resource.rs");
    println!("cargo:rerun-if-changed=../../packaging/icons/sean-toolbox.ico");
    println!("cargo:rerun-if-env-changed=RC");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let target = env::var("TARGET").expect("Cargo target");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"));
    let manifest =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"));
    let icon = manifest.join("../../packaging/icons/sean-toolbox.ico");
    let script = out.join("sean-toolbox-icon.rc");
    let icon_path = icon.to_string_lossy().replace('\\', "/");
    fs::write(&script, format!("1 ICON \"{icon_path}\"\n"))
        .expect("write Windows icon resource script");

    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let resource = out.join("sean-toolbox-icon.res");
        let mut compiler = env::var_os("RC").map(Command::new).unwrap_or_else(|| {
            cc::windows_registry::find(&target, "rc.exe").unwrap_or_else(|| Command::new("rc.exe"))
        });
        compiler
            .args(["/nologo", "/c65001", "/fo"])
            .arg(&resource)
            .arg(&script);
        compile(&mut compiler, "Windows SDK resource compiler (rc.exe)");
        println!("cargo:rustc-link-arg-bins={}", resource.display());
    } else {
        let object = out.join("sean-toolbox-icon.o");
        let compiler_name = env::var_os("RC").unwrap_or_else(|| {
            // Native MinGW shells provide windres; cross builds normally use
            // a prefixed tool and can override it through RC.
            let host = env::var("HOST").unwrap_or_default();
            if host.contains("windows") {
                "windres".into()
            } else {
                let arch = target.split('-').next().expect("target architecture");
                format!("{arch}-w64-mingw32-windres").into()
            }
        });
        let mut compiler = Command::new(compiler_name);
        compiler.args([
            "--input-format=rc",
            "--output-format=coff",
            "--codepage=65001",
        ]);
        compiler
            .arg("--input")
            .arg(&script)
            .arg("--output")
            .arg(&object);
        compile(&mut compiler, "MinGW resource compiler (windres)");
        println!("cargo:rustc-link-arg-bins={}", object.display());
    }
}

fn compile(command: &mut Command, description: &str) {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("Cannot run {description}: {error}. Install the target Windows SDK/MinGW tools or set RC to the resource compiler path."));
    assert!(
        output.status.success(),
        "{description} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
