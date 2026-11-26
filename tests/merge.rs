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
fn include_supplies_vars_for_bare_tera_expression() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "machine.toml",
        "[vars]\ngui = \"desktop\"\nwrapper_guis = [\"desktop\"]\n",
    );
    let main = write(
        &dir,
        "config.toml",
        r#"include = ["machine.toml"]

[todoke.gui]
gui = {{ vars.gui in vars.wrapper_guis }}
"#,
    );

    let merged = load_merged([&main], &mut Engine::new(), &Context::new()).unwrap();
    assert_eq!(merged.config["todoke"]["gui"]["gui"].as_bool(), Some(true));
}

#[test]
fn namespaced_include_after_bare_expression_supplies_vars_before_root_overrides() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "machine.toml",
        r#"[vars]
gui = "terminal"
wrapper_guis = ["desktop"]

[todoke.gui]
source = "machine"
"#,
    );
    let main = write(
        &dir,
        "config.toml",
        r#"[todoke.gui]
gui = {{ vars.gui in vars.wrapper_guis }}
source = "root"

[teravars]
include = ["machine.toml"]
"#,
    );

    let merged = load_merged([&main], &mut Engine::new(), &Context::new()).unwrap();
    let gui = &merged.config["todoke"]["gui"];
    assert_eq!(gui["gui"].as_bool(), Some(false));
    assert_eq!(gui["source"].as_str(), Some("root"));
}

#[test]
fn include_like_text_is_not_loaded_and_table_local_include_is_preserved() {
    let dir = TempDir::new().unwrap();
    write(&dir, "machine.toml", "[vars]\nready = true\n");
    let main = write(
        &dir,
        "config.toml",
        r###"# include = ["missing-comment.toml"]
{# include = ["missing-tera-comment.toml"] #}
include = ["machine.toml"]

doc = """
include = ["missing-basic.toml"]
[teravars]
include = ["missing-namespace.toml"]
"""
literal = '''
include = ["missing-literal.toml"]
'''
nested = { include = ["missing-inline.toml"] }

{% if false %}
include = ["missing-conditional.toml"]
{% endif %}

[app]
include = ["missing-table-local.toml"]
ready = {{ vars.ready }}
"###,
    );

    let merged = load_merged([&main], &mut Engine::new(), &Context::new()).unwrap();
    assert_eq!(merged.config["app"]["ready"].as_bool(), Some(true));
    assert_eq!(
        merged.config["app"]["include"][0].as_str(),
        Some("missing-table-local.toml")
    );
}

#[test]
fn include_in_a_conditionally_opened_table_is_not_a_directive() {
    let dir = TempDir::new().unwrap();
    let main = write(
        &dir,
        "config.toml",
        r#"x = {% if true %}1
[app]
{% endif %}
include = ["not-a-file-to-load.toml"]
"#,
    );

    let merged = load_merged([&main], &mut Engine::new(), &Context::new()).unwrap();
    assert_eq!(merged.config["x"].as_integer(), Some(1));
    assert_eq!(
        merged.config["app"]["include"][0].as_str(),
        Some("not-a-file-to-load.toml")
    );
}

#[test]
fn escaped_tera_quote_and_inactive_table_do_not_hide_root_include() {
    let dir = TempDir::new().unwrap();
    write(&dir, "machine.toml", "[vars]\nready = true\n");
    let main = write(
        &dir,
        "config.toml",
        r#"{% set label = 'it\'s %} still a string' %}
{% if false %}
[app]
{% endif %}
include = ["machine.toml"]
ready = {{ vars.ready }}
"#,
    );

    let merged = load_merged([&main], &mut Engine::new(), &Context::new()).unwrap();
    assert_eq!(merged.config["ready"].as_bool(), Some(true));
}

#[test]
fn dead_branch_with_unterminated_toml_string_does_not_hide_include() {
    let dir = TempDir::new().unwrap();
    write(&dir, "machine.toml", "[vars]\nready = true\n");
    let main = write(
        &dir,
        "config.toml",
        r#"{% if false %}
x = '''
{# {% endif %} #}
{% if true %}ignored{% endif %}
{% endif %}
include = ["machine.toml"]
ready = {{ vars.ready }}
"#,
    );
    let merged = load_merged([&main], &mut Engine::new(), &Context::new()).unwrap();
    assert_eq!(merged.config["ready"].as_bool(), Some(true));
}

#[test]
fn raw_table_header_keeps_include_table_local() {
    let dir = TempDir::new().unwrap();
    let main = write(
        &dir,
        "config.toml",
        r#"x = {% raw %}1
[app]
{% endraw %}
include = ["missing.toml"]
"#,
    );
    let merged = load_merged([&main], &mut Engine::new(), &Context::new()).unwrap();
    assert_eq!(merged.config["x"].as_integer(), Some(1));
    assert_eq!(
        merged.config["app"]["include"][0].as_str(),
        Some("missing.toml")
    );
}

#[test]
fn rendered_table_header_keeps_include_table_local() {
    let dir = TempDir::new().unwrap();
    for header in [
        "{{ '[app]' }}",
        "{{- '[app]' }}",
        "{% if true %}\n{{ '[app]' }}\n{% endif %}",
    ] {
        let main = write(
            &dir,
            "config.toml",
            &format!("{header}\ninclude = ['missing.toml']\n"),
        );
        let merged = load_merged([&main], &mut Engine::new(), &Context::new()).unwrap();
        assert_eq!(
            merged.config["app"]["include"][0].as_str(),
            Some("missing.toml")
        );
    }
}

#[test]
fn definite_conditional_namespace_header_loads_include() {
    let dir = TempDir::new().unwrap();
    write(&dir, "machine.toml", "[vars]\nready = true\n");
    let mut ctx = Context::new();
    ctx.insert("enabled", &false);
    for header in [
        "{% if true %}\n[teravars]\n{% endif %}",
        "{%- if true %}\n[teravars]\n{% endif %}",
        "{% if false %}\n[app]\n{% else %}\n[teravars]\n{% endif %}",
        "{% if false %}\n[app]\n{% elif true %}\n[teravars]\n{% endif %}",
        "{% if true %}\n{% if true %}\n[teravars]\n{% endif %}\n{% endif %}",
        "{% if enabled %}\n[app]\n{% endif %}\n{% if true %}\n[teravars]\n{% endif %}",
    ] {
        let main = write(
            &dir,
            "config.toml",
            &format!("{header}\ninclude = ['machine.toml']\n[app]\nready = {{{{ vars.ready }}}}\n"),
        );
        let merged = load_merged([&main], &mut Engine::new(), &ctx)
            .unwrap_or_else(|err| panic!("{header}: {err}"));
        assert_eq!(merged.config["app"]["ready"].as_bool(), Some(true));
    }
}

#[test]
fn conditional_namespace_after_value_expression_requires_static_owner() {
    let dir = TempDir::new().unwrap();
    write(&dir, "machine.toml", "[vars]\nready = true\n");
    let main = write(
        &dir,
        "config.toml",
        "ready = {{ vars.ready }}\n{% if true %}\n[teravars]\n{% endif %}\n\
         include = ['machine.toml']\n",
    );
    let err = load_merged([&main], &mut Engine::new(), &Context::new()).unwrap_err();
    assert!(
        matches!(&err, Error::Extract(message)
            if message.contains("`include`")
                && message.contains("owning table depends on Tera rendering")
                && message.contains("vars.ready")),
        "{err}"
    );

    let main = write(
        &dir,
        "config.toml",
        "ready = {{ vars.ready }}\n[teravars]\ninclude = ['machine.toml']\n",
    );
    let merged = load_merged([&main], &mut Engine::new(), &Context::new()).unwrap();
    assert_eq!(merged.config["ready"].as_bool(), Some(true));
}

#[test]
fn whitespace_trimmed_conditional_header_keeps_include_table_local() {
    let dir = TempDir::new().unwrap();
    for header in [
        "{%- if true %}[teravars]\n{% endif %}",
        "{%- if true -%}\n[teravars]\n{% endif %}",
        "{%- if true %}{% if true -%}\n[teravars]\n{% endif %}{% endif %}",
    ] {
        let main = write(
            &dir,
            "config.toml",
            &format!("[app]\n{{{{ \"#\" }}}}\n{header}\ninclude = ['missing.toml']\n"),
        );
        let merged = load_merged([&main], &mut Engine::new(), &Context::new())
            .unwrap_or_else(|err| panic!("{header}: {err}"));
        assert_eq!(
            merged.config["app"]["include"][0].as_str(),
            Some("missing.toml")
        );
    }
}

#[test]
fn conditional_header_after_rendered_multiline_string_opening_is_ambiguous() {
    let dir = TempDir::new().unwrap();
    let mut ctx = Context::new();
    ctx.insert("closing", &"\"\"\"");
    for (statement, opening) in [
        ("{{ opening }}", "note = \"\"\""),
        ("note = {{ opening }}", "\"\"\""),
    ] {
        ctx.insert("opening", &opening);
        for intermediate in ["", "[other]\n"] {
            let main = write(
                &dir,
                "config.toml",
                &format!(
                    "[app]\n{statement}\n{intermediate}\
                     {{% if true %}}\n[teravars]\n{{% endif %}}\n\
                     include = ['missing.toml']\n{{{{ closing }}}}\n"
                ),
            );
            let merged = load_merged([&main], &mut Engine::new(), &ctx)
                .unwrap_or_else(|err| panic!("{statement}: {err}"));
            assert_eq!(
                merged.config["app"]["note"].as_str(),
                Some(
                    format!("{intermediate}\n[teravars]\n\ninclude = ['missing.toml']\n").as_str()
                )
            );
            assert!(!merged.config.contains_key("other"));
        }
    }
}

#[test]
fn potentially_active_namespace_header_keeps_include_ambiguous() {
    let dir = TempDir::new().unwrap();
    write(&dir, "machine.toml", "[vars]\nready = true\n");
    let mut ctx = Context::new();
    ctx.insert("enabled", &false);
    for header in [
        "{% if enabled %}\n[app]\n{% else %}\n[teravars]\n{% endif %}",
        "{% if enabled %}\n[app]\n{% elif true %}\n[teravars]\n{% endif %}",
        "{% if not enabled %}\n{% if true %}\n[teravars]\n{% endif %}\n{% endif %}",
        "{% for item in [1] %}\n{% if true %}\n[teravars]\n{% endif %}\n{% endfor %}",
    ] {
        let main = write(
            &dir,
            "config.toml",
            &format!("{header}\ninclude = ['machine.toml']\n"),
        );
        let err = load_merged([&main], &mut Engine::new(), &ctx).unwrap_err();
        assert!(
            matches!(&err, Error::Extract(message)
                if message.contains("config.toml `teravars.include`")
                    && message.contains("owning table depends on Tera rendering")),
            "{header}: {err}"
        );
    }
}

#[test]
fn rendered_namespace_header_makes_include_ambiguous() {
    let dir = TempDir::new().unwrap();
    let main = write(
        &dir,
        "config.toml",
        "{{ '[teravars]' }}\ninclude = ['missing.toml']\n",
    );
    let err = load_merged([&main], &mut Engine::new(), &Context::new()).unwrap_err();
    assert!(
        matches!(&err, Error::Extract(message)
            if message.contains("config.toml `teravars.include`")
                && message.contains("owning table depends on Tera")),
        "{err}"
    );
}

#[test]
fn templated_header_include_depends_on_rendered_owner() {
    let dir = TempDir::new().unwrap();
    let mut ctx = Context::new();
    for (header, section) in [
        (r#"["{{ section }}"]"#, "teravars"),
        ("[{{ section }}]", "teravars"),
        ("[tera{{ section }}]", "vars"),
        ("[tera{% if section == 'vars' %}vars{% endif %}]", "vars"),
        (r#"["tera{# comment #}vars"]"#, "teravars"),
    ] {
        ctx.insert("section", &section);
        let main = write(
            &dir,
            "config.toml",
            &format!("{header}\ninclude = ['machine.toml']\n"),
        );
        let err = load_merged([&main], &mut Engine::new(), &ctx).unwrap_err();
        assert!(
            matches!(&err, Error::Extract(message)
                if message.contains("config.toml `teravars.include`")
                    && message.contains("owning table depends on Tera rendering")),
            "{header}: {err}"
        );
    }
}

#[test]
fn templated_header_keeps_application_include_and_restores_static_ownership() {
    let dir = TempDir::new().unwrap();
    write(&dir, "machine.toml", "[vars]\nready = true\n");
    let mut ctx = Context::new();
    ctx.insert("section", &"app");
    let main = write(
        &dir,
        "config.toml",
        "[\"{{ section }}\"]\ninclude = ['missing.toml']\nready = {{ vars.ready }}\n\
         [teravars]\ninclude = ['machine.toml']\n",
    );
    let merged = load_merged([&main], &mut Engine::new(), &ctx).unwrap();
    assert_eq!(merged.config["app"]["ready"].as_bool(), Some(true));
    assert_eq!(
        merged.config["app"]["include"][0].as_str(),
        Some("missing.toml")
    );
}

#[test]
fn static_namespace_header_with_trailing_tera_statement_loads_include() {
    let dir = TempDir::new().unwrap();
    write(&dir, "machine.toml", "[vars]\nready = true\n");
    let main = write(
        &dir,
        "config.toml",
        "ready = {{ vars.ready }}\n[teravars] {% set enabled = true %}\n\
         include = ['machine.toml']\n",
    );
    let merged = load_merged([&main], &mut Engine::new(), &Context::new()).unwrap();
    assert_eq!(merged.config["ready"].as_bool(), Some(true));
}

#[test]
fn static_header_after_rendered_header_restores_include_ownership() {
    let dir = TempDir::new().unwrap();
    write(&dir, "machine.toml", "[vars]\nready = true\n");
    let main = write(
        &dir,
        "config.toml",
        "{{ '[app]' }}\nready = {{ vars.ready }}\n[teravars]\ninclude = ['machine.toml']\n",
    );
    let merged = load_merged([&main], &mut Engine::new(), &Context::new()).unwrap();
    assert_eq!(merged.config["app"]["ready"].as_bool(), Some(true));
}

#[test]
fn root_include_after_value_expression_remains_static() {
    let dir = TempDir::new().unwrap();
    write(&dir, "machine.toml", "[vars]\nready = true\n");
    let main = write(
        &dir,
        "config.toml",
        "ready = {{ vars.ready }}\ninclude = ['machine.toml']\n",
    );
    let merged = load_merged([&main], &mut Engine::new(), &Context::new()).unwrap();
    assert_eq!(merged.config["ready"].as_bool(), Some(true));
}

#[test]
fn rendered_root_include_with_ambiguous_ownership_is_an_error() {
    let dir = TempDir::new().unwrap();
    let main = write(
        &dir,
        "config.toml",
        r#"{% if show_app %}
[app]
{% endif %}
include = ["machine.toml"]
"#,
    );
    let mut ctx = Context::new();
    ctx.insert("show_app", &false);

    let err = load_merged([&main], &mut Engine::new(), &ctx).unwrap_err();
    assert!(
        matches!(&err, Error::Extract(message)
            if message.contains("config.toml `include`")
            && message.contains("owning table depends on Tera rendering")),
        "{err}"
    );
}

#[test]
fn render_error_keeps_the_ambiguous_include_diagnostic() {
    let dir = TempDir::new().unwrap();
    write(&dir, "machine.toml", "[vars]\nready = true\n");
    let main = write(
        &dir,
        "config.toml",
        r#"{% if show_app %}
[app]
{% endif %}
include = ["machine.toml"]
ready = "{{ vars.ready }}"
"#,
    );
    let mut ctx = Context::new();
    ctx.insert("show_app", &false);

    let err = load_merged([&main], &mut Engine::new(), &ctx).unwrap_err();
    assert!(
        matches!(&err, Error::Extract(message)
            if message.contains("config.toml `include`")
                && message.contains("owning table depends on Tera rendering")
                && message.contains("ready")),
        "{err}"
    );
}

#[test]
fn malformed_include_reports_the_owning_file_before_loading_paths() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "machine.toml",
        "[teravars]\ninclude = ['missing.toml', 42]\n",
    );
    let main = write(&dir, "config.toml", "include = ['machine.toml']\n");

    let err = load_merged([&main], &mut Engine::new(), &Context::new()).unwrap_err();
    let Error::Extract(message) = err else {
        panic!("expected an include diagnostic, got: {err}");
    };
    assert!(
        message.contains("machine.toml `teravars.include`"),
        "{message}"
    );
    assert!(message.contains("expected a string"), "{message}");
}

#[test]
fn root_include_requires_an_array_not_a_string_or_bare_expression() {
    let dir = TempDir::new().unwrap();
    for (name, text) in [
        ("scalar.toml", r#"include = "machine.toml""#),
        ("expression.toml", "include = {{ vars.paths }}"),
    ] {
        let main = write(&dir, name, text);
        let err = load_merged([&main], &mut Engine::new(), &Context::new()).unwrap_err();
        let Error::Extract(message) = err else {
            panic!("expected an include diagnostic, got: {err}");
        };
        assert!(
            message.contains(&format!("{name} `include`: ")),
            "{message}"
        );
    }
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
fn vars_self_ref_with_tera_block_in_value() {
    // Regression for issue #21: a [vars] value that contains a Tera control
    // block (e.g. `{% if is_windows() %}...{% endif %}`) is a *value*, not a
    // control-flow line, and must be extracted into the vars table so other
    // [vars] entries can refer to it.
    let dir = TempDir::new().unwrap();
    let path = write(
        &dir,
        "config.toml",
        r#"
[vars]
base = '''{% if is_windows() %}C:\Kanade{% else %}/var/lib/kanade{% endif %}'''
log_dir = '''{{ vars.base }}{% if is_windows() %}\logs{% else %}/logs{% endif %}'''

[log]
path = '{{ vars.log_dir }}/agent.log'
"#,
    );

    let mut engine = Engine::new();
    let merged = load_merged([&path], &mut engine, &system_context()).unwrap();

    let log = merged.config.get("log").and_then(|v| v.as_table()).unwrap();
    let rendered = log.get("path").and_then(|v| v.as_str()).unwrap();

    if cfg!(windows) {
        assert_eq!(rendered, r"C:\Kanade\logs/agent.log");
    } else {
        assert_eq!(rendered, "/var/lib/kanade/logs/agent.log");
    }
}

#[test]
fn vars_value_references_system_context() {
    // Regression for issue #21: a [vars] entry that references the caller-
    // supplied context (e.g. `{{ system.host }}`) must resolve so downstream
    // [vars] and config consumers see the concrete value, not the literal
    // template string.
    let dir = TempDir::new().unwrap();
    let path = write(
        &dir,
        "config.toml",
        r#"
[vars]
who = '{{ system.host }}'
banner = 'host={{ vars.who }}'

[agent]
id = '{{ vars.banner }}'
"#,
    );

    let mut ctx = Context::new();
    ctx.insert(
        "system",
        &serde_json::json!({
            "host": "myhost",
        }),
    );

    let mut engine = Engine::new();
    let merged = load_merged([&path], &mut engine, &ctx).unwrap();

    assert_eq!(
        merged.vars.get("who").and_then(|v| v.as_str()),
        Some("myhost"),
    );
    assert_eq!(
        merged.vars.get("banner").and_then(|v| v.as_str()),
        Some("host=myhost"),
    );
    let agent = merged
        .config
        .get("agent")
        .and_then(|v| v.as_table())
        .unwrap();
    assert_eq!(
        agent.get("id").and_then(|v| v.as_str()),
        Some("host=myhost")
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

#[test]
fn comments_are_inert_and_never_rendered() {
    // Regression: a documented config file is full of commented-out samples
    // and notes that quote teravars syntax. Rendering the file as a Tera
    // template also rendered those comments, so `# port = "{{ vars.x }}"`
    // failed with "field not defined" and `# disable: {% if false %}` with
    // "unexpected end of input" — the whole load died on a comment.
    let dir = TempDir::new().unwrap();
    let path = write(
        &dir,
        "config.toml",
        r#"
# sample: port = "{{ vars.not_defined }}"
# to disable the block below: {% if false %}
# {% endfor %} {{ broken |

[vars] # app vars
name = "app"
# disabled = "{{ vars.also_not_defined }}"

[server]
title = "{{ vars.name }}"   # trailing note with {{ vars.nope }}
url = "https://example.com/#anchor"
"#,
    );

    let mut engine = Engine::new();
    let merged = load_merged([&path], &mut engine, &system_context()).unwrap();

    assert_eq!(
        merged.vars.get("name").and_then(|v| v.as_str()),
        Some("app")
    );
    assert!(
        merged.vars.get("disabled").is_none(),
        "a commented-out [vars] entry must not become a var"
    );

    let server = merged
        .config
        .get("server")
        .and_then(|v| v.as_table())
        .unwrap();
    assert_eq!(server.get("title").and_then(|v| v.as_str()), Some("app"));
    // A `#` inside a string is data, not a comment.
    assert_eq!(
        server.get("url").and_then(|v| v.as_str()),
        Some("https://example.com/#anchor")
    );
}

#[test]
fn commented_out_include_is_not_loaded() {
    let dir = TempDir::new().unwrap();
    let path = write(
        &dir,
        "config.toml",
        r#"
# include = ["does-not-exist.toml"]
include = ["base.toml"]   # only this one counts

[vars]
x = "1"
"#,
    );
    write(&dir, "base.toml", "[vars]\ny = \"2\"\n");

    let mut engine = Engine::new();
    let merged = load_merged([&path], &mut engine, &system_context()).unwrap();

    assert_eq!(merged.vars.get("x").and_then(|v| v.as_str()), Some("1"));
    assert_eq!(merged.vars.get("y").and_then(|v| v.as_str()), Some("2"));
}

#[test]
fn render_error_line_numbers_survive_comment_stripping() {
    // Comment text is dropped but the lines it occupied are kept, so a real
    // render failure still points at the line the author sees on disk.
    let dir = TempDir::new().unwrap();
    let path = write(
        &dir,
        "config.toml",
        r#"
# note
# note
[server]
title = "{{ vars.genuinely_missing }}"
"#,
    );

    let mut engine = Engine::new();
    let err = load_merged([&path], &mut engine, &system_context()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains(":5:"),
        "expected the error to point at line 5, got: {msg}"
    );
}

/// Shaped like renri's real `renri.toml`: documented examples living in
/// comments, a `#` inside prose, and the deferred `{{ vcs.* }}` idiom.
#[test]
fn consumer_shaped_config_with_documented_comments() {
    let dir = TempDir::new().unwrap();
    let path = write(
        &dir,
        "renri.toml",
        r#"
# renri.toml — worktree config.
# Surface the GitHub PR # next to each worktree in `renri list`.
#
# Example hook (uncomment to use):
# [[hooks.post_create]]
# type    = "command"
# run     = "echo {{ vars.branch_slug }} {{ vcs.repo }}"
# disable = {% if false %}

[vars]
port = "{{ 'main' | hash | port_offset(start=3000, range=100) }}"

[ui]
show_pr = true

[layout]
worktree_path = "{{ vcs.repo }}/{{ vcs.branch }}"

[[hooks.post_create]]
type = "command"
run = "cargo make on-add --port {{ vars.port }}"
"#,
    );

    let mut ctx = system_context();
    // The documented deferred-template idiom: self-referential placeholders
    // so `vcs.*` survives config load and renders in a later pass.
    ctx.insert(
        "vcs",
        &serde_json::json!({ "repo": "{{ vcs.repo }}", "branch": "{{ vcs.branch }}" }),
    );

    let mut engine = Engine::new();
    let merged = load_merged([&path], &mut engine, &ctx).unwrap();

    let hooks = merged
        .config
        .get("hooks")
        .and_then(|v| v.as_table())
        .unwrap()
        .get("post_create")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(hooks.len(), 1, "the commented-out hook must not be loaded");

    let port: u32 = merged.vars["port"].as_str().unwrap().parse().unwrap();
    assert!((3000..3100).contains(&port), "port was {port}");
    assert_eq!(
        hooks[0].get("run").and_then(|v| v.as_str()),
        Some(format!("cargo make on-add --port {port}").as_str())
    );

    let layout = merged
        .config
        .get("layout")
        .and_then(|v| v.as_table())
        .unwrap();
    assert_eq!(
        layout.get("worktree_path").and_then(|v| v.as_str()),
        Some("{{ vcs.repo }}/{{ vcs.branch }}"),
        "deferred vcs.* placeholders must survive the load pass"
    );
}

#[test]
fn runtime_expressions_survive_includes_and_second_render() {
    use teravars::{LoadOptions, load_merged_with};
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "base.toml",
        r#"
[vars]
prefix = "run"
runtime = '{{ group }}'
alias = '{{ vars.runtime }}'
raw_runtime = '{% raw %}{{ group }}{% endraw %}'
"#,
    );
    let path = write(
        &dir,
        "config.toml",
        r##"
include = ["base.toml"]
[commands]
todoke = '{{ vars.prefix }} {{ group }} {{ file_path }} {{ cap[0] }} {{ passthrough | join(sep=" ") }}'
shun = '{{ args }} {{ args_list | join(sep=",") }} {{ file_name }} {{ file_dir }}'
mixed = '{{ group ~ ":" ~ vars.prefix }}'
propagated = '{{ vars.alias }}'
path_replace = "{{ file_path | replace(from='\\', to='/') }}"
named_capture = '{{ cap.x }}'
literal_tag = "{{ '{{ group }}' }}"
trim_unicode = "a {{- group -}} b"
trim = 'a   {{- group -}}   b'
collision = '__teravars_deferred_0__'
raw = '{% raw %}{{ group }}{% endraw %}'
literal = '{{ "group" }}'
attribute = '{{ vars.runtime_name }}'
quoted = "{{ cap[0] | replace(from="a", to="#") }}" # trailing comment
array = ['{{ group }}', { key = '{{ cap[0] }}' }]
# {% if group %} ignored
commented = 'a{# {{ group }} #}b'
[vars]
runtime_name = "ordinary"
"##,
    );
    let options = LoadOptions::new()
        .defer("group")
        .defer("file_path")
        .defer("cap")
        .defer("passthrough")
        .defer("args")
        .defer("args_list")
        .defer_prefix("file_");
    let mut engine = Engine::new();
    let mut ctx = Context::new();
    let merged = load_merged_with([&path], &mut engine, &ctx, &options).unwrap();
    let commands = merged.config["commands"].as_table().unwrap();
    assert_eq!(
        commands["todoke"].as_str().unwrap(),
        r#"run {{ group }} {{ file_path }} {{ cap[0] }} {{ passthrough | join(sep=" ") }}"#
    );
    assert_eq!(
        commands["mixed"].as_str().unwrap(),
        r#"{{ group ~ ":" ~ vars.prefix }}"#
    );
    assert_eq!(commands["propagated"].as_str().unwrap(), "{{ group }}");
    assert_eq!(commands["literal_tag"].as_str().unwrap(), "{{ group }}");
    assert_eq!(commands["named_capture"].as_str().unwrap(), "{{ cap.x }}");
    assert_eq!(
        commands["path_replace"].as_str().unwrap(),
        r#"{{ file_path | replace(from='\\', to='/') }}"#
    );
    assert_eq!(commands["trim"].as_str().unwrap(), "a{{- group -}}b");
    assert_eq!(
        commands["collision"].as_str().unwrap(),
        "__teravars_deferred_0__"
    );
    assert_eq!(commands["raw"].as_str().unwrap(), "{{ group }}");
    assert_eq!(commands["literal"].as_str().unwrap(), "group");
    assert_eq!(commands["attribute"].as_str().unwrap(), "ordinary");
    assert_eq!(merged.vars["raw_runtime"].as_str().unwrap(), "{{ group }}");
    assert_eq!(
        commands["quoted"].as_str().unwrap(),
        r##"{{ cap[0] | replace(from="a", to="#") }}"##
    );
    assert_eq!(commands["array"][0].as_str().unwrap(), "{{ group }}");
    assert_eq!(
        commands["array"][1]["key"].as_str().unwrap(),
        "{{ cap[0] }}"
    );
    assert_eq!(commands["commented"].as_str().unwrap(), "ab");
    ctx.insert("vars", &merged.vars);
    ctx.insert("group", &"dev");
    ctx.insert("file_path", &"path");
    ctx.insert("cap", &["capture"]);
    ctx.insert("passthrough", &["a", "b"]);
    ctx.insert("args", &"one two");
    ctx.insert("args_list", &["one", "two"]);
    ctx.insert("file_name", &"name");
    ctx.insert("file_dir", &"dir");
    for (key, expected) in [
        ("todoke", "run dev path capture a b"),
        ("shun", "one two one,two name dir"),
        ("mixed", "dev:run"),
        ("propagated", "dev"),
        ("trim", "adevb"),
    ] {
        assert_eq!(
            engine
                .render(commands[key].as_str().unwrap(), &ctx)
                .unwrap(),
            expected
        );
    }
    ctx.insert("cap", &serde_json::json!({"x": "named"}));
    ctx.insert("file_path", &r"C:\a\b.txt");
    for (key, expected) in [("named_capture", "named"), ("path_replace", "C:/a/b.txt")] {
        assert_eq!(
            engine
                .render(commands[key].as_str().unwrap(), &ctx)
                .unwrap(),
            expected
        );
    }
    assert_eq!(
        commands["trim_unicode"].as_str().unwrap(),
        engine.render("a {{- '{{- group -}}' -}} b", &ctx).unwrap()
    );
}

#[test]
fn runtime_expressions_do_not_defer_unrelated_or_local_names() {
    use teravars::{LoadOptions, load_merged_with};
    let dir = TempDir::new().unwrap();
    let path = write(
        &dir,
        "config.toml",
        r#"
{% set group = "local" %}
local = '{{ group }}'
{% if true %}{% set file_branch = "yes" %}{% else %}{% set file_branch = "no" %}{% endif %}
branch = '{{ file_branch }}'
{% for file_name in ["a", "b"] %}
[[items]]
name = '{{ file_name }}'
{% endfor %}
[other]
comprehension = '{{ [group for group in ["a", "b"]] | join(sep=",") }}'
"#,
    );
    let options = LoadOptions::new().defer("group").defer_prefix("file_");
    let mut engine = Engine::new();
    let merged = load_merged_with([&path], &mut engine, &Context::new(), &options).unwrap();
    assert_eq!(merged.config["local"].as_str().unwrap(), "local");
    assert_eq!(merged.config["branch"].as_str().unwrap(), "yes");
    assert_eq!(merged.config["items"][0]["name"].as_str().unwrap(), "a");
    assert_eq!(
        merged.config["other"]["comprehension"].as_str().unwrap(),
        "a,b"
    );
    let path = write(&dir, "bad.toml", "value = '{{ group_extra }}'");
    let err = load_merged_with([&path], &mut engine, &Context::new(), &options).unwrap_err();
    assert!(err.to_string().contains("group_extra"), "{err}");
}

#[test]
fn runtime_expressions_reject_structural_contexts() {
    use teravars::{LoadOptions, load_merged_with};
    let dir = TempDir::new().unwrap();
    let options = LoadOptions::new().defer("group");
    let mut engine = Engine::new();
    for source in [
        "include = ['{{ group }}']",
        "value = {{ group }}",
        "'{{ group }}' = 'value'",
        "['{{ group }}']\nvalue = 'x'",
        "{% if group %}\nvalue = 'x'\n{% endif %}",
        "{% set x = group %}\nvalue = '{{ x }}'",
        "{% for x in ['a'] %}\nvalue = '{{ group ~ x }}'\n{% endfor %}",
        "{% if flag %}{% set group = 'x' %}{% endif %}\nvalue = '{{ group }}'",
        "{{ '#' }} value = '{{ group }}'",
    ] {
        let path = write(&dir, "bad.toml", source);
        let err = load_merged_with([&path], &mut engine, &Context::new(), &options).unwrap_err();
        assert!(
            err.to_string().contains("runtime variable"),
            "{source}: {err}"
        );
    }
}

#[test]
fn runtime_deferral_does_not_relax_ambiguous_include_ownership() {
    use teravars::{LoadOptions, load_merged_with};
    let dir = TempDir::new().unwrap();
    let path = write(
        &dir,
        "config.toml",
        r#"
{% if show_app %}
[app]
{% endif %}
include = ["missing.toml"]
[commands]
run = "{{ group }}"
"#,
    );
    let mut ctx = Context::new();
    ctx.insert("show_app", &false);
    let err = load_merged_with(
        [&path],
        &mut Engine::new(),
        &ctx,
        &LoadOptions::new().defer("group"),
    )
    .unwrap_err();
    assert!(
        matches!(&err, Error::Extract(message)
            if message.contains("config.toml `include`")
                && message.contains("owning table depends on Tera rendering")),
        "{err}"
    );
}

#[test]
fn runtime_options_reject_invalid_identifiers() {
    use teravars::{LoadOptions, load_merged_with};
    let dir = TempDir::new().unwrap();
    let path = write(&dir, "config.toml", "value = 'ordinary'");
    for invalid in ["", "9name", "file_*", "a.b", "a b", "é"] {
        for options in [
            LoadOptions::new().defer(invalid),
            LoadOptions::new().defer_prefix(invalid),
        ] {
            let err = load_merged_with([&path], &mut Engine::new(), &Context::new(), &options)
                .unwrap_err();
            assert!(
                err.to_string().contains("invalid deferred variable"),
                "{err}"
            );
        }
    }
}

#[test]
fn runtime_aliases_cannot_drive_load_time_operations() {
    use teravars::{LoadOptions, load_merged_with};
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "base.toml",
        "[vars]\nruntime = '{{ group }}'\nalias = '{{ vars.runtime }}'\nordinary = 'abc'\n",
    );
    let options = LoadOptions::new().defer("group");
    for source in [
        "[app]\n{% if vars.runtime == 'dev' %}\nvalue = 'yes'\n{% endif %}",
        "[app]\nvalue = '{{ vars.runtime | length }}'",
        "[app]\nvalue = '{{ vars.alias | upper }}'",
        "[app]\nvalue = '{{ vars[\"runtime\"] | length }}'",
        "[app]\n{% set x = vars.runtime %}\nvalue = '{{ x }}'",
        "[vars]\nsecond = '{{ vars.alias }}'\nlength = '{{ vars.second | length }}'",
    ] {
        let path = write(
            &dir,
            "config.toml",
            &format!("include = ['base.toml']\n{source}"),
        );
        let err =
            load_merged_with([&path], &mut Engine::new(), &Context::new(), &options).unwrap_err();
        assert!(
            err.to_string().contains("runtime variable value"),
            "{source}: {err}"
        );
    }
    let path = write(
        &dir,
        "config.toml",
        "include = ['base.toml']\n[app]\nvalue = '{{ vars.ordinary | length }}'\ncopy = '{{ vars[\"alias\"] }}'",
    );
    let merged = load_merged_with([&path], &mut Engine::new(), &Context::new(), &options).unwrap();
    assert_eq!(merged.config["app"]["value"].as_str(), Some("3"));
    assert_eq!(merged.config["app"]["copy"].as_str(), Some("{{ group }}"));
}

#[test]
fn malformed_runtime_expression_is_not_deferred() {
    use teravars::{LoadOptions, load_merged_with};
    let dir = TempDir::new().unwrap();
    let options = LoadOptions::new().defer("group");
    for expression in ["{{ group + }}", "{{ group - }}", "{{- group - }}"] {
        let path = write(&dir, "config.toml", &format!("value = '{expression}'"));
        assert!(
            load_merged_with([&path], &mut Engine::new(), &Context::new(), &options).is_err(),
            "{expression}"
        );
    }
    let path = write(&dir, "config.toml", "value = '{{ group | runtime_only }}'");
    let merged = load_merged_with([&path], &mut Engine::new(), &Context::new(), &options).unwrap();
    assert_eq!(
        merged.config["value"].as_str(),
        Some("{{ group | runtime_only }}")
    );
}

#[test]
fn runtime_names_shadowed_in_extracted_vars_keep_load_time_resolution() {
    use teravars::{LoadOptions, load_merged_with};
    let dir = TempDir::new().unwrap();
    let path = write(
        &dir,
        "config.toml",
        "{% set group = 'local' %}\n[vars]\na = '{{ group }}'\n[app]\nvalue = '{{ vars.a | upper }}'",
    );
    for caller_group in [None, Some("load")] {
        let mut ctx = Context::new();
        if let Some(group) = caller_group {
            ctx.insert("group", &group);
        }
        let baseline = load_merged([&path], &mut Engine::new(), &ctx).unwrap();
        let merged = load_merged_with(
            [&path],
            &mut Engine::new(),
            &ctx,
            &LoadOptions::new().defer("group"),
        )
        .unwrap();
        assert_eq!(merged.vars, baseline.vars);
        assert_eq!(merged.config, baseline.config);
    }
}

#[test]
fn shadowed_vars_keep_load_time_filters_alongside_free_runtime_references() {
    use teravars::{LoadOptions, load_merged_with};
    let dir = TempDir::new().unwrap();
    let path = write(
        &dir,
        "config.toml",
        "[app]\nfree = '{{ group }}'\n{% set group = 'local' %}\n\
         [vars]\na = { items = ['{{ group }}'] }\n\
         b = '{{ vars.a.items[0] | length }}'\n\
         [other]\nvalue = '{{ vars.a.items[0] | upper }}'",
    );
    let merged = load_merged_with(
        [&path],
        &mut Engine::new(),
        &Context::new(),
        &LoadOptions::new().defer("group"),
    )
    .unwrap();
    assert_eq!(merged.config["app"]["free"].as_str(), Some("{{ group }}"));
    assert_eq!(merged.vars["a"]["items"][0].as_str(), Some("local"));
    assert_eq!(merged.vars["b"].as_str(), Some("11"));
    assert_eq!(merged.config["other"]["value"].as_str(), Some("{{ GROUP }}"));
}

#[test]
fn inherited_raw_runtime_reference_is_not_exempted_by_identical_local_value() {
    use teravars::{LoadOptions, load_merged_with};
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "base.toml",
        "[vars]\nitems = ['{% raw %}{{ group }}{% endraw %}']",
    );
    let path = write(
        &dir,
        "config.toml",
        "include = ['base.toml']\n{% set group = 'local' %}\n\
         [vars]\nitems = ['{{ group }}']\n\
         [app]\nvalue = '{{ vars.items[0] | upper }}'",
    );
    let err = load_merged_with(
        [&path],
        &mut Engine::new(),
        &Context::new(),
        &LoadOptions::new().defer("group"),
    )
    .unwrap_err();
    assert!(err.to_string().contains("runtime variable value"), "{err}");
}

#[test]
fn embedded_string_source_loads_and_merges_with_file_override() {
    use teravars::{ConfigSource, load_merged_sources};

    let dir = TempDir::new().unwrap();
    let user_cfg = write(
        &dir,
        "config.toml",
        r#"
[vars]
greeting = "hi"

[app]
banner = "{{ vars.greeting }} from {{ vars.origin }}"
tags = ["user"]
"#,
    );

    let embedded = ConfigSource::string(
        "<embedded defaults>",
        r#"
[vars]
greeting = "hello"
origin = "defaults"

[app]
banner = "{{ vars.greeting }} from {{ vars.origin }}"
timeout_ms = 5000
tags = ["default"]
"#,
    );

    let mut engine = Engine::new();
    let merged = load_merged_sources(
        [embedded, ConfigSource::path(&user_cfg)],
        &mut engine,
        &Context::new(),
    )
    .unwrap();

    assert_eq!(merged.vars["greeting"].as_str(), Some("hi"));
    assert_eq!(merged.vars["origin"].as_str(), Some("defaults"));
    assert_eq!(
        merged.config["app"]["banner"].as_str(),
        Some("hi from defaults")
    );
    assert_eq!(merged.config["app"]["timeout_ms"].as_integer(), Some(5000));
    let tags: Vec<_> = merged.config["app"]["tags"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(tags, vec!["default", "user"]);
}

#[test]
fn string_source_relative_nested_includes_and_rendered_root() {
    use teravars::{ConfigSource, SourceId, load_merged_rendered};

    let dir = TempDir::new().unwrap();
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).unwrap();

    let leaf = sub.join("leaf.toml");
    fs::write(
        &leaf,
        r#"
[vars]
host = "leaf.example"
port = 8080

[server]
owner = "leaf"
"#,
    )
    .unwrap();

    let mid = sub.join("mid.toml");
    fs::write(
        &mid,
        r#"
include = ["leaf.toml"]

[vars]
base_url = "https://{{ vars.host }}:{{ vars.port }}"

[server]
owner = "mid"
"#,
    )
    .unwrap();

    let embedded = ConfigSource::string_with_base_dir(
        "embedded.toml",
        r#"
# comment with {{ vars.not_defined }}
include = ["sub/mid.toml"]

[server]
url = "{{ vars.base_url }}/api"
owner = "root"
"#,
        dir.path(),
    );

    let mut engine = Engine::new();
    let rendered = load_merged_rendered([embedded], &mut engine, &Context::new()).unwrap();

    assert_eq!(rendered.sources.len(), 3);
    assert_eq!(rendered.sources[0].id, SourceId::Path(leaf));
    assert!(!rendered.sources[0].is_root);
    assert_eq!(rendered.sources[1].id, SourceId::Path(mid));
    assert!(!rendered.sources[1].is_root);
    assert_eq!(
        rendered.sources[2].id,
        SourceId::Name("embedded.toml".into())
    );
    assert!(rendered.sources[2].is_root);

    let root_text = rendered.rendered_root().expect("single root");
    assert!(root_text.contains(r#"url = "https://leaf.example:8080/api""#));
    assert!(root_text.contains(r#"include = ["sub/mid.toml"]"#));
    assert!(!root_text.contains("vars.not_defined"));

    assert_eq!(
        rendered.merged.config["server"]["owner"].as_str(),
        Some("root"),
        "declaring string source should override included files"
    );
    assert_eq!(
        rendered.merged.config["server"]["url"].as_str(),
        Some("https://leaf.example:8080/api")
    );
    assert!(rendered.merged.config.get("include").is_none());
}

#[test]
fn string_source_allows_absolute_include_without_base_dir() {
    use teravars::{ConfigSource, load_merged_sources};

    let dir = TempDir::new().unwrap();
    let abs = write(&dir, "abs.toml", "[vars]\nfrom_abs = \"yes\"\n");
    let abs_str = abs.to_string_lossy().replace('\\', "/");

    let embedded = ConfigSource::string(
        "embedded.toml",
        format!("include = [\"{abs_str}\"]\n[app]\nval = \"{{{{ vars.from_abs }}}}\"\n"),
    );

    let merged = load_merged_sources([embedded], &mut Engine::new(), &Context::new()).unwrap();
    assert_eq!(merged.config["app"]["val"].as_str(), Some("yes"));
}

#[test]
fn string_source_rejects_relative_include_without_base_dir_and_invalid_base_dir() {
    use teravars::{ConfigSource, load_merged_sources};

    // Relative include with base_dir = None must fail with a named error
    // rather than silently resolving against CWD.
    let err = load_merged_sources(
        [ConfigSource::string(
            "embedded.toml",
            "include = [\"Cargo.toml\"]\n",
        )],
        &mut Engine::new(),
        &Context::new(),
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(matches!(err, Error::Extract(_)), "{err}");
    assert!(msg.contains("embedded.toml"), "{msg}");
    assert!(msg.contains("Cargo.toml"), "{msg}");
    assert!(msg.contains("base_dir"), "{msg}");

    // Empty and relative base_dir must be rejected with a named error.
    for bad_base in ["", "relative/dir", "."] {
        let err = load_merged_sources(
            [ConfigSource::string_with_base_dir(
                "embedded.toml",
                "x = 1\n",
                std::path::Path::new(bad_base),
            )],
            &mut Engine::new(),
            &Context::new(),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(matches!(err, Error::Extract(_)), "{bad_base}: {err}");
        assert!(msg.contains("embedded.toml"), "{bad_base}: {msg}");
        assert!(msg.contains("base_dir"), "{bad_base}: {msg}");
    }

    // Empty / whitespace-only source name must be rejected.
    for bad_name in ["", "   "] {
        let err = load_merged_sources(
            [ConfigSource::string(bad_name, "x = 1\n")],
            &mut Engine::new(),
            &Context::new(),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Extract(_)), "{bad_name:?}: {err}");
    }
}

#[cfg(windows)]
#[test]
fn string_source_rejects_drive_relative_include() {
    use teravars::{ConfigSource, load_merged_sources};

    let err = load_merged_sources(
        [ConfigSource::string_with_base_dir(
            "embedded.toml",
            "include = [\"D:foo.toml\"]\n",
            std::path::Path::new("C:/configs"),
        )],
        &mut Engine::new(),
        &Context::new(),
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(matches!(err, Error::Extract(_)), "{msg}");
    assert!(msg.contains("embedded.toml"), "{msg}");
    assert!(msg.contains("D:foo.toml"), "{msg}");
    assert!(msg.contains("base_dir"), "{msg}");
}

#[test]
fn string_source_named_diagnostics_and_ownership_protection() {
    use teravars::{ConfigSource, load_merged_sources};

    // Undefined variable render error includes source name and line number.
    let err = load_merged_sources(
        [ConfigSource::string(
            "defaults.toml",
            "\n# comment\n[app]\nval = \"{{ vars.missing }}\"\n",
        )],
        &mut Engine::new(),
        &Context::new(),
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("defaults.toml"), "{msg}");
    assert!(msg.contains(":4:"), "{msg}");

    // Invalid [vars] TOML includes source name.
    let err = load_merged_sources(
        [ConfigSource::string("broken-vars.toml", "[vars]\na = [\n")],
        &mut Engine::new(),
        &Context::new(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("broken-vars.toml"), "{err}");

    // Conflicting include forms return IncludeConflict naming the string source.
    let err = load_merged_sources(
        [ConfigSource::string(
            "conflict.toml",
            "include = []\n[teravars]\ninclude = []\n",
        )],
        &mut Engine::new(),
        &Context::new(),
    )
    .unwrap_err();
    assert!(
        matches!(&err, Error::IncludeConflict { path } if path == std::path::Path::new("conflict.toml")),
        "{err:?}"
    );

    // Ambiguous include ownership protection applies identically to string sources.
    let mut ctx = Context::new();
    ctx.insert("show_app", &false);
    let err = load_merged_sources(
        [ConfigSource::string(
            "ambiguous.toml",
            "{% if show_app %}\n[app]\n{% endif %}\ninclude = [\"missing.toml\"]\n",
        )],
        &mut Engine::new(),
        &ctx,
    )
    .unwrap_err();
    assert!(
        matches!(&err, Error::Extract(message)
            if message.contains("ambiguous.toml `include`")
                && message.contains("owning table depends on Tera rendering")),
        "{err}"
    );
}

#[test]
fn duplicate_detection_separates_file_and_string_namespaces() {
    use teravars::{ConfigSource, load_merged_sources};

    // Duplicate string source names are rejected.
    let err = load_merged_sources(
        [
            ConfigSource::string("defaults.toml", "a = 1\n"),
            ConfigSource::string("defaults.toml", "b = 2\n"),
        ],
        &mut Engine::new(),
        &Context::new(),
    )
    .unwrap_err();
    assert!(
        matches!(&err, Error::IncludeCycle { path } if path == std::path::Path::new("defaults.toml")),
        "{err:?}"
    );

    // A string source whose name happens to equal an on-disk path string does
    // not collide with loading that file, because string names and canonical
    // file paths occupy separate namespaces.
    let dir = TempDir::new().unwrap();
    let file_path = write(&dir, "config.toml", "from_file = true\n");
    let merged = load_merged_sources(
        [
            ConfigSource::string(
                file_path.to_string_lossy().into_owned(),
                "from_str = true\n",
            ),
            ConfigSource::path(&file_path),
        ],
        &mut Engine::new(),
        &Context::new(),
    )
    .unwrap();
    assert_eq!(merged.config["from_str"].as_bool(), Some(true));
    assert_eq!(merged.config["from_file"].as_bool(), Some(true));
}

#[test]
fn rendered_root_direct_deserialization_preserves_line_and_column_diagnostics() {
    use serde::Deserialize;
    use teravars::{ConfigSource, load_merged_rendered};

    #[derive(Debug, Deserialize)]
    struct AppConfig {
        server: Server,
    }
    #[derive(Debug, Deserialize)]
    struct Server {
        #[allow(dead_code)]
        host: String,
        #[allow(dead_code)]
        port: u16,
    }

    // Valid single-root config deserializes directly from rendered_root().
    let good = ConfigSource::string(
        "app.toml",
        r#"# header comment
[vars]
host = "localhost"

[server]
host = "{{ vars.host }}"
port = 8080
"#,
    );
    let rendered = load_merged_rendered([good], &mut Engine::new(), &Context::new()).unwrap();
    let root_text = rendered.rendered_root().unwrap();
    let cfg: AppConfig = toml::from_str(root_text).unwrap();
    assert_eq!(cfg.server.host, "localhost");
    assert_eq!(cfg.server.port, 8080);

    // Mistyped field on line 7 keeps exact line/column diagnostics in toml::from_str.
    let bad = ConfigSource::string(
        "app.toml",
        "# line 1\n[vars]\nhost = \"localhost\"\n\n[server]\nhost = \"{{ vars.host }}\"\nport = \"not-a-port\"\n",
    );
    let rendered = load_merged_rendered([bad], &mut Engine::new(), &Context::new()).unwrap();
    let root_text = rendered.rendered_root().unwrap();
    let err = toml::from_str::<AppConfig>(root_text).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("line 7"), "expected line 7 diagnostic: {msg}");
    assert!(err.span().is_some(), "expected span in error: {msg}");

    // Multi-root load yields None for single-root accessors and exposes all roots via roots().
    let multi = load_merged_rendered(
        [
            ConfigSource::string("base.toml", "a = 1\n"),
            ConfigSource::string("override.toml", "b = 2\n"),
        ],
        &mut Engine::new(),
        &Context::new(),
    )
    .unwrap();
    assert!(multi.root().is_none());
    assert!(multi.rendered_root().is_none());
    assert_eq!(multi.roots().count(), 2);
}

#[test]
fn rendered_text_does_not_rerender_and_matches_path_api() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use teravars::{
        ConfigSource, Kwargs, SourceId, State, TeraResult, Value, load_merged_rendered,
    };

    let dir = TempDir::new().unwrap();
    let base = write(
        &dir,
        "base.toml",
        "[vars]\nval = \"from-base\"\n[base]\ntick = \"{{ tick() }}\"\n",
    );
    let main = write(
        &dir,
        "config.toml",
        "include = [\"base.toml\"]\n[vars]\nval = \"from-main\"\n[main]\ntick = \"{{ tick() }}\"\nseen = \"{{ vars.val }}\"\n",
    );

    let counter = Arc::new(AtomicUsize::new(0));
    let counter_fn = Arc::clone(&counter);
    let mut engine = Engine::new();
    engine.register_function(
        "tick",
        move |_kwargs: Kwargs, _state: &State| -> TeraResult<Value> {
            let n = counter_fn.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(Value::from(format!("call-{n}")))
        },
    );

    let rendered =
        load_merged_rendered([ConfigSource::path(&main)], &mut engine, &Context::new()).unwrap();

    // Exactly two calls total (one for base.toml, one for config.toml) — proving
    // no second render happens to produce rendered text.
    assert_eq!(counter.load(Ordering::SeqCst), 2);
    assert_eq!(rendered.sources.len(), 2);
    assert_eq!(rendered.sources[0].id, SourceId::Path(base));
    assert!(!rendered.sources[0].is_root);
    assert_eq!(rendered.sources[1].id, SourceId::Path(main.clone()));
    assert!(rendered.sources[1].is_root);
    assert!(rendered.sources[0].rendered.contains(r#"tick = "call-1""#));
    assert!(
        rendered.sources[0]
            .rendered
            .contains(r#"val = "from-base""#)
    );
    assert!(rendered.sources[1].rendered.contains(r#"tick = "call-2""#));
    assert!(
        rendered.sources[1]
            .rendered
            .contains(r#"seen = "from-main""#)
    );
    // Earlier source's rendered text is not retroactively rewritten by later overrides.
    assert!(!rendered.sources[0].rendered.contains("from-main"));

    // Existing path API produces the exact same merged tables (modulo tick counter).
    let mut baseline_engine = Engine::new();
    let baseline_counter = Arc::new(AtomicUsize::new(0));
    let baseline_fn = Arc::clone(&baseline_counter);
    baseline_engine.register_function(
        "tick",
        move |_kwargs: Kwargs, _state: &State| -> TeraResult<Value> {
            let n = baseline_fn.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(Value::from(format!("call-{n}")))
        },
    );
    let baseline = load_merged([&main], &mut baseline_engine, &Context::new()).unwrap();
    assert_eq!(rendered.merged.vars, baseline.vars);
    assert_eq!(rendered.merged.config, baseline.config);

    // Path iterators from discover_config_files work directly with load_merged_rendered.
    let discovered = discover_config_files(dir.path()).unwrap();
    let mut disc_engine = Engine::new();
    disc_engine.register_function(
        "tick",
        |_kwargs: Kwargs, _state: &State| -> TeraResult<Value> { Ok(Value::from("ok")) },
    );
    let disc_rendered =
        load_merged_rendered(discovered.iter(), &mut disc_engine, &Context::new()).unwrap();
    assert_eq!(disc_rendered.sources.len(), 2);
    assert!(disc_rendered.rendered_root().is_some());
}

#[test]
fn rendered_output_interoperates_with_deferred_runtime_expressions() {
    use teravars::{ConfigSource, LoadOptions, load_merged_rendered_with};

    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "base.toml",
        r#"
[vars]
prefix = "run"
runtime = '{{ group }}'
quoted_runtime = "{{ file_path | replace(from='\\', to='/') }}"
alias = '{{ vars.runtime }}'
"#,
    );

    let root_content = concat!(
        "include = [\"base.toml\"]\n",
        "[commands]\n",
        "direct_basic = \"{{ vars.prefix }} {{ cap[0] | replace(from=\"a\", to=\"#\") }}\"\n",
        "direct_literal = '{{ vars.prefix }} {{ group }}'\n",
        "from_var_basic = \"{{ vars.alias }}\"\n",
        "from_var_single_quote_in_literal = '{{ vars.quoted_runtime }}'\n",
        "mixed_emitted_and_var = \"{{ group }} :: {{ vars.alias }}\"\n",
        "ml_literal = '''\n",
        "first {{ vars.prefix }}\n",
        "second {{ group }}\n",
        "'''\n",
        "ml_switch = '''\n",
        "{{ vars.quoted_runtime }}\n",
        "'''\n",
        "after = \"line-check\"\n",
    );

    let options = LoadOptions::new()
        .defer("group")
        .defer("file_path")
        .defer("cap");

    let mut engine = Engine::new();
    let rendered = load_merged_rendered_with(
        [ConfigSource::string_with_base_dir(
            "root.toml",
            root_content,
            dir.path(),
        )],
        &mut engine,
        &Context::new(),
        &options,
    )
    .unwrap();

    // No internal deferred marker characters may leak into any rendered source.
    for source in &rendered.sources {
        assert!(
            !source
                .rendered
                .chars()
                .any(|c| matches!(c, '\u{e000}' | '\u{e001}' | '\u{e002}')),
            "marker leaked in {}: {:?}",
            source.id,
            source.rendered
        );
    }

    let root_text = rendered.rendered_root().unwrap();
    // Line count must stay identical to the original source.
    assert_eq!(
        root_text.lines().count(),
        root_content.lines().count(),
        "line structure changed:\n{root_text}"
    );

    // Restored rendered text must be valid TOML and decode to the exact same
    // string values as the merged table.
    let reparsed: toml::Table = toml::from_str(root_text)
        .unwrap_or_else(|e| panic!("restored rendered text is invalid TOML: {e}\n{root_text}"));
    let reparsed_cmds = reparsed["commands"].as_table().unwrap();
    let merged_cmds = rendered.merged.config["commands"].as_table().unwrap();
    assert_eq!(reparsed_cmds, merged_cmds);

    // Verify second render works directly on values parsed from rendered_root().
    let mut rt_ctx = Context::new();
    rt_ctx.insert("vars", &rendered.merged.vars);
    rt_ctx.insert("group", &"dev");
    rt_ctx.insert("file_path", &r"C:\a\b.txt");
    rt_ctx.insert("cap", &["alpha"]);

    assert_eq!(
        engine
            .render(reparsed_cmds["direct_basic"].as_str().unwrap(), &rt_ctx)
            .unwrap(),
        "run #lph#"
    );
    assert_eq!(
        engine
            .render(
                reparsed_cmds["from_var_single_quote_in_literal"]
                    .as_str()
                    .unwrap(),
                &rt_ctx
            )
            .unwrap(),
        "C:/a/b.txt"
    );
    assert_eq!(
        engine
            .render(
                reparsed_cmds["mixed_emitted_and_var"].as_str().unwrap(),
                &rt_ctx
            )
            .unwrap(),
        "dev :: dev"
    );
    assert_eq!(
        engine
            .render(reparsed_cmds["ml_literal"].as_str().unwrap(), &rt_ctx)
            .unwrap(),
        "first run\nsecond dev\n"
    );
}
