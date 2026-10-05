use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=native/gpu_copy.c");
    println!("cargo:rerun-if-env-changed=CC");
    println!("cargo:rerun-if-env-changed=AR");
    if std::env::var_os("CARGO_FEATURE_GPU_COPY").is_none() {
        return;
    }
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let object = out.join("gpu_copy.o");
    let compiler = std::env::var_os("CC").unwrap_or_else(|| "cc".into());
    assert!(
        Command::new(compiler)
            .args([
                "-std=c11",
                "-O2",
                "-fPIC",
                "-fvisibility=hidden",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-c",
                "native/gpu_copy.c",
                "-o"
            ])
            .arg(&object)
            .status()
            .unwrap()
            .success(),
        "GPU transfer compilation failed"
    );
    let archive = out.join("libqcom_gpu_copy.a");
    let archiver = std::env::var_os("AR").unwrap_or_else(|| "ar".into());
    assert!(
        Command::new(archiver)
            .arg("crs")
            .arg(&archive)
            .arg(&object)
            .status()
            .unwrap()
            .success(),
        "GPU transfer archive failed"
    );
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=qcom_gpu_copy");
    println!("cargo:rustc-link-lib=dl");
}
