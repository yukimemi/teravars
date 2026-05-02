//! Integration tests for the `merge` feature: multi-file config loading
//! with per-file Tera rendering and deep recursive merge.

#![cfg(feature = "merge")]

use std::fs;

use tempfile::TempDir;
use teravars::{Context, Engine, discover_config_files, load_merged, system_context};

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
