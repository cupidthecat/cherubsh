fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
        let object = out_dir.join("loadable_abi.o");
        let archive = out_dir.join("libcherub_loadable_abi.a");
        let source = std::path::Path::new("loadable_abi.c");
        let compiler = std::env::var_os("CC").unwrap_or_else(|| "cc".into());
        let status = std::process::Command::new(compiler)
            .args(["-std=c11", "-fPIC", "-O2", "-c"])
            .arg(source)
            .arg("-o")
            .arg(&object)
            .status()
            .expect("run C compiler for the Bash loadable ABI bridge");
        assert!(status.success(), "failed to compile loadable_abi.c");
        let archiver = std::env::var_os("AR").unwrap_or_else(|| "ar".into());
        let status = std::process::Command::new(archiver)
            .arg("crus")
            .arg(&archive)
            .arg(&object)
            .status()
            .expect("run archiver for the Bash loadable ABI bridge");
        assert!(status.success(), "failed to archive loadable_abi.c");
        println!("cargo:rerun-if-changed={}", source.display());
        println!("cargo:rustc-link-search=native={}", out_dir.display());
        println!("cargo:rustc-link-lib=static=cherub_loadable_abi");
        println!("cargo:rustc-link-arg=-Wl,--export-dynamic");
    }
}
