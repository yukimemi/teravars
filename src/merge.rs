//! Multi-file config merge — yui-style deep recursive merge with per-file
//! Tera rendering and an `include = [...]` directive that lets a config file
//! pull in other files before it is processed.

mod deferred;
mod include;

use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use tera::Context;
use toml::{Table, Value};

use crate::Result;
use crate::comments::strip_toml_comments;
use crate::engine::Engine;
use crate::error::Error;
use crate::vars::extract_vars;
use deferred::Deferred;
pub use deferred::LoadOptions;
use include::extract_include_paths;

/// Identifies a loaded configuration source in diagnostics and
/// [`RenderedSource`] entries.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SourceId {
    /// A configuration file loaded from the filesystem.
    Path(PathBuf),
    /// An in-memory string configuration source identified by name.
    Name(String),
}

impl SourceId {
    /// Return the filesystem path if this source was loaded from disk.
    pub fn as_path(&self) -> Option<&Path> {
        match self {
            Self::Path(path) => Some(path),
            Self::Name(_) => None,
        }
    }

    /// Return the logical name if this source was loaded from a string.
    pub fn as_name(&self) -> Option<&str> {
        match self {
            Self::Path(_) => None,
            Self::Name(name) => Some(name),
        }
    }

    pub(super) fn to_diagnostic_path(&self) -> PathBuf {
        match self {
            Self::Path(path) => path.clone(),
            Self::Name(name) => PathBuf::from(name),
        }
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(path) => write!(f, "{}", path.display()),
            Self::Name(name) => f.write_str(name),
        }
    }
}

impl From<PathBuf> for SourceId {
    fn from(path: PathBuf) -> Self {
        Self::Path(path)
    }
}

impl From<&Path> for SourceId {
    fn from(path: &Path) -> Self {
        Self::Path(path.to_path_buf())
    }
}

impl From<&PathBuf> for SourceId {
    fn from(path: &PathBuf) -> Self {
        Self::Path(path.clone())
    }
}

/// Configuration input for [`load_merged_sources`] and [`load_merged_rendered`].
///
/// Explicitly distinguishes filesystem paths from named in-memory strings so
/// embedded defaults (e.g. `include_str!`) and on-disk config files can be
/// combined through the same loader.
/// Borrowed inputs avoid copying paths and embedded text; owned inputs are
/// also accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigSource<'a> {
    /// Load a configuration file from the filesystem.
    Path(Cow<'a, Path>),
    /// Load an in-memory TOML+Tera document identified by `name`.
    ///
    /// `name` is used only for diagnostics and duplicate detection and is never
    /// treated as a filesystem path. Relative `include` paths require an
    /// explicit absolute `base_dir`; absolute `include` paths work even when
    /// `base_dir` is `None`.
    String {
        name: Cow<'a, str>,
        content: Cow<'a, str>,
        base_dir: Option<Cow<'a, Path>>,
    },
}

impl<'a> ConfigSource<'a> {
    /// Construct a filesystem path source.
    pub fn path(path: impl Into<Cow<'a, Path>>) -> Self {
        Self::Path(path.into())
    }

    /// Construct a named in-memory string source with no relative include base
    /// directory.
    ///
    /// Absolute `include` paths are still allowed, while relative `include`
    /// paths fail with a named error instead of silently resolving against the
    /// process working directory.
    pub fn string(name: impl Into<Cow<'a, str>>, content: impl Into<Cow<'a, str>>) -> Self {
        Self::String {
            name: name.into(),
            content: content.into(),
            base_dir: None,
        }
    }

    /// Construct a named in-memory string source with an explicit absolute
    /// `base_dir` for resolving relative `include` paths.
    pub fn string_with_base_dir(
        name: impl Into<Cow<'a, str>>,
        content: impl Into<Cow<'a, str>>,
        base_dir: impl Into<Cow<'a, Path>>,
    ) -> Self {
        Self::String {
            name: name.into(),
            content: content.into(),
            base_dir: Some(base_dir.into()),
        }
    }

    fn as_input(&self) -> SourceInput<'_> {
        match self {
            Self::Path(path) => SourceInput::Path(path.as_ref()),
            Self::String {
                name,
                content,
                base_dir,
            } => SourceInput::String {
                name: name.as_ref(),
                content: content.as_ref(),
                base_dir: base_dir.as_deref(),
            },
        }
    }
}

impl From<PathBuf> for ConfigSource<'_> {
    fn from(path: PathBuf) -> Self {
        Self::Path(Cow::Owned(path))
    }
}

impl<'a> From<&'a Path> for ConfigSource<'a> {
    fn from(path: &'a Path) -> Self {
        Self::Path(Cow::Borrowed(path))
    }
}

impl<'a> From<&'a PathBuf> for ConfigSource<'a> {
    fn from(path: &'a PathBuf) -> Self {
        Self::from(path.as_path())
    }
}

#[derive(Debug, Clone, Default)]
pub struct MergedConfig {
    pub vars: Table,
    pub config: Table,
}

/// Actual rendered TOML text produced for a single source during loading.
///
/// The text is captured directly from the single Tera render pass after
/// validation — never re-rendered and never re-serialized from the merged
/// table.
///
/// Contract details:
/// - TOML `#` comments are already stripped (preserving line numbers).
/// - Values reflect what was in scope when this source rendered (including
///   vars from earlier sources and includes); later sources' overrides are not
///   retroactively reflected.
/// - Unlike [`MergedConfig::config`], any `include` or `[teravars]` directive
///   present in the source remains in `rendered`.
/// - When [`LoadOptions`] deferred runtime expressions are active, deferred
///   tags are restored as valid TOML string literals while preserving line
///   structure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedSource {
    pub id: SourceId,
    pub is_root: bool,
    pub rendered: String,
}

/// Result of [`load_merged_rendered`] / [`load_merged_rendered_with`].
///
/// Contains the merged configuration tables together with each loaded source's
/// rendered TOML text in load order (included files before their declaring
/// source).
#[derive(Debug, Clone, Default)]
pub struct RenderedConfig {
    pub merged: MergedConfig,
    pub sources: Vec<RenderedSource>,
}

impl RenderedConfig {
    /// Return the single root source if exactly one root was loaded.
    ///
    /// Returns `None` when zero or multiple root sources were passed.
    pub fn root(&self) -> Option<&RenderedSource> {
        let mut roots = self.roots();
        let first = roots.next()?;
        roots.next().is_none().then_some(first)
    }

    /// Return the actual rendered text of the single root source, if exactly
    /// one root was loaded.
    ///
    /// When a single root has no `include` or `[teravars]` directives, this
    /// text can be passed directly to `toml::from_str` for full line/column
    /// deserialization diagnostics.
    /// Returning `Some` does not guarantee the absence of these directives;
    /// callers that permit them must deserialize from [`MergedConfig::config`]
    /// instead to retain included settings and omit reserved directives.
    pub fn rendered_root(&self) -> Option<&str> {
        self.root().map(|source| source.rendered.as_str())
    }

    /// Iterate over all root sources (excluding transitive includes) in load
    /// order.
    pub fn roots(&self) -> impl Iterator<Item = &RenderedSource> {
        self.sources.iter().filter(|source| source.is_root)
    }
}

pub fn load_merged<P: AsRef<Path>>(
    paths: impl IntoIterator<Item = P>,
    engine: &mut Engine,
    extra_ctx: &Context,
) -> Result<MergedConfig> {
    load_merged_with(paths, engine, extra_ctx, &LoadOptions::default())
}

/// Load configuration with opt-in preservation of runtime-only expressions.
///
/// Deferred output tags survive verbatim in string values. Control statements,
/// include paths, keys and table headers cannot reference runtime-only names.
/// Mixed expressions are preserved whole, so the second render must supply
/// their other dependencies as well.
/// String values carrying deferred tags may be copied unchanged through
/// static variable paths, but cannot be filtered or used in control flow.
pub fn load_merged_with<P: AsRef<Path>>(
    paths: impl IntoIterator<Item = P>,
    engine: &mut Engine,
    extra_ctx: &Context,
    options: &LoadOptions,
) -> Result<MergedConfig> {
    load_merged_sources_with(
        paths
            .into_iter()
            .map(|p| ConfigSource::path(p.as_ref().to_path_buf())),
        engine,
        extra_ctx,
        options,
    )
}

/// Load and merge configuration from any mix of filesystem paths and named
/// in-memory strings.
///
/// Uses the exact same include-first, per-source Tera rendering and deep-merge
/// pipeline as [`load_merged`].
///
/// # Example: embedded defaults (`include_str!`)
///
/// ```
/// use teravars::{ConfigSource, Context, Engine, load_merged_sources};
///
/// const EMBEDDED_DEFAULTS: &str = r#"
/// [vars]
/// editor = "nvim"
///
/// [ui]
/// command = "{{ vars.editor }}"
/// "#;
///
/// let mut engine = Engine::new();
/// let merged = load_merged_sources(
///     [ConfigSource::string("<embedded defaults>", EMBEDDED_DEFAULTS)],
///     &mut engine,
///     &Context::new(),
/// )
/// .unwrap();
///
/// assert_eq!(merged.config["ui"]["command"].as_str(), Some("nvim"));
/// ```
pub fn load_merged_sources<'a, S: Into<ConfigSource<'a>>>(
    sources: impl IntoIterator<Item = S>,
    engine: &mut Engine,
    extra_ctx: &Context,
) -> Result<MergedConfig> {
    load_merged_sources_with(sources, engine, extra_ctx, &LoadOptions::default())
}

/// Like [`load_merged_sources`], with opt-in [`LoadOptions`] for preserving
/// runtime-only Tera expressions.
pub fn load_merged_sources_with<'a, S: Into<ConfigSource<'a>>>(
    sources: impl IntoIterator<Item = S>,
    engine: &mut Engine,
    extra_ctx: &Context,
    options: &LoadOptions,
) -> Result<MergedConfig> {
    load_internal(sources, engine, extra_ctx, options, None)
}

/// Like [`load_merged_sources`], additionally returning the actual rendered
/// TOML text for each loaded source (and [`RenderedConfig::rendered_root`] for
/// single-root loads) without re-rendering or re-serializing the merged table.
///
/// # Example: `config show --rendered` and direct deserialization
///
/// ```
/// use teravars::{ConfigSource, Context, Engine, load_merged_rendered};
///
/// let mut engine = Engine::new();
/// let rendered = load_merged_rendered(
///     [ConfigSource::string(
///         "defaults.toml",
///         "[vars]\nport = 8080\n[server]\nport = {{ vars.port }}\n",
///     )],
///     &mut engine,
///     &Context::new(),
/// )
/// .unwrap();
///
/// // Single root with no `include`: deserialize directly for line/column diagnostics.
/// let root_text = rendered.rendered_root().unwrap();
/// let parsed: toml::Table = toml::from_str(root_text).unwrap();
/// assert_eq!(parsed["server"]["port"].as_integer(), Some(8080));
///
/// // Or inspect every rendered source in load order (e.g. `config show --rendered`).
/// assert_eq!(rendered.sources.len(), 1);
/// assert_eq!(rendered.sources[0].id.as_name(), Some("defaults.toml"));
/// ```
pub fn load_merged_rendered<'a, S: Into<ConfigSource<'a>>>(
    sources: impl IntoIterator<Item = S>,
    engine: &mut Engine,
    extra_ctx: &Context,
) -> Result<RenderedConfig> {
    load_merged_rendered_with(sources, engine, extra_ctx, &LoadOptions::default())
}

/// Like [`load_merged_rendered`], with opt-in [`LoadOptions`] for preserving
/// runtime-only Tera expressions in both the merged tables and the returned
/// rendered text.
pub fn load_merged_rendered_with<'a, S: Into<ConfigSource<'a>>>(
    sources: impl IntoIterator<Item = S>,
    engine: &mut Engine,
    extra_ctx: &Context,
    options: &LoadOptions,
) -> Result<RenderedConfig> {
    let mut rendered_sources = Vec::new();
    let merged = load_internal(
        sources,
        engine,
        extra_ctx,
        options,
        Some(&mut rendered_sources),
    )?;
    Ok(RenderedConfig {
        merged,
        sources: rendered_sources,
    })
}

enum SourceInput<'a> {
    Path(&'a Path),
    String {
        name: &'a str,
        content: &'a str,
        base_dir: Option<&'a Path>,
    },
}

#[derive(Default)]
struct Visited {
    paths: HashSet<PathBuf>,
    names: HashSet<String>,
}

struct LoadState<'a> {
    vars: Table,
    config: Table,
    visited: Visited,
    rendered_sources: Option<&'a mut Vec<RenderedSource>>,
}

fn load_internal<'a, S: Into<ConfigSource<'a>>>(
    sources: impl IntoIterator<Item = S>,
    engine: &mut Engine,
    extra_ctx: &Context,
    options: &LoadOptions,
    rendered_sources: Option<&mut Vec<RenderedSource>>,
) -> Result<MergedConfig> {
    let mut deferred = Deferred::new(options, engine)?;
    let mut state = LoadState {
        vars: Table::new(),
        config: Table::new(),
        visited: Visited::default(),
        rendered_sources,
    };

    for source in sources {
        let source = source.into();
        load_source_recursive(
            source.as_input(),
            true,
            engine,
            extra_ctx,
            &mut deferred,
            &mut state,
        )?;
    }

    if !state.vars.is_empty() {
        let merged_vars_id = SourceId::Name("<merged vars>".into());
        deferred.resolve(&mut state.vars, engine, extra_ctx, &merged_vars_id, false, None)?;
    }
    deferred.restore(&mut state.vars)?;
    deferred.restore(&mut state.config)?;

    Ok(MergedConfig {
        vars: state.vars,
        config: state.config,
    })
}

fn load_source_recursive(
    input: SourceInput<'_>,
    is_root: bool,
    engine: &mut Engine,
    extra_ctx: &Context,
    deferred: &mut Deferred<'_>,
    state: &mut LoadState<'_>,
) -> Result<()> {
    let (source_id, raw): (SourceId, Cow<'_, str>) = match input {
        SourceInput::Path(path) => {
            let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
            if !state.visited.paths.insert(canonical.clone()) {
                return Err(Error::IncludeCycle { path: canonical });
            }
            let raw = std::fs::read_to_string(path).map_err(|e| {
                Error::Io(std::io::Error::new(
                    e.kind(),
                    format!("{}: {e}", path.display()),
                ))
            })?;
            (SourceId::Path(path.to_path_buf()), Cow::Owned(raw))
        }
        SourceInput::String {
            name,
            content,
            base_dir,
        } => {
            if name.trim().is_empty() {
                return Err(Error::Extract(
                    "string config source name must not be empty".into(),
                ));
            }
            if let Some(dir) = base_dir {
                if dir.as_os_str().is_empty() {
                    return Err(Error::Extract(format!(
                        "{name}: base_dir must be an absolute path"
                    )));
                }
                if !dir.is_absolute() {
                    return Err(Error::Extract(format!(
                        "{name}: base_dir `{}` must be an absolute path",
                        dir.display()
                    )));
                }
            }
            if !state.visited.names.insert(name.to_owned()) {
                return Err(Error::IncludeCycle {
                    path: PathBuf::from(name),
                });
            }
            (SourceId::Name(name.to_owned()), Cow::Borrowed(content))
        }
    };

    // Comment text never reaches Tera: a commented-out sample line or a note
    // quoting teravars syntax would otherwise render (and typically fail the
    // whole load). Done once here so include detection, [vars] extraction
    // and the render below all see the same comment-free text; line numbers
    // are preserved, so error locations still line up with the source.
    let raw = strip_toml_comments(&raw);

    let include_paths = extract_include_paths(&raw, &source_id)?;
    for raw_inc in &include_paths.paths {
        if deferred.active() {
            deferred.protect(raw_inc, &source_id, false)?;
        }
        let rendered_inc = engine
            .render(raw_inc, extra_ctx)
            .map_err(|e| Error::Render(format!("{source_id} include: {}", error_msg(&e))))?;
        let inc_path = resolve_include_path(&input, &source_id, &rendered_inc)?;
        load_source_recursive(
            SourceInput::Path(&inc_path),
            false,
            engine,
            extra_ctx,
            deferred,
            state,
        )?;
    }

    let protected;
    let mut classified = None;
    let raw = if deferred.active() {
        let (text, annotated) = deferred.protect_classified(&raw, &source_id)?;
        protected = text;
        classified = annotated.map(|vars| deferred::classified_vars(&state.vars, vars));
        protected.as_str()
    } else {
        raw.as_ref()
    };
    let file_vars = extract_vars(raw).map_err(|e| match e {
        Error::Extract(msg) => Error::Extract(format!("{source_id}: {msg}")),
        other => other,
    })?;

    let mut resolution_vars = state.vars.clone();
    deep_merge(&mut resolution_vars, file_vars);
    // Best-effort per-file resolution: a [vars] entry in this file may
    // legitimately reference vars introduced by a later file in the merge
    // chain, so we tolerate `ResolveNotConverged` / render errors here.
    // The authoritative pass runs at the end of `load_internal` once every
    // source has contributed; genuine render failures resurface from the
    // `engine.render` call below with a more useful location.
    deferred.resolve(
        &mut resolution_vars,
        engine,
        extra_ctx,
        &source_id,
        true,
        classified.as_ref(),
    )?;

    let mut ctx = extra_ctx.clone();
    ctx.insert("vars", &resolution_vars);
    deferred.validate_indirect(raw, &source_id, &ctx, classified.as_ref())?;

    let (rendered, emissions) = deferred
        .render(raw, engine, &ctx)
        .map_err(|e| include_paths.render_error(&source_id, error_msg(&e)))?;

    let mut parsed: Table = rendered
        .parse()
        .map_err(|e: toml::de::Error| Error::Extract(format!("{source_id}: {e}")))?;

    include_paths.check_rendered(&parsed, &source_id)?;
    deferred
        .normalize(&mut parsed, emissions.as_ref())
        .map_err(|e| Error::Render(format!("{source_id}: {}", error_msg(&e))))?;
    deferred.check_keys(&parsed, &source_id)?;

    if let Some(sources) = &mut state.rendered_sources {
        let rendered = deferred
            .restore_rendered(rendered, emissions.as_ref().map(|e| e.prefix.as_str()))
            .map_err(|e| Error::Render(format!("{source_id}: {}", error_msg(&e))))?;
        sources.push(RenderedSource {
            id: source_id,
            is_root,
            rendered,
        });
    }

    parsed.remove("include");
    parsed.remove("teravars");

    if let Some(Value::Table(rendered_vars)) = parsed.get("vars").cloned() {
        deep_merge(&mut state.vars, rendered_vars);
    }

    deep_merge(&mut state.config, parsed);

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

fn resolve_include_path(
    input: &SourceInput<'_>,
    source_id: &SourceId,
    rendered_include: &str,
) -> Result<PathBuf> {
    let target = Path::new(rendered_include);
    if target.is_absolute() {
        return Ok(target.to_path_buf());
    }
    match input {
        SourceInput::Path(path) => Ok(resolve_relative(path, target)),
        SourceInput::String { base_dir, .. } => match base_dir {
            Some(dir) => {
                let joined = dir.join(target);
                if !joined.is_absolute() {
                    return Err(Error::Extract(format!(
                        "{source_id} `include`: path `{rendered_include}` does not resolve to an absolute path with base_dir `{}`",
                        dir.display()
                    )));
                }
                Ok(joined)
            }
            None => Err(Error::Extract(format!(
                "{source_id} `include`: relative path `{rendered_include}` requires an explicit absolute base_dir"
            ))),
        },
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
        include::extract_include_paths(text, &SourceId::from(path)).map(|includes| includes.paths)
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
