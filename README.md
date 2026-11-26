# teravars

<p align="center">
  <img src="https://raw.githubusercontent.com/yukimemi/teravars/master/assets/logo_with_icon.svg" width="400" alt="teravars logo">
</p>

<p align="center">
  <strong>[Tera] templating + smart `[vars]` handling for self-rendering TOML configs.</strong>
</p>

<p align="center">
  Extracted from the duplicated patterns in <a href="https://github.com/yukimemi/shun">shun</a> / <a href="https://github.com/yukimemi/rvpm">rvpm</a> / <a href="https://github.com/yukimemi/todoke">todoke</a> / <a href="https://github.com/yukimemi/yui">yui</a>.
</p>

---

**Status: 0.2.2 — core + multi-file merge + `include` directive + `hash` / `port_offset` filters + TOML-comment stripping shipped. [renri](https://github.com/yukimemi/renri) and [magi](https://github.com/yukimemi/magi) consume it today; migration of the remaining sibling tools is the next step, see [ROADMAP.md](https://github.com/yukimemi/teravars/blob/main/ROADMAP.md).**

## Quickstart

```rust
use teravars::{Context, Engine, extract_vars, resolve, system_context};

let raw = std::fs::read_to_string("config.toml")?;
let mut engine = Engine::new();              // Tera + standard helpers

let mut vars = extract_vars(&raw)?;          // text-based [vars] carve-out
resolve(&mut vars, &mut engine)?;            // iterate cross-refs to fixpoint

let mut ctx: Context = system_context();     // system.os/arch/user/host
ctx.insert("vars", &vars);

let rendered = engine.render_toml(&raw, &ctx)?;   // comments never rendered
let cfg: MyConfig = toml::from_str(&rendered)?;
```

`resolve` mutates `vars` in place. On non-convergence it returns
`Err(Error::ResolveNotConverged { .. })` while leaving `vars` in its
last partially-resolved state — callers that prefer resilience over
strictness can `if let Err(_)` and continue with what's there.

## Comments are inert

A config file's `#` comments are stripped before Tera ever sees them, so a
comment can say anything — including teravars syntax — without affecting the
load:

```toml
# sample: port = "{{ vars.not_defined_yet }}"   # would be a render error
# to disable the block below: {% if false %}    # would be "unexpected end of input"

[vars] # app vars
name = "app"
# disabled = "{{ vars.gone }}"                  # not a var

[server]
url = "https://example.com/#anchor"             # `#` in a string is data
```

This is automatic in [`load_merged`](#multi-file-merge-feature-merge) and in
`Engine::render_toml`. Plain `Engine::render` deliberately does **not** strip
anything — it renders arbitrary text, where `#` carries no meaning. Reach for
`strip_toml_comments` directly if you drive the render yourself.

The scanner is TOML-aware, so a `#` that is not a comment survives: inside
basic / literal strings, inside multi-line `"""` / `'''` strings, and inside
Tera delimiters (`{{ … }}`, `{% … %}`, `{# … #}`). Only the bytes from `#` to
end of line are dropped, so error messages keep pointing at the original line
numbers.

## Multi-file merge (feature `merge`)

```rust
use teravars::{Context, Engine, discover_config_files, load_merged, system_context};

let mut engine = Engine::new();
let files = discover_config_files("/etc/myapp")?;   // config.toml, config.*.toml, config.local.toml
let merged = load_merged(files.iter(), &mut engine, &system_context())?;

let cfg: MyConfig = merged.config.try_into()?;       // deep-merged, rendered, vars-resolved
```

`load_merged` does **per-file Tera rendering** with vars accumulated
from earlier files in scope, then **deep-recursively merges** the
parsed result. Tables merge, arrays append, scalars are overwritten
by later files. Missing files are an error — filter the path list
beforehand if you want skip-on-missing.

`discover_config_files(dir)` returns the file set in the canonical
order: `config.toml` first, alphabetical `config.*.toml` next,
`config.local.toml` last (so the local override always wins).

### `include` directive

Any TOML file processed by `load_merged` may pull in other files:

```toml
# /etc/myapp/config.toml
include = [
  "common.toml",                            # relative to /etc/myapp/
  "{{ system.host }}.toml",                 # Tera-rendered before path resolution
  "{{ system.cwd }}/dev-overrides.toml",    # cwd-relative if you want it
]

[vars]
api_url = "https://api.{{ vars.global_host }}"   # global_host comes from common.toml
```

Semantics:

- Static includes are discovered and loaded **before** rendering the
  declaring file, even if a bare Tera expression elsewhere makes the
  source invalid TOML before rendering. Included vars are available to
  that render, and the declaring file overrides what it includes.
- Declare `include` as a TOML array of strings outside Tera control
  blocks. An invalid declaration returns an error naming the file and
  directive rather than loading only some paths or silently skipping it.
  Statically unreachable `if` arms with literal boolean conditions do not
  affect table ownership. Static headers in guaranteed `if` arms (literal
  `true` after only literal `false` conditions, or the corresponding
  `else`) establish ownership, provided all enclosing blocks are also
  guaranteed `if` arms and a newline separates the header from the
  preceding Tera tag without right whitespace trimming. A preceding
  Tera output expression outside a TOML string, including in an assignment
  value, prevents conditional headers from establishing ownership, because
  its output may open a multiline string. A potentially
  active header, a templated table name, or a Tera expression at the start
  of a statement may make ownership ambiguous until the next definite
  static header. If rendering produces an ambiguous include directive that
  was not loaded, loading fails instead of dropping it.
  If rendering fails before ownership can be checked, the error names the
  ambiguous directive and retains the render failure.
  This also affects previously accepted configurations such as
  `ready = {{ vars.ready }}` followed by `{% if true %}` wrapping a
  `[teravars]` header and then a static `include` assignment. The include
  can no longer supply `vars.ready` in that arrangement. Remove the
  redundant conditional around `[teravars]` to retain the original
  assignment order, or put the conditional header and its include before
  the output expression (in a separate application table).
- Paths are **rendered through Tera** with `system.*` and any
  caller-supplied context in scope, but **not** with `vars` (vars
  haven't accumulated yet at the point includes resolve).
- Relative paths resolve against the **directory of the including
  file**. Use `{{ system.cwd }}` when you want process-cwd-relative.
- Includes can include further files; the loader detects cycles and
  returns `Error::IncludeCycle`.
- The directive itself never appears in the merged result — both
  root `include` and the entire `[teravars]` namespace are stripped
  before merging.
- If both root `include = [...]` and `[teravars] include = [...]`
  are present in the same file, including empty arrays, that's
  `Error::IncludeConflict`.
  Pick one form per file.

Use `[teravars] include = [...]` to keep the directive in the reserved
`teravars` namespace. An `include` key in any other table is ordinary
application config.

### Runtime-only expressions (todoke / shun)

Use `LoadOptions` with `load_merged_with` when some variables are only
available at execution time. For example, todoke can load its configuration
with:

```rust
use teravars::{LoadOptions, load_merged_with};

let options = LoadOptions::new()
    .defer("group")
    .defer("file_path")
    .defer("cap")
    .defer("passthrough");
let merged = load_merged_with(paths, &mut engine, &load_context, &options)?;
```

For shun, use:

```rust
let options = LoadOptions::new()
    .defer("args")
    .defer("args_list")
    .defer_prefix("file_");
```

Names are matched on root variable references (not attributes, literals,
or unrelated names). Prefix matching explicitly opts in every matching
root name. The usual `load_merged` remains strict and does not defer any
names.

For example, the following string values can be loaded without runtime
values in the context:

```toml
[command]
todoke = '{{ vars.program }} {{ group }} {{ file_path }} {{ cap[0] }} {{ passthrough | join(sep=" ") }}'
shun = '{{ args }} {{ args_list | join(sep=" ") }} {{ file_name }}'
mixed = '{{ group ~ ":" ~ vars.program }}'
```

`vars.program` in the first line renders at load time (including from an
included file). Any output tag containing a free runtime-only reference is
preserved **whole**, including filters, indexing and whitespace-control
delimiters. The `mixed` tag above is not partially evaluated. At execution
time, call `Engine::render` on the loaded string with actual runtime
values and any other context used by its deferred tags, including `vars`
from `merged.vars` and `system` if referenced.

Supported positions are TOML string values, including strings in arrays,
inline tables and `[vars]`. Runtime-only references in control statements,
assignments, include paths, TOML keys, table headers and unquoted values
are unsupported. A tag mixing runtime-only references and load-time
local variables cannot be deferred because those locals will not exist
in the execution-time context. Tera comments and raw blocks are not
interpreted as runtime references. Undeclared names in ordinary load-time
expressions still cause normal Tera errors.

## Cargo features

| feature       | default | what it adds |
|---------------|---------|--------------|
| `std-helpers` | yes     | functions: `env(name, default?)`, `home()`, `is_windows()`, `is_linux()`, `is_mac()`. filters: `\| hash`, `\| port_offset(start, range)` |
| `shell`       | no      | `ps()` / `psf()` (Windows), `bash()` / `bashf()` (Unix) |
| `merge`       | no      | `load_merged()` / `discover_config_files()` — yui/shun-style multi-file config loading |
| `tracing`     | no      | emit `tracing` events from internal operations |

The point: today, every yukimemi/* tool that consumes a TOML config
written by-hand re-implements

1. *"Pre-extract `[vars]` from the raw text so the template can
   reference its own vars."*
2. *"Iteratively resolve `vars.a = "{{ vars.b }}"` cross-refs."*
3. *"Standard Tera helpers like `env(name='X')`, `is_windows()`."*
4. *"A standard system context with os / arch / user / host."*
5. *"Multi-file merge with vars accumulating across files."*

…and they all do it slightly differently. teravars is the one place
that intent lives.

## License

Same as the parent projects: MIT.

[Tera]: https://keats.github.io/tera/
[shun]: https://github.com/yukimemi/shun
[rvpm]: https://github.com/yukimemi/rvpm
[todoke]: https://github.com/yukimemi/todoke
[yui]: https://github.com/yukimemi/yui
