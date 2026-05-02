use serde::Serialize;
use tera::Context;

#[derive(Debug, Clone, Serialize)]
pub struct SystemInfo {
    pub os: String,
    pub arch: String,
    pub user: String,
    pub host: String,
}

impl SystemInfo {
    pub fn current() -> Self {
        Self {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            user: whoami::username(),
            host: whoami::fallible::hostname().unwrap_or_default(),
        }
    }
}

pub fn system_context() -> Context {
    let mut ctx = Context::new();
    ctx.insert("system", &SystemInfo::current());
    ctx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_context_populates_known_fields() {
        let ctx = system_context();
        let json = ctx.into_json();
        let system = json.get("system").expect("system namespace missing");
        assert!(system.get("os").and_then(|v| v.as_str()).is_some());
        assert!(system.get("arch").and_then(|v| v.as_str()).is_some());
        assert!(system.get("user").is_some());
        assert!(system.get("host").is_some());
    }

    #[test]
    fn os_value_matches_consts() {
        let info = SystemInfo::current();
        assert_eq!(info.os, std::env::consts::OS);
        assert_eq!(info.arch, std::env::consts::ARCH);
    }
}
