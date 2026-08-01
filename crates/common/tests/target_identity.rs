use cherubsh_common::target::TargetIdentity;

#[test]
fn gnu_linux_targets_use_bash_compatible_machine_names() {
    let x86 = TargetIdentity::from_parts("x86_64-unknown-linux-gnu", "x86_64", "linux", "gnu");
    assert_eq!(x86.hosttype, "x86_64");
    assert_eq!(x86.ostype, "linux-gnu");
    assert_eq!(x86.machtype, "x86_64-pc-linux-gnu");
    assert_eq!(x86.rust_target, "x86_64-unknown-linux-gnu");

    let arm = TargetIdentity::from_parts("aarch64-unknown-linux-gnu", "aarch64", "linux", "gnu");
    assert_eq!(arm.hosttype, "aarch64");
    assert_eq!(arm.ostype, "linux-gnu");
    assert_eq!(arm.machtype, "aarch64-unknown-linux-gnu");
    assert_eq!(arm.rust_target, "aarch64-unknown-linux-gnu");
}

#[test]
fn current_identity_comes_from_the_compiler_target() {
    let identity = TargetIdentity::current();

    assert_eq!(identity.hosttype, std::env::consts::ARCH);
    assert_eq!(identity.ostype, "linux-gnu");
    assert!(identity.rust_target.starts_with(std::env::consts::ARCH));
    assert!(identity.machtype.starts_with(std::env::consts::ARCH));
}
