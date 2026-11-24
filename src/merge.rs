//! Multi-file config merge — yui-style deep recursive merge with per-file
//! Tera rendering and an `include = [...]` directive that lets a config file
//! pull in other files before it is processed.

mod include;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tera::Context;
use toml::{Table, Value};

use crate::Result;
use crate::comments::strip_toml_comments;
use crate::engine::Engine;
use crate::error::Error;
use crate::vars::{extract_vars, resolve_in_context};
use include::extract_include_paths;

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
        resolve_in_context(&mut acc_vars, engine, extra_ctx)?;
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

    // Comment text never reaches Tera: a commented-out sample line or a note
    // quoting teravars syntax would otherwise render (and typically fail the
    // whole load). Done once here so include detection, [vars] extraction
    // and the render below all see the same comment-free text; line numbers
    // are preserved, so error locations still line up with the file on disk.
    let raw = strip_toml_comments(&raw);

    let include_paths = extract_include_paths(&raw, path)?;
    for raw_inc in &include_paths.paths {
        let rendered = engine
            .render(raw_inc, extra_ctx)
            .map_err(|e| Error::Render(format!("{} include: {}", path.display(), error_msg(&e))))?;
        let inc_path = resolve_relative(path, Path::new(&rendered));
        load_file_recursive(&inc_path, engine, extra_ctx, acc_vars, acc_config, visited)?;
    }

    let file_vars = extract_vars(&raw)?;

    let mut resolution_vars = acc_vars.clone();
    deep_merge(&mut resolution_vars, file_vars);
    // Best-effort per-file resolution: a [vars] entry in this file may
    // legitimately reference vars introduced by a later file in the merge
    // chain, so we tolerate `ResolveNotConverged` / render errors here.
    // The authoritative pass runs at the end of `load_merged` once every
    // file has contributed; genuine render failures resurface from the
    // `engine.render` call below with a more useful location.
    let _ = resolve_in_context(&mut resolution_vars, engine, extra_ctx);

    let mut ctx = extra_ctx.clone();
    ctx.insert("vars", &resolution_vars);

    let rendered = engine
        .render(&raw, &ctx)
        .map_err(|e| include_paths.render_error(path, error_msg(&e)))?;

    let mut parsed: Table = rendered
        .parse()
        .map_err(|e: toml::de::Error| Error::Extract(format!("{}: {e}", path.display())))?;

    include_paths.check_rendered(&parsed, path)?;
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

fn error_msg(err: &Error) -> String {
    match err {
        Error::Render(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract_include_paths(text: &str, path: &Path) -> Result<Vec<String>> {
        include::extract_include_paths(text, path).map(|includes| includes.paths)
    }

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
include = []

[teravars]
include = ["b.toml"]
"#;
        let err = extract_include_paths(text, Path::new("conflict.toml")).unwrap_err();
        assert!(matches!(err, Error::IncludeConflict { .. }));
    }

    #[test]
    fn extract_include_paths_skips_tera_control_blocks() {
        let text = r#"
{%- if ("%}" ==
       "%}") -%}
include = ["should-not-appear.toml"]
{%- if true %}
include = ["nested.toml"]
{% endif -%}
{%- endif %}
{% if true %} include = ["same-line.toml"] {% endif %}
{% set captured | replace(from="a", to="b") %}
[captured]
include = ["captured.toml"]
{% endset %}
{% raw %}
{% if true %}
include = ["literal-template.toml"]
{% endraw %}

values = {% if true %}[1]{% endif %}
include = ["actual.toml"]
"#;
        let paths = extract_include_paths(text, Path::new("dummy")).unwrap();
        assert_eq!(paths, ["actual.toml"]);
    }

    #[test]
    fn extract_include_paths_none_when_absent() {
        let text = r#"[vars]
foo = "bar"
"#;
        let paths = extract_include_paths(text, Path::new("dummy")).unwrap();
        assert!(paths.is_empty());
    }

    #[test]
    fn extract_include_paths_ignores_non_directives() {
        let text = r###"
# include = ["comment.toml"]
{#
include = ["tera-comment.toml"]
#}
example = '''
include = ["literal-string.toml"]
''''
example_basic = """
[teravars]
include = ["basic-string.toml"]
""""
example_expr = {{ `}}
include = ["tera-expression.toml"]
` }}
entries = [{
    include = ["inline-member.toml"]
}]
"teravars.include" = ["quoted-single-key.toml"]
teravars = { nested = { include = ["nested-member.toml"] } }

[app]
include = ["table-local.toml"]
teravars.include = ["table-local-namespace.toml"]

[[teravars]]
include = ["array-table.toml"]
"###;
        let paths = extract_include_paths(text, Path::new("dummy")).unwrap();
        assert!(paths.is_empty());
    }

    #[test]
    fn extract_include_paths_ignores_keys_under_nested_or_dynamic_headers() {
        for header in ["[teravars.sub]", "[{{ vars.dynamic_table }}]"] {
            let text = format!("{header}\ninclude = [\"table-local.toml\"]");
            let paths = extract_include_paths(&text, Path::new("dummy")).unwrap();
            assert!(
                paths.is_empty(),
                "unexpected includes under {header}: {paths:?}"
            );
        }
    }

    #[test]
    fn extract_include_paths_supports_quoted_dotted_and_inline_forms() {
        for text in [
            r#""includ\u0065" = ["a.toml", 'b.toml']"#,
            r#""teravars" . 'include' = ["a.toml", 'b.toml']"#,
            r#"teravars = { other = {{ unrelated }}, "include" = ["a.toml", 'b.toml'] }"#,
            "teravars = {\ninclude = [\"a.toml\", 'b.toml'] # note\n}\n",
            "[\"teravars\"]\r\n'include' = [\r\n\"a.toml\", # note\r\n'b.toml',\r\n]\r\n",
        ] {
            let paths = extract_include_paths(text, Path::new("dummy")).unwrap();
            assert_eq!(paths, ["a.toml", "b.toml"], "source: {text}");
        }
    }

    #[test]
    fn extract_include_paths_rejects_invalid_directives() {
        for (text, directive) in [
            ("include = ", "include"),
            ("include = [\"a.toml\"", "include"),
            ("include = \"a.toml\"", "include"),
            ("[teravars]\ninclude = [\"a.toml\", 1]", "teravars.include"),
            ("teravars.include = {{ paths }}", "teravars.include"),
            ("teravars = { include = [false] }", "teravars.include"),
        ] {
            let error = extract_include_paths(text, Path::new("broken.toml")).unwrap_err();
            let Error::Extract(message) = error else {
                panic!("unexpected error: {error}");
            };
            assert!(
                message.starts_with(&format!("broken.toml `{directive}`: ")),
                "{message}"
            );
        }
    }

    #[test]
    fn extract_include_paths_rejects_duplicate_declarations() {
        for (text, directive) in [
            ("include = []\n'include' = [\"b.toml\"]", "include"),
            (
                "teravars.include = []\n[teravars]\ninclude = [\"b.toml\"]",
                "teravars.include",
            ),
        ] {
            let error = extract_include_paths(text, Path::new("duplicate.toml")).unwrap_err();
            let Error::Extract(message) = error else {
                panic!("unexpected error: {error}");
            };
            assert_eq!(
                message,
                format!("duplicate.toml `{directive}`: duplicate declaration")
            );
        }
    }
}
