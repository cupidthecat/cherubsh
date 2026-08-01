fn main() {
    for (source, destination) in [
        ("TARGET", "CHERUBSH_TARGET"),
        ("CARGO_CFG_TARGET_ARCH", "CHERUBSH_TARGET_ARCH"),
        ("CARGO_CFG_TARGET_OS", "CHERUBSH_TARGET_OS"),
        ("CARGO_CFG_TARGET_ENV", "CHERUBSH_TARGET_ENV"),
    ] {
        let value = std::env::var(source).unwrap_or_else(|_| panic!("Cargo did not set {source}"));
        println!("cargo:rustc-env={destination}={value}");
        println!("cargo:rerun-if-env-changed={source}");
    }
}
