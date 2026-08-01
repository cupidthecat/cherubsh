#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetIdentity {
    pub rust_target: String,
    pub hosttype: String,
    pub ostype: String,
    pub machtype: String,
}

impl TargetIdentity {
    pub fn current() -> Self {
        Self::from_parts(
            env!("CHERUBSH_TARGET"),
            env!("CHERUBSH_TARGET_ARCH"),
            env!("CHERUBSH_TARGET_OS"),
            env!("CHERUBSH_TARGET_ENV"),
        )
    }

    pub fn from_parts(rust_target: &str, arch: &str, os: &str, target_env: &str) -> Self {
        let ostype = match (os, target_env) {
            ("linux", "gnu") => String::from("linux-gnu"),
            ("linux", "") => String::from("linux"),
            ("linux", environment) => format!("linux-{environment}"),
            ("macos", _) => String::from("darwin"),
            ("windows", _) => String::from("msys"),
            (other, _) => other.to_string(),
        };
        let machtype = if rust_target == "x86_64-unknown-linux-gnu" {
            String::from("x86_64-pc-linux-gnu")
        } else {
            rust_target.to_string()
        };

        Self {
            rust_target: rust_target.to_string(),
            hosttype: arch.to_string(),
            ostype,
            machtype,
        }
    }
}
