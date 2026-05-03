# teravars

> [Tera] templating + smart `[vars]` handling for self-rendering TOML
> configs. Extracted from the duplicated patterns in
> [shun] / [rvpm] / [todoke] / [yui] / [spyrun].

**Status: 0.1.3 — core + multi-file merge + `include` directive +
`hash` / `port_offset` filters shipped. Migration of the 5 sibling
tools is the next step; see [ROADMAP.md](./ROADMAP.md).**

## Quickstart

```rust
use teravars::{Context, Engine, extract_vars, resolve, system_context};

let raw = std::fs::read_to_string("config.toml")?;
let mut engine = Engine::new();              // Tera + standard helpers

let mut vars = extract_vars(&raw)?;          // text-based [vars] carve-out
resolve(&mut vars, &mut engine)?;            // iterate cross-refs to fixpoint

let mut ctx: Context = system_context();     // system.os/arch/user/host
ctx.insert("vars", &vars);

let rendered = engine.render(&raw, &ctx)?;
let cfg: MyConfig = toml::from_str(&rendered)?;
```

`resolve` mutates `vars` in place. On non-convergence it returns
`Err(Error::ResolveNotConverged { .. })` while leaving `vars` in its
last partially-resolved state — callers that prefer resilience over
strictness can `if let Err(_)` and continue with what's there.

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

- Includes are loaded **before** the file that declares them, so the
  declaring file overrides anything it includes (the natural reading
  of "build on top of these").
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
  are present in the same file, that's `Error::IncludeConflict`.
  Pick one form per file.

The `[teravars]` namespace exists as a fallback for the rare case
where your application config legitimately uses `include` as a
top-level key for something else.

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
[spyrun]: https://github.com/yukimemi/spyrun
