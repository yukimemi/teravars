//! Integration tests for the `merge` feature: multi-file config loading
//! with per-file Tera rendering and deep recursive merge.

#![cfg(feature = "merge")]

use std::fs;

use tempfile::TempDir;
use teravars::{Context, Engine, Error, discover_config_files, load_merged, system_context};

fn write(dir: &TempDir, name: &str, contents: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    fs::write(&path, contents).unwrap();
    path
}

#[test]
fn three_file_merge_with_vars_cross_refs() {
    let dir = TempDir::new().unwrap();

    let base = write(
        &dir,
        "config.toml",
        r#"
[vars]
host = "base.example"
port = "8080"

[server]
url = "https://{{ vars.host }}:{{ vars.port }}"

[features]
items = ["a"]
"#,
    );

    let env_file = write(
        &dir,
        "config.env.toml",
        r#"
[vars]
port = "9090"

[features]
items = ["b"]
"#,
    );

    let local = write(
        &dir,
        "config.local.toml",
        r#"
[vars]
host = "local.example"

[features]
items = ["c"]

[server]
debug = true
"#,
    );

    let mut engine = Engine::new();
    let merged = load_merged([&base, &env_file, &local], &mut engine, &system_context()).unwrap();

    assert_eq!(
        merged.vars.get("host").and_then(|v| v.as_str()),
        Some("local.example"),
        "local file should win"
    );
    assert_eq!(
        merged.vars.get("port").and_then(|v| v.as_str()),
        Some("9090"),
        "env file should override base"
    );

    let server = merged
        .config
        .get("server")
        .and_then(|v| v.as_table())
        .unwrap();
    assert_eq!(
        server.get("url").and_then(|v| v.as_str()),
        Some("https://base.example:8080"),
        "url is rendered per-file with the vars seen at that point"
    );
    assert_eq!(server.get("debug").and_then(|v| v.as_bool()), Some(true));

    let items = merged
        .config
        .get("features")
        .and_then(|v| v.as_table())
        .and_then(|t| t.get("items"))
        .and_then(|v| v.as_array())
        .unwrap();
    let strs: Vec<_> = items.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(
        strs,
        vec!["a", "b", "c"],
        "deep merge should append arrays in load order"
    );
}

#[test]
fn later_file_can_reference_earlier_file_vars() {
    let dir = TempDir::new().unwrap();

    let base = write(
        &dir,
        "config.toml",
        r#"
[vars]
domain = "example.com"
"#,
    );

    let extra = write(
        &dir,
        "config.api.toml",
        r#"
[vars]
api_url = "https://api.{{ vars.domain }}"
"#,
    );

    let mut engine = Engine::new();
    let merged = load_merged([&base, &extra], &mut engine, &Context::new()).unwrap();

    assert_eq!(
        merged.vars.get("api_url").and_then(|v| v.as_str()),
        Some("https://api.example.com"),
        "extra file should resolve {{ vars.domain }} from base"
    );
}

#[test]
fn missing_file_is_an_error() {
    let dir = TempDir::new().unwrap();
    let real = write(&dir, "config.toml", "[vars]\na = \"1\"\n");
    let missing = dir.path().join("does-not-exist.toml");

    let mut engine = Engine::new();
    let result = load_merged([&real, &missing], &mut engine, &Context::new());

    let err = result.expect_err("expected error for missing file");
    assert!(
        err.to_string().contains("does-not-exist.toml"),
        "error should mention the missing path: {err}"
    );
}

#[test]
fn discover_config_files_orders_local_last() {
    let dir = TempDir::new().unwrap();
    write(&dir, "config.toml", "");
    write(&dir, "config.local.toml", "");
    write(&dir, "config.env.toml", "");
    write(&dir, "config.linux.toml", "");
    write(&dir, "other.toml", ""); // should be ignored
    write(&dir, "config.toml.bak", ""); // should be ignored

    let files = discover_config_files(dir.path()).unwrap();
    let names: Vec<_> = files
        .iter()
        .filter_map(|p| p.file_name())
        .filter_map(|n| n.to_str())
        .collect();

    assert_eq!(
        names,
        vec![
            "config.toml",
            "config.env.toml",
            "config.linux.toml",
            "config.local.toml",
        ]
    );
}

#[test]
fn discover_then_load_round_trip() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "config.toml",
        r#"
[vars]
greeting = "hello"

[banner]
text = "{{ vars.greeting }}"
"#,
    );
    write(
        &dir,
        "config.local.toml",
        r#"
[vars]
greeting = "hi"
"#,
    );

    let files = discover_config_files(dir.path()).unwrap();
    let mut engine = Engine::new();
    let merged = load_merged(files.iter(), &mut engine, &Context::new()).unwrap();

    assert_eq!(
        merged.vars.get("greeting").and_then(|v| v.as_str()),
        Some("hi"),
        "local override should win"
    );
    let banner = merged
        .config
        .get("banner")
        .and_then(|v| v.as_table())
        .unwrap();
    assert_eq!(
        banner.get("text").and_then(|v| v.as_str()),
        Some("hello"),
        "first file's banner was rendered with greeting=hello at that moment"
    );
}

#[test]
fn include_directive_loads_referenced_file_first() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "base.toml",
        r#"
[vars]
domain = "example.com"
"#,
    );
    let main = write(
        &dir,
        "config.toml",
        r#"
include = ["base.toml"]

[vars]
api_url = "https://api.{{ vars.domain }}"
"#,
    );

    let mut engine = Engine::new();
    let merged = load_merged([&main], &mut engine, &Context::new()).unwrap();

    assert_eq!(
        merged.vars.get("domain").and_then(|v| v.as_str()),
        Some("example.com"),
        "vars from included file should be present"
    );
    assert_eq!(
        merged.vars.get("api_url").and_then(|v| v.as_str()),
        Some("https://api.example.com"),
        "main file should resolve {{ vars.domain }} from the included file"
    );
}

#[test]
fn include_strips_directive_from_merged_config() {
    let dir = TempDir::new().unwrap();
    write(&dir, "base.toml", "[vars]\na = \"1\"\n");
    let main = write(&dir, "config.toml", "include = [\"base.toml\"]\n");

    let mut engine = Engine::new();
    let merged = load_merged([&main], &mut engine, &Context::new()).unwrap();

    assert!(
        merged.config.get("include").is_none(),
        "the include key must not leak into the merged config"
    );
    assert!(
        merged.config.get("teravars").is_none(),
        "the [teravars] namespace must not leak either"
    );
}

#[test]
fn include_supports_teravars_namespace_form() {
    let dir = TempDir::new().unwrap();
    write(&dir, "base.toml", "[vars]\nfrom_base = \"yes\"\n");
    let main = write(
        &dir,
        "config.toml",
        r#"
[teravars]
include = ["base.toml"]

[vars]
include = "this is a config key, not the directive"
"#,
    );

    let mut engine = Engine::new();
    let merged = load_merged([&main], &mut engine, &Context::new()).unwrap();

    assert_eq!(
        merged.vars.get("from_base").and_then(|v| v.as_str()),
        Some("yes"),
        "namespaced include directive should still work"
    );
}

#[test]
fn include_path_with_system_cwd_template() {
    let dir = TempDir::new().unwrap();
    write(&dir, "extras.toml", "[vars]\nfrom_extras = \"hi\"\n");

    // Build a Context that mimics system_context() but with cwd pinned to the
    // temp dir, so the test does not depend on the actual process cwd.
    let cwd_str = dir.path().to_string_lossy().replace('\\', "/");
    let mut ctx = Context::new();
    ctx.insert(
        "system",
        &serde_json::json!({
            "cwd": cwd_str,
        }),
    );

    let main = write(
        &dir,
        "config.toml",
        r#"include = ["{{ system.cwd }}/extras.toml"]
"#,
    );

    let mut engine = Engine::new();
    let merged = load_merged([&main], &mut engine, &ctx).unwrap();

    assert_eq!(
        merged.vars.get("from_extras").and_then(|v| v.as_str()),
        Some("hi"),
        "system.cwd should be rendered before path resolution"
    );
}

#[test]
fn include_relative_to_current_file() {
    let dir = TempDir::new().unwrap();
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).unwrap();

    fs::write(sub.join("base.toml"), "[vars]\na = \"1\"\n").unwrap();
    let main = sub.join("config.toml");
    fs::write(&main, "include = [\"base.toml\"]\n").unwrap();

    let mut engine = Engine::new();
    let merged = load_merged([&main], &mut engine, &Context::new()).unwrap();

    assert_eq!(
        merged.vars.get("a").and_then(|v| v.as_str()),
        Some("1"),
        "relative include should resolve against the including file's directory"
    );
}

#[test]
fn include_recursive_chain() {
    let dir = TempDir::new().unwrap();
    write(&dir, "c.toml", "[vars]\nc = \"3\"\n");
    write(
        &dir,
        "b.toml",
        r#"include = ["c.toml"]
[vars]
b = "2"
"#,
    );
    let main = write(
        &dir,
        "a.toml",
        r#"include = ["b.toml"]
[vars]
a = "1"
"#,
    );

    let mut engine = Engine::new();
    let merged = load_merged([&main], &mut engine, &Context::new()).unwrap();

    assert_eq!(merged.vars.get("a").and_then(|v| v.as_str()), Some("1"));
    assert_eq!(merged.vars.get("b").and_then(|v| v.as_str()), Some("2"));
    assert_eq!(merged.vars.get("c").and_then(|v| v.as_str()), Some("3"));
}

#[test]
fn include_cycle_is_detected() {
    let dir = TempDir::new().unwrap();
    let a = dir.path().join("a.toml");
    let b = dir.path().join("b.toml");
    fs::write(&a, "include = [\"b.toml\"]\n").unwrap();
    fs::write(&b, "include = [\"a.toml\"]\n").unwrap();

    let mut engine = Engine::new();
    let err = load_merged([&a], &mut engine, &Context::new()).unwrap_err();

    assert!(
        matches!(err, Error::IncludeCycle { .. }),
        "expected IncludeCycle, got: {err:?}"
    );
}

#[test]
fn include_current_file_overrides_included() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "base.toml",
        r#"
[vars]
greeting = "hello-from-base"
"#,
    );
    let main = write(
        &dir,
        "config.toml",
        r#"include = ["base.toml"]

[vars]
greeting = "hello-from-main"
"#,
    );

    let mut engine = Engine::new();
    let merged = load_merged([&main], &mut engine, &Context::new()).unwrap();

    assert_eq!(
        merged.vars.get("greeting").and_then(|v| v.as_str()),
        Some("hello-from-main"),
        "the file declaring the include should win against the included file"
    );
}

#[test]
fn include_conflict_root_and_teravars() {
    let dir = TempDir::new().unwrap();
    write(&dir, "base.toml", "[vars]\nx = \"1\"\n");
    let main = write(
        &dir,
        "config.toml",
        r#"
include = ["base.toml"]

[teravars]
include = ["base.toml"]
"#,
    );

    let mut engine = Engine::new();
    let err = load_merged([&main], &mut engine, &system_context()).unwrap_err();
    assert!(
        matches!(err, Error::IncludeConflict { .. }),
        "expected IncludeConflict, got: {err:?}"
    );
}
