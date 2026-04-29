# teravars — ROADMAP

Survey of how each yukimemi/* tool currently uses Tera, and the union
of features `teravars` should provide so all five can drop their
local copy.

---

## Survey

### shun (Tauri app, `shun/src-tauri/`)

- `tera::Tera::one_off(template, ctx, false)` — single-shot rendering
  inline, no engine kept around.
- `extract_vars(&toml::Value) -> HashMap<String, String>` — pulls
  `[vars]` out of an already-parsed `toml::Value` (post-TOML-parse).
- `expand_value(&mut toml::Value, &Context)` — walks the parsed value
  tree and re-renders every string field through Tera.
- No custom helpers / functions registered.
- No system context.
- Vars are flat `HashMap<String, String>` (no nested tables).

### rvpm (`rvpm/src/config.rs`)

- `extract_vars_section(toml_str: &str) -> String` — text-based
  extraction (line walk), survives `{% ... %}` Tera blocks elsewhere
  in the file. **Most thorough text-based extractor of the bunch.**
- Iteratively resolves vars cross-references (`for _ in 0..MAX`)
  until convergence.
- `env`, `is_windows` exposed in context (not as `register_function`
  but inserted as values).
- Uses `vars`, `env`, `is_windows` keys in context.
- Rendered string per call (`tera.render_str`).
- No system context (`os` / `arch` / `user` / `host`).

### todoke (`todoke/src/template.rs` + `config.rs`)

- `extract_vars(text: &str) -> BTreeMap<String, toml::Value>` —
  text-based, similar to rvpm's.
- Registers function helpers via `Tera::register_function`:
  - `is_windows()` / `is_linux()` / `is_mac()`
- Uses standard `tera::Context` with `vars` plus dispatch-time
  variables (`group`, `file_path`, …).
- Re-renders templates inside the parsed config tree.
- No `env(name=)`. No `os` / `arch` / `user` / `host` context.

### yui (`yui/src/template.rs` + `config.rs`)

- `Engine::new()` thin wrapper over `Tera::default()` + registers
  `env(name=, default=)` as a function.
- `pre_extract_vars(raw, file)` — text-based `[vars]` extraction
  (line walk), tolerates `{% set %}` blocks at file top.
- `resolve_vars_refs(&mut Table, ...)` — iterative convergence with
  `MAX_VARS_RESOLVE_ITERATIONS`. Walks `toml::Table` recursively and
  re-renders every string value.
- Standard context via `YuiVars { os, arch, host, user, source }`.
- Vars merged across files (`config.toml` → `config.*.toml` →
  `config.local.toml` in alphabetical-then-local order).
- Tera error chain walked manually so `Failed to render '__tera_one_off'`
  carries the actual cause string.

### spyrun (`spyrun/src/util.rs`)

- Largest helper set. `register_function`:
  - `env(name=)` — read env var
  - `setenv(name=, value=)` — set env var
  - `enc(text=)` / `dec(text=)` — string encode / decode
  - `ps(script=)` — run a PowerShell snippet, capture stdout
  - `psf(file=)` — run a `.ps1` file, capture stdout
- Custom `Context` per call site (no shared standard context).
- Uses Tera throughout (settings, command, logger, util).

---

## Union — what teravars should expose

### Core API

| symbol | what it does | inspired by |
|---|---|---|
| `Engine` | Tera engine + opinionated helper set | yui |
| `Engine::new()` | default helpers preloaded | yui |
| `Engine::new_minimal()` | bare Tera, no helpers | shun |
| `Engine::register_function(name, fn)` | add custom helper | spyrun |
| `Engine::render(src, ctx)` | render with a clean Tera error chain (no bare `Failed to render '__tera_one_off'`) | yui |
| `Engine::render_str(src, ctx)` | thin pass-through for one-offs | shun |
| `extract_vars(text) -> Table` | text-based `[vars]` extraction (handles `[vars.sub]`, skips `{% %}` blocks) | rvpm + yui |
| `resolve(&mut vars, &mut engine, max_iter)` | iterative cross-reference convergence | rvpm + yui |
| `system_context() -> Context` | standard `system.os/arch/user/host` (or `host` for compat) | yui |
| `expand_value(&mut toml::Value, &Context, &Engine)` | walk a parsed TOML tree and re-render every string field | shun |

### Standard helpers (Engine::new pre-registers)

| name | sig | source |
|---|---|---|
| `env(name=, default=?)` | read env var with optional fallback | yui / spyrun |
| `is_windows()` / `is_linux()` / `is_mac()` | OS predicate functions | todoke / rvpm |
| `setenv(name=, value=)` | set env var (returns empty string) | spyrun |
| `enc(text=)` / `dec(text=)` | escape helpers (URL? base64? — spec to confirm) | spyrun |
| `ps(script=)` / `psf(file=)` | run PowerShell, capture stdout — gated `cfg(windows)` | spyrun |

### Standard context

`system_context()` populates a single `system` (or `host`) namespace:

```
system.os        — "windows" | "linux" | "macos"
system.arch      — "x86_64" | "aarch64" | …
system.user      — current user
system.host      — hostname
system.cwd       — current working dir (optional, opt-in)
```

Tools like `yui` historically used `yui.os` etc.; teravars should ship
the same data under `system.*` and let callers alias if they want
backwards compatibility (one line in their own `Context`).

### Multi-file merge helper (yui-only today)

```rust
let merged = teravars::load_merged([
    "config.toml",
    "config.local.toml",
])?;
```

Optional add-on. Could live in a `teravars::config` module behind a
feature flag if it bloats the core API.

### Quality-of-life

- Walk Tera's error chain (`err.source()` recursively) so callers see
  `template: Failed to render '__tera_one_off': Variable 'vars.foo' not found in context...`
  instead of the bare top-level message. Keep this in `Engine::render`.
- Rename detection / type preservation: yui's `resolve_vars_refs`
  recursively walks `toml::Table` / `Value::Array`. Make that the
  default behaviour of `resolve`, not a separate codepath.

---

## Migration plan

In rough size order:

1. **yui** — most recent, cleanest design. Port first, validate the API.
2. **rvpm** — text-based extractor lives here in its most thorough form;
   confirm behaviour parity.
3. **todoke** — small surface; mostly the `is_*` helpers + extract_vars.
4. **shun** — switches from `Tera::one_off` to `Engine::render_str`.
5. **spyrun** — biggest helper surface; needs `enc` / `dec` / `ps` /
   `psf` semantic carry-over and Windows-gated tests.

Each migration is its own PR per project, tracking duplication
removal.

---

## Out of scope

- Tera dialect changes (we use upstream `tera` as-is).
- Schema validation (callers' job).
- TOML parsing beyond the text-based `[vars]` carve-out.
- Lockfile / hash-based change detection (yui-specific, stays in yui).
