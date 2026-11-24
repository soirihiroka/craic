use std::path::PathBuf;
use std::process::Command;
use std::{env, fs};

fn main() {
    println!("cargo:rerun-if-changed=../../tools/build-ghostty-vt.sh");
    println!("cargo:rerun-if-changed=../../tools/namespace-ghostty-vt.sh");
    println!("cargo:rerun-if-env-changed=GHOSTTY_VT_DIR");
    println!("cargo:rerun-if-env-changed=ZIG");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let target = env::var("TARGET").unwrap();
    let native = if let Some(path) = env::var_os("GHOSTTY_VT_DIR") {
        PathBuf::from(path).canonicalize().expect("GHOSTTY_VT_DIR")
    } else {
        let native = out.join("ghostty-vt");
        let status = Command::new("sh")
            .arg("../../tools/build-ghostty-vt.sh")
            .arg(&native)
            .arg(&target)
            .status()
            .expect("launch libghostty-vt build");
        assert!(status.success(), "could not prepare libghostty-vt");
        native
    };
    let archive = native.join("lib/libghostty-vt.a");
    assert!(archive.is_file(), "missing {}", archive.display());
    let include = native.join("include");
    println!("cargo:rerun-if-changed={}", archive.display());
    println!("cargo:rerun-if-changed={}", include.display());
    let isolated = out.join("isolated");
    fs::create_dir_all(&isolated).expect("create Ghostty archive directory");
    let status = Command::new("sh")
        .arg("../../tools/namespace-ghostty-vt.sh")
        .arg(&archive)
        .arg(isolated.join("libghostty-vt.a"))
        .status()
        .expect("launch Ghostty symbol isolation");
    assert!(status.success(), "could not isolate libghostty-vt symbols");
    println!("cargo:rustc-link-search=native={}", isolated.display());
    println!("cargo:rustc-link-lib=static=ghostty-vt");
    bindgen::Builder::default()
        .header_contents("ghostty_wrapper.h", "#include <ghostty/vt.h>\n#if !GHOSTTY_VT_VERSION_AT_LEAST(1, 0, 1) || GHOSTTY_VT_VERSION_MAJOR != 1\n#error Craic requires libghostty-vt 1.0.1 or a compatible 1.x release\n#endif")
        .clang_arg(format!("-I{}", include.display()))
        .clang_arg("-DGHOSTTY_STATIC")
        .allowlist_function("ghostty_.*")
        .allowlist_type("Ghostty.*")
        .allowlist_var("GHOSTTY_.*")
        .prepend_enum_name(false)
        .derive_default(true)
        .layout_tests(false)
        .generate()
        .expect("generate libghostty-vt C bindings")
        .write_to_file(out.join("ghostty.rs"))
        .expect("write libghostty-vt C bindings");
}
