use std::fs;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn parity_workflow_runs_the_full_gate_on_native_x64_and_arm64() {
    let workflow = fs::read_to_string(workspace_root().join(".github/workflows/ci.yml"))
        .expect("read parity workflow");

    assert!(workflow.contains("runs-on: ${{ matrix.runner }}"));
    assert!(workflow.contains("runner: ubuntu-24.04\n"));
    assert!(workflow.contains("runner: ubuntu-24.04-arm\n"));
    assert_eq!(workflow.matches("run: ./tools/run-parity.sh").count(), 1);
    assert!(!workflow.contains("continue-on-error"));
    assert!(workflow.contains("  linux:\n    name: linux\n    needs: parity\n"));
}

#[test]
fn release_workflow_builds_and_publishes_both_native_archives() {
    let workflow = fs::read_to_string(workspace_root().join(".github/workflows/release.yml"))
        .expect("read release workflow");

    assert!(workflow.contains("runs-on: ${{ matrix.runner }}"));
    assert!(workflow.contains("runner: ubuntu-24.04\n"));
    assert!(workflow.contains("runner: ubuntu-24.04-arm\n"));
    assert!(workflow.contains("uses: actions/upload-artifact@v7"));
    assert!(workflow.contains("uses: actions/download-artifact@v8"));
    assert!(workflow.contains("needs: build-linux-archives"));
    assert!(workflow.contains("sha256sum --check SHA256SUMS"));
    assert!(workflow.contains("run: ./tools/build-readline.sh"));
    assert!(workflow.contains("./tools/package-readline-dev.sh"));
    assert!(workflow.contains("sha256sum cherubsh-*.tar.gz > SHA256SUMS"));
    assert!(workflow.contains("dist/cherubsh-readline-dev-*.tar.gz"));
}

#[test]
fn loadable_bridge_is_linked_directly_into_the_shell_binary() {
    let build_script = fs::read_to_string(workspace_root().join("crates/shell/build.rs"))
        .expect("read shell build script");

    assert!(build_script.contains("cargo:rustc-link-arg-bin=cherubsh={}"));
    assert!(!build_script.contains("cargo:rustc-link-lib=static=cherub_loadable_abi"));
    assert!(build_script.contains("-fno-stack-protector"));
    assert!(build_script
        .contains("std::env::var(\"CARGO_CFG_TARGET_ARCH\").as_deref() == Ok(\"aarch64\")"));
}
