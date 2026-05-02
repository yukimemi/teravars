//! Multi-file config merge — yui-style deep recursive merge with per-file
//! Tera rendering and an `include = [...]` directive that lets a config file
//! pull in other files before it is processed.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tera::Context;
use toml::{Table, Value};

use crate::Result;
use crate::engine::Engine;
use crate::error::Error;
use crate::vars::{extract_vars, resolve, scan_tera_tags};

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
    let mut visited: HashSet<PathBuf> = HashSet::new();

    for p in paths {
        load_file_recursive(
            p.as_ref(),
            engine,
            extra_ctx,
            &mut acc_vars,
            &mut acc_config,
            &mut visited,
        )?;
    }

    if !acc_vars.is_empty() {
        resolve(&mut acc_vars, engine)?;
    }

    Ok(MergedConfig {
        vars: acc_vars,
        config: acc_config,
    })
}

fn load_file_recursive(
    path: &Path,
    engine: &mut Engine,
    extra_ctx: &Context,
    acc_vars: &mut Table,
    acc_config: &mut Table,
    visited: &mut HashSet<PathBuf>,
) -> Result<()> {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !visited.insert(canonical.clone()) {
        return Err(Error::IncludeCycle { path: canonical });
    }

    let raw = std::fs::read_to_string(path).map_err(|e| {
        Error::Io(std::io::Error::new(
            e.kind(),
            format!("{}: {e}", path.display()),
        ))
    })?;

    let include_paths = extract_include_paths(&raw, path)?;
    for raw_inc in &include_paths {
        let rendered = engine
            .render(raw_inc, extra_ctx)
            .map_err(|e| Error::Render(format!("{} include: {}", path.display(), error_msg(&e))))?;
        let inc_path = resolve_relative(path, Path::new(&rendered));
        load_file_recursive(&inc_path, engine, extra_ctx, acc_vars, acc_config, visited)?;
    }

    let file_vars = extract_vars(&raw)?;

    let mut resolution_vars = acc_vars.clone();
    deep_merge(&mut resolution_vars, file_vars);
    let _ = resolve(&mut resolution_vars, engine);

    let mut ctx = extra_ctx.clone();
    ctx.insert("vars", &resolution_vars);

    let rendered = engine
        .render(&raw, &ctx)
        .map_err(|e| Error::Render(format!("{}: {}", path.display(), error_msg(&e))))?;

    let mut parsed: Table = rendered
        .parse()
        .map_err(|e: toml::de::Error| Error::Extract(format!("{}: {e}", path.display())))?;

    parsed.remove("include");
    parsed.remove("teravars");

    if let Some(Value::Table(rendered_vars)) = parsed.get("vars").cloned() {
        deep_merge(acc_vars, rendered_vars);
    }

    deep_merge(acc_config, parsed);

    Ok(())
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

fn resolve_relative(base_file: &Path, target: &Path) -> PathBuf {
    if target.is_absolute() {
        target.to_path_buf()
    } else if let Some(parent) = base_file.parent() {
        parent.join(target)
    } else {
        target.to_path_buf()
    }
}

/// Extract the include directive paths from a raw TOML+Tera file.
///
/// Looks at the file's TOML skeleton (Tera control blocks `{% ... %}` removed
/// to make it parse-friendly) and reads either:
/// - root-level `include = [...]`, or
/// - `[teravars] include = [...]` (namespaced fallback)
///
/// If both forms are present, returns `Error::IncludeConflict`.
fn extract_include_paths(text: &str, path: &Path) -> Result<Vec<String>> {
    let skeleton = strip_tera_blocks(text);
    if skeleton.trim().is_empty() {
        return Ok(Vec::new());
    }
    let parsed: Table = match skeleton.parse() {
        Ok(t) => t,
        Err(_) => return Ok(Vec::new()),
    };

    let root_inc = read_string_array(parsed.get("include"));
    let teravars_inc = parsed
        .get("teravars")
        .and_then(|v| v.as_table())
        .and_then(|t| read_string_array(t.get("include")).into());

    match (
        root_inc.is_empty(),
        teravars_inc.as_ref().is_none_or(|v| v.is_empty()),
    ) {
        (false, false) => Err(Error::IncludeConflict {
            path: path.to_path_buf(),
        }),
        (false, _) => Ok(root_inc),
        (_, false) => Ok(teravars_inc.unwrap()),
        _ => Ok(Vec::new()),
    }
}

fn read_string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn strip_tera_blocks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut block_depth: usize = 0;
    let mut multiline_open = false;

    for line in text.lines() {
        let trimmed = line.trim();

        if multiline_open {
            if trimmed.contains("%}") {
                multiline_open = false;
            }
            out.push('\n');
            continue;
        }

        let scan = scan_tera_tags(trimmed);
        if scan.unterminated {
            multiline_open = true;
            out.push('\n');
            continue;
        }

        let starting_depth = block_depth;
        block_depth = block_depth
            .saturating_add(scan.opens)
            .saturating_sub(scan.closes);

        if scan.has_any_tag || starting_depth > 0 {
            out.push('\n');
            continue;
        }

        out.push_str(line);
        out.push('\n');
    }

    out
}

fn error_msg(err: &Error) -> String {
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

        assert!(!matches_config_pattern("config..toml"));
        assert!(!matches_config_pattern("other.toml"));
        assert!(!matches_config_pattern("config.toml.bak"));
    }

    #[test]
    fn file_rank_orders_correctly() {
        assert!(file_rank("config.toml") < file_rank("config.linux.toml"));
        assert!(file_rank("config.linux.toml") < file_rank("config.local.toml"));
    }

    #[test]
    fn extract_include_paths_root_form() {
        let text = r#"
include = ["a.toml", "b.toml"]

[vars]
foo = "bar"
"#;
        let paths = extract_include_paths(text, Path::new("dummy")).unwrap();
        assert_eq!(paths, vec!["a.toml", "b.toml"]);
    }

    #[test]
    fn extract_include_paths_teravars_form() {
        let text = r#"
[teravars]
include = ["a.toml"]

[vars]
foo = "bar"
"#;
        let paths = extract_include_paths(text, Path::new("dummy")).unwrap();
        assert_eq!(paths, vec!["a.toml"]);
    }

    #[test]
    fn extract_include_paths_conflict() {
        let text = r#"
include = ["a.toml"]

[teravars]
include = ["b.toml"]
"#;
        let err = extract_include_paths(text, Path::new("conflict.toml")).unwrap_err();
        assert!(matches!(err, Error::IncludeConflict { .. }));
    }

    #[test]
    fn extract_include_paths_skips_tera_control_blocks() {
        let text = r#"
{% if true %}
include = ["should-not-appear.toml"]
{% endif %}

[vars]
foo = "bar"
"#;
        let paths = extract_include_paths(text, Path::new("dummy")).unwrap();
        assert!(paths.is_empty());
    }

    #[test]
    fn extract_include_paths_none_when_absent() {
        let text = r#"[vars]
foo = "bar"
"#;
        let paths = extract_include_paths(text, Path::new("dummy")).unwrap();
        assert!(paths.is_empty());
    }
}
