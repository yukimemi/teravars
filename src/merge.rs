//! Multi-file config merge — yui-style deep recursive merge with per-file
//! Tera rendering, so that vars accumulated from earlier files are visible
//! during the current file's render pass.

use std::path::{Path, PathBuf};

use tera::Context;
use toml::{Table, Value};

use crate::Result;
use crate::engine::Engine;
use crate::error::Error;
use crate::vars::{extract_vars, resolve};

#[derive(Debug, Clone, Default)]
pub struct MergedConfig {
    pub vars: Table,
    pub config: Table,
}

pub fn load_merged<P: AsRef<Path>>(
    paths: impl IntoIterator<Item = P>,
    engine: &mut Engine,
    extra_ctx: &Context,
) -> Result<MergedConfig> {
    let mut acc_vars = Table::new();
    let mut acc_config = Table::new();

    for p in paths {
        let path = p.as_ref();
        let raw = std::fs::read_to_string(path).map_err(|e| {
            Error::Io(std::io::Error::new(
                e.kind(),
                format!("{}: {e}", path.display()),
            ))
        })?;

        let file_vars = extract_vars(&raw)?;

        let mut resolution_vars = acc_vars.clone();
        deep_merge(&mut resolution_vars, file_vars);
        let _ = resolve(&mut resolution_vars, engine);

        let mut ctx = extra_ctx.clone();
        ctx.insert("vars", &resolution_vars);

        let rendered = engine
            .render(&raw, &ctx)
            .map_err(|e| Error::Render(format!("{}: {}", path.display(), error_inner(&e))))?;

        let parsed: Table = rendered
            .parse()
            .map_err(|e: toml::de::Error| Error::Extract(format!("{}: {e}", path.display())))?;

        if let Some(Value::Table(rendered_vars)) = parsed.get("vars").cloned() {
            deep_merge(&mut acc_vars, rendered_vars);
        }

        deep_merge(&mut acc_config, parsed);
    }

    if !acc_vars.is_empty() {
        resolve(&mut acc_vars, engine)?;
    }

    Ok(MergedConfig {
        vars: acc_vars,
        config: acc_config,
    })
}

pub fn discover_config_files(dir: impl AsRef<Path>) -> Result<Vec<PathBuf>> {
    let dir = dir.as_ref();
    let mut files = Vec::new();

    for entry in std::fs::read_dir(dir).map_err(|e| {
        Error::Io(std::io::Error::new(
            e.kind(),
            format!("{}: {e}", dir.display()),
        ))
    })? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if matches_config_pattern(name) {
            files.push(path);
        }
    }

    files.sort_by(|a, b| {
        let na = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let nb = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
        file_rank(na).cmp(&file_rank(nb)).then_with(|| na.cmp(nb))
    });

    Ok(files)
}

fn matches_config_pattern(name: &str) -> bool {
    name == "config.toml"
        || (name.starts_with("config.")
            && name.ends_with(".toml")
            && name.len() > "config..toml".len())
}

fn file_rank(name: &str) -> u8 {
    match name {
        "config.toml" => 0,
        "config.local.toml" => 2,
        _ => 1,
    }
}

pub fn deep_merge(base: &mut Table, overlay: Table) {
    for (k, v) in overlay {
        match (base.remove(&k), v) {
            (Some(Value::Table(mut b)), Value::Table(o)) => {
                deep_merge(&mut b, o);
                base.insert(k, Value::Table(b));
            }
            (Some(Value::Array(mut b)), Value::Array(o)) => {
                b.extend(o);
                base.insert(k, Value::Array(b));
            }
            (_, v) => {
                base.insert(k, v);
            }
        }
    }
}

fn error_inner(err: &Error) -> String {
    match err {
        Error::Render(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_merge_recursive_tables() {
        let mut base: Table = toml::from_str(
            r#"
[server]
host = "base"
port = 8080
[server.tls]
enabled = false
"#,
        )
        .unwrap();
        let overlay: Table = toml::from_str(
            r#"
[server]
host = "overlay"
[server.tls]
cert = "/etc/cert"
"#,
        )
        .unwrap();

        deep_merge(&mut base, overlay);

        let server = base.get("server").and_then(|v| v.as_table()).unwrap();
        assert_eq!(server.get("host").and_then(|v| v.as_str()), Some("overlay"));
        assert_eq!(server.get("port").and_then(|v| v.as_integer()), Some(8080));
        let tls = server.get("tls").and_then(|v| v.as_table()).unwrap();
        assert_eq!(tls.get("enabled").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(tls.get("cert").and_then(|v| v.as_str()), Some("/etc/cert"));
    }

    #[test]
    fn deep_merge_arrays_append() {
        let mut base: Table = toml::from_str(r#"items = [1, 2]"#).unwrap();
        let overlay: Table = toml::from_str(r#"items = [3, 4]"#).unwrap();
        deep_merge(&mut base, overlay);
        let items = base.get("items").and_then(|v| v.as_array()).unwrap();
        assert_eq!(items.len(), 4);
    }

    #[test]
    fn matches_config_pattern_examples() {
        assert!(matches_config_pattern("config.toml"));
        assert!(matches_config_pattern("config.local.toml"));
        assert!(matches_config_pattern("config.linux.toml"));
        assert!(matches_config_pattern("config.x86.toml"));

        assert!(!matches_config_pattern("config..toml"));
        assert!(!matches_config_pattern("other.toml"));
        assert!(!matches_config_pattern("config.toml.bak"));
        assert!(!matches_config_pattern("Config.toml"));
    }

    #[test]
    fn file_rank_orders_correctly() {
        assert!(file_rank("config.toml") < file_rank("config.linux.toml"));
        assert!(file_rank("config.linux.toml") < file_rank("config.local.toml"));
    }
}
