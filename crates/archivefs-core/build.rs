use std::{env, path::PathBuf, process::Command};
fn main() {
    let source = "src/sram_provenance/observer.c";
    println!("cargo:rerun-if-changed={source}");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("sram-observer.so");
    let target = env::var("TARGET").unwrap();
    if target == "x86_64-unknown-linux-gnu" && env::var("HOST").unwrap() == target {
        let status = Command::new(env::var_os("CC").unwrap_or_else(|| "cc".into()))
            .args([
                "-std=c11",
                "-shared",
                "-fPIC",
                "-fcf-protection=branch",
                "-O2",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-Wno-unused-parameter",
                "-Wl,-z,relro,-z,now",
                "-o",
            ])
            .arg(&out)
            .arg(source)
            .args(["-ldl", "-pthread"])
            .status()
            .expect("native SRAM observer C compiler");
        assert!(status.success(), "native SRAM observer compilation failed");
    } else {
        std::fs::write(out, []).unwrap();
    }
}
