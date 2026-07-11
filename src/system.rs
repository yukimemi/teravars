use serde::Serialize;
use tera::Context;

#[derive(Debug, Clone, Serialize)]
pub struct SystemInfo {
    pub os: String,
    pub arch: String,
    pub user: String,
    pub host: String,
    /// Process working directory at the time `system_context()` was called.
    /// Useful for templating cwd-relative paths, e.g.
    /// `include = ["{{ system.cwd }}/extras.toml"]`.
    pub cwd: String,
}

impl SystemInfo {
    pub fn current() -> Self {
        Self {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            user: whoami::username().unwrap_or_default(),
            host: whoami::hostname().unwrap_or_default(),
            cwd: std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
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
        // tera 2.0 dropped `Context::into_json`, so verify the `system.*`
        // namespace by rendering each field through a one-off template.
        let ctx = system_context();

        // os / arch come from compile-time consts and must round-trip exactly.
        let os = tera::Tera::one_off("{{ system.os }}", &ctx, false).unwrap();
        assert_eq!(os, std::env::consts::OS);
        let arch = tera::Tera::one_off("{{ system.arch }}", &ctx, false).unwrap();
        assert_eq!(arch, std::env::consts::ARCH);

        // user / host / cwd must at least be present (renderable without error).
        for field in ["user", "host", "cwd"] {
            tera::Tera::one_off(&format!("{{{{ system.{field} }}}}"), &ctx, false)
                .unwrap_or_else(|e| panic!("system.{field} should render: {e}"));
        }
    }

    #[test]
    fn os_value_matches_consts() {
        let info = SystemInfo::current();
        assert_eq!(info.os, std::env::consts::OS);
        assert_eq!(info.arch, std::env::consts::ARCH);
    }
}
