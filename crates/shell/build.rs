fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
        let object = out_dir.join("loadable_abi.o");
        let source = std::path::Path::new("loadable_abi.c");
        let compiler = std::env::var_os("CC").unwrap_or_else(|| "cc".into());
        let mut command = std::process::Command::new(compiler);
        command.args(["-std=c11", "-fPIC", "-O2", "-c"]);
        if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64") {
            command.arg("-fno-stack-protector");
        }
        match std::env::var("CHERUBSH_C_SANITIZER").as_deref() {
            Ok("address") => {
                command.args(["-fsanitize=address", "-fno-omit-frame-pointer"]);
            }
            Ok(other) => panic!("unsupported CHERUBSH_C_SANITIZER={other}"),
            Err(std::env::VarError::NotPresent) => {}
            Err(error) => panic!("read CHERUBSH_C_SANITIZER: {error}"),
        }
        let status = command
            .arg(source)
            .arg("-o")
            .arg(&object)
            .status()
            .expect("run C compiler for the Bash loadable ABI bridge");
        assert!(status.success(), "failed to compile loadable_abi.c");
        println!("cargo:rerun-if-changed={}", source.display());
        println!("cargo:rerun-if-env-changed=CHERUBSH_C_SANITIZER");
        println!("cargo:rustc-link-arg-bin=cherubsh={}", object.display());
        println!("cargo:rustc-link-arg-bin=cherubsh=-Wl,--export-dynamic");
    }
}
