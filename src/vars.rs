use tera::Context;
use toml::{Table, Value};

use crate::Engine;
use crate::error::Error;
use crate::Result;

pub const DEFAULT_MAX_RESOLVE_ITERATIONS: usize = 10;

pub fn extract_vars(text: &str) -> Result<Table> {
    let raw = extract_vars_section(text);
    if raw.trim().is_empty() {
        return Ok(Table::new());
    }
    let parsed: Table = raw.parse().map_err(|e: toml::de::Error| Error::Extract(e.to_string()))?;
    Ok(match parsed.get("vars") {
        Some(Value::Table(t)) => t.clone(),
        _ => Table::new(),
    })
}

fn extract_vars_section(text: &str) -> String {
    let mut out = String::new();
    let mut in_vars = false;
    let mut block_depth: usize = 0;
    let mut multiline_tag_open = false;

    for line in text.lines() {
        let trimmed = line.trim();

        if multiline_tag_open {
            if trimmed.contains("%}") {
                multiline_tag_open = false;
            }
            continue;
        }

        let scan = scan_tera_tags(trimmed);
        if scan.unterminated {
            multiline_tag_open = true;
            continue;
        }

        let starting_depth = block_depth;
        block_depth = block_depth
            .saturating_add(scan.opens)
            .saturating_sub(scan.closes);

        if scan.has_any_tag || starting_depth > 0 {
            continue;
        }

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if trimmed == "[vars]" || trimmed.starts_with("[vars.") {
                in_vars = true;
                out.push_str(line);
                out.push('\n');
            } else {
                in_vars = false;
            }
            continue;
        }

        if in_vars {
            out.push_str(line);
            out.push('\n');
        }
    }

    out
}

struct TagScan {
    opens: usize,
    closes: usize,
    has_any_tag: bool,
    unterminated: bool,
}

fn scan_tera_tags(line: &str) -> TagScan {
    const OPENERS: &[&str] = &["if", "for", "block", "macro", "filter", "raw"];
    const CLOSERS: &[&str] = &[
        "endif", "endfor", "endblock", "endmacro", "endfilter", "endraw",
    ];

    let mut opens = 0;
    let mut closes = 0;
    let mut has_any_tag = false;
    let mut unterminated = false;
    let mut s = line;

    while let Some(idx) = s.find("{%") {
        has_any_tag = true;
        let after = &s[idx + 2..];
        match after.find("%}") {
            Some(end) => {
                let body = after[..end].trim();
                let first = body.split_whitespace().next().unwrap_or("");
                if OPENERS.contains(&first) {
                    opens += 1;
                } else if CLOSERS.contains(&first) {
                    closes += 1;
                }
                s = &after[end + 2..];
            }
            None => {
                unterminated = true;
                break;
            }
        }
    }

    TagScan {
        opens,
        closes,
        has_any_tag,
        unterminated,
    }
}

pub fn resolve(vars: &mut Table, engine: &mut Engine) -> Result<()> {
    resolve_with_max_iter(vars, engine, DEFAULT_MAX_RESOLVE_ITERATIONS)
}

pub fn resolve_with_max_iter(
    vars: &mut Table,
    engine: &mut Engine,
    max_iter: usize,
) -> Result<()> {
    if vars.is_empty() {
        return Ok(());
    }

    for _ in 0..max_iter {
        let mut ctx = Context::new();
        ctx.insert("vars", &*vars);

        let mut next = Table::new();
        for (k, v) in vars.iter() {
            next.insert(k.clone(), render_value(v, engine, &ctx)?);
        }

        let converged = next == *vars;
        *vars = next;
        if converged {
            return Ok(());
        }
    }

    Err(Error::ResolveNotConverged {
        iterations: max_iter,
    })
}

pub fn expand_value(value: &mut Value, engine: &mut Engine, ctx: &Context) -> Result<()> {
    match value {
        Value::String(s) => {
            let rendered = engine.render(s, ctx)?;
            *s = rendered;
        }
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                expand_value(item, engine, ctx)?;
            }
        }
        Value::Table(t) => {
            for (_, v) in t.iter_mut() {
                expand_value(v, engine, ctx)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn render_value(v: &Value, engine: &mut Engine, ctx: &Context) -> Result<Value> {
    match v {
        Value::String(s) => Ok(Value::String(engine.render(s, ctx)?)),
        Value::Array(arr) => {
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                out.push(render_value(item, engine, ctx)?);
            }
            Ok(Value::Array(out))
        }
        Value::Table(t) => {
            let mut out = Table::new();
            for (k, vv) in t {
                out.insert(k.clone(), render_value(vv, engine, ctx)?);
            }
            Ok(Value::Table(out))
        }
        other => Ok(other.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_vars_basic() {
        let toml_text = r#"
[other]
foo = "bar"

[vars]
a = "hello"
b = "world"

[server]
port = 8080
"#;
        let vars = extract_vars(toml_text).unwrap();
        assert_eq!(vars.get("a").and_then(|v| v.as_str()), Some("hello"));
        assert_eq!(vars.get("b").and_then(|v| v.as_str()), Some("world"));
        assert_eq!(vars.len(), 2);
    }

    #[test]
    fn extract_vars_with_subsection() {
        let toml_text = r#"
[vars]
a = "1"

[vars.nested]
b = "2"

[other]
c = "3"
"#;
        let vars = extract_vars(toml_text).unwrap();
        assert_eq!(vars.get("a").and_then(|v| v.as_str()), Some("1"));
        let nested = vars.get("nested").and_then(|v| v.as_table()).unwrap();
        assert_eq!(nested.get("b").and_then(|v| v.as_str()), Some("2"));
    }

    #[test]
    fn extract_vars_skips_tera_blocks() {
        let toml_text = r#"
{% set name = "world" %}

[vars]
greeting = "hello"

{% if true %}
ignored = "should not appear"
{% endif %}

[other]
foo = "bar"
"#;
        let vars = extract_vars(toml_text).unwrap();
        assert_eq!(vars.get("greeting").and_then(|v| v.as_str()), Some("hello"));
        assert!(vars.get("ignored").is_none());
    }

    #[test]
    fn extract_vars_empty_when_no_vars_section() {
        let vars = extract_vars("[other]\nfoo = \"bar\"\n").unwrap();
        assert!(vars.is_empty());
    }

    #[test]
    fn resolve_simple_cross_reference() {
        let mut vars = toml::from_str::<Table>(
            r#"
a = "hello"
b = "{{ vars.a }} world"
"#,
        )
        .unwrap();
        let mut engine = Engine::new_minimal();
        resolve(&mut vars, &mut engine).unwrap();
        assert_eq!(vars.get("a").and_then(|v| v.as_str()), Some("hello"));
        assert_eq!(vars.get("b").and_then(|v| v.as_str()), Some("hello world"));
    }

    #[test]
    fn resolve_chained_cross_reference() {
        let mut vars = toml::from_str::<Table>(
            r#"
a = "1"
b = "{{ vars.a }}-2"
c = "{{ vars.b }}-3"
"#,
        )
        .unwrap();
        let mut engine = Engine::new_minimal();
        resolve(&mut vars, &mut engine).unwrap();
        assert_eq!(vars.get("c").and_then(|v| v.as_str()), Some("1-2-3"));
    }

    #[test]
    fn resolve_returns_err_but_keeps_partial_state_on_non_convergence() {
        // Self-referential expansion that grows on every iteration; never reaches a fixed point.
        let mut vars = toml::from_str::<Table>(
            r#"
a = "x{{ vars.a }}"
"#,
        )
        .unwrap();
        let mut engine = Engine::new_minimal();
        let result = resolve_with_max_iter(&mut vars, &mut engine, 3);
        assert!(
            matches!(result, Err(Error::ResolveNotConverged { iterations: 3 })),
            "expected ResolveNotConverged, got: {result:?}"
        );
        let a = vars.get("a").and_then(|v| v.as_str()).unwrap();
        assert!(a.contains("{{ vars.a }}") || a.contains("{{vars.a}}"));
        assert!(a.starts_with('x'));
    }

    #[test]
    fn resolve_handles_nested_tables() {
        let mut vars = toml::from_str::<Table>(
            r#"
host = "example.com"
[server]
url = "https://{{ vars.host }}/api"
"#,
        )
        .unwrap();
        let mut engine = Engine::new_minimal();
        resolve(&mut vars, &mut engine).unwrap();
        let server = vars.get("server").and_then(|v| v.as_table()).unwrap();
        assert_eq!(
            server.get("url").and_then(|v| v.as_str()),
            Some("https://example.com/api")
        );
    }

    #[test]
    fn expand_value_walks_strings_in_tree() {
        let mut value: Value = toml::from_str(
            r#"
greeting = "hello {{ name }}"
[server]
url = "https://{{ host }}"
ports = ["{{ port }}", 8080]
"#,
        )
        .unwrap();
        let mut engine = Engine::new_minimal();
        let mut ctx = Context::new();
        ctx.insert("name", "world");
        ctx.insert("host", "example.com");
        ctx.insert("port", "8080");

        expand_value(&mut value, &mut engine, &ctx).unwrap();

        assert_eq!(
            value.get("greeting").and_then(|v| v.as_str()),
            Some("hello world")
        );
        let server = value.get("server").and_then(|v| v.as_table()).unwrap();
        assert_eq!(
            server.get("url").and_then(|v| v.as_str()),
            Some("https://example.com")
        );
        let ports = server.get("ports").and_then(|v| v.as_array()).unwrap();
        assert_eq!(ports[0].as_str(), Some("8080"));
        assert_eq!(ports[1].as_integer(), Some(8080));
    }
}
