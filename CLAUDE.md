# CLAUDE.md

Guidance for Claude Code when working in this repo.

## What teravars is

A small Rust library (~1500 lines) that wraps the [Tera] templating
engine with the conventions every yukimemi/* tool reinvented for
TOML-config rendering: a **text-based `[vars]` extractor**, an
**iterative cross-reference resolver**, a standard **`system.*`
context** with `os/arch/user/host/cwd`, **typed shell helpers**
(`env`, `is_windows`, `ps`, `bash`, …), **multi-file merge** with
deep-recursive semantics, and an **`include = [...]` directive**
for splitting config across files.

Crate name `teravars`, repo `yukimemi/teravars`. The five sibling
tools are slated to migrate onto it: yui, rvpm, todoke, shun, spyrun.

## Source layout

```
src/
  lib.rs        — module list, public re-exports, top-level docs + doctest
  engine.rs     — Engine: wraps tera::Tera; new()/new_minimal()/render()/
                   register_function()/tera_mut()
  error.rs      — teravars::Error (thiserror); From<tera::Error> walks
                   the source chain so callers see the real cause, not the
                   bare `Failed to render '__tera_one_off'`
  vars.rs       — extract_vars (text-based, Tera-block-depth aware),
                   resolve / resolve_with_max_iter (iterative fixed-point),
                   expand_value (in-place tree walker),
                   scan_tera_tags (pub(crate); reused by merge.rs)
  system.rs     — SystemInfo {os, arch, user, host, cwd} + system_context()
  merge.rs      — load_merged + discover_config_files + deep_merge +
                   include-directive resolution with cycle detection
  helpers/
    mod.rs      — register_default; gates by feature
    env.rs      — env(name, default?)                     (cfg=std-helpers)
    os.rs       — is_windows / is_linux / is_mac          (cfg=std-helpers)
    filters.rs  — hash (FNV-1a 64-bit) + port_offset      (cfg=std-helpers)
    shell.rs    — ps/psf (Windows), bash/bashf (Unix)     (cfg=shell)
tests/
  merge.rs      — integration tests for load_merged / include
```

## Key design decisions (don't rediscover)

These were settled during the initial design pass; flag with the
user before reverting any of them.

- **`extract_vars` is text-based, not post-parse.** Each yukimemi/*
  tool reinvents the same line-walking extractor because parsing
  the TOML first means Tera blocks already corrupted the structure.
  We track `{% if %}` / `{% for %}` / `{% block %}` depth so vars
  inside conditional blocks aren't picked up. When a `[vars]`
  section header appears, we copy lines until the next non-vars
  section header. The tag scanner (`scan_tera_tags`) is shared
  with `merge.rs`'s skeleton-stripper.
- **`resolve` returns `Err` on non-convergence but leaves the
  `&mut Table` in its last partial state.** Default budget is 10
  iterations. Callers that prefer rvpm-style resilience over
  strictness do `if let Err(_) = resolve(...) { /* warn, continue */ }`
  and the partial result is right there. Callers that want strict
  failure propagate the `?`.
- **`load_merged` follows yui's semantics, not shun's.** Per-file
  Tera rendering with vars accumulated from earlier files in scope,
  then deep-recursive merge of the parsed result (tables merge,
  arrays append, scalars overwrite). shun's selective-shallow merge
  with `APPEND_KEYS` / `TABLE_MERGE_KEYS` is schema-aware business
  logic and lives in shun, not here.
- **`include = [...]` is a teravars directive, NOT a Tera include.**
  Tera's `{% include "..." %}` is text-level inline and requires
  template registration; that's a different problem. teravars
  include is TOML-aware: the included file is fully loaded
  (extract_vars → resolve → render → merge), then merged into the
  accumulator before the declaring file. Cycle detection uses
  canonical paths in a HashSet.
- **`include` lives at root, with `[teravars] include = [...]` as
  a namespaced fallback.** If both forms appear in the same file,
  it's `Error::IncludeConflict`. Both forms are stripped from the
  merged result (`teravars` is reserved).
- **Deferred-template trick for `vcs.*`-style late binding.** A
  consumer (renri) wants `{{ vcs.repo }}` inside a layout template
  to NOT render at config-load time, and instead survive into the
  next render pass when the actual branch is known. Solution:
  pre-populate the load-time context with self-referential
  placeholders — `vcs = { repo: "{{ vcs.repo }}", ... }` — so Tera
  substitutes the literal back. Document this idiom for future
  consumers; it's not obvious.
- **No `enc` / `dec` / `setenv` helpers.** spyrun has `enc`/`dec` as
  AES-256-GCM (a security primitive, not a generic helper) and
  `setenv` as a side-effecting env mutator. Both deliberately stay
  in spyrun. teravars rendering must be pure / idempotent so the
  resolve loop's fixpoint detection is meaningful.
- **`hash` filter is FNV-1a 64-bit, not crypto.** Pure Rust, no
  deps, deterministic across platforms / processes / versions. The
  use case is per-branch resource allocation (port numbers, db
  schema names) — collision resistance is enough; preimage
  resistance is irrelevant.
- **`port_offset(start, range)` uses `(n % range) + start`.** Simple
  and predictable. Errors on missing args or zero range; that's the
  full validation surface.
- **shell helpers split by OS, not unified.** `ps` / `psf` only
  exist on Windows targets (`cfg(windows)`), `bash` / `bashf` only
  on Unix (`cfg(unix)`). On the wrong target the function still
  exists but errors with a clear "X is only available on Y
  targets" message — so the registry is consistent across builds
  but the behaviour reflects reality.

## Development

**Practice TDD.** Red-green-refactor.

```bash
cargo make setup                            # one-time on clone: hook + apm
cargo test                                  # default features
cargo test --all-features                   # incl. shell, merge, tracing
cargo test --no-default-features            # core only
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo make check                            # all of the above (pre-push gate)
```

`cargo make setup` is `hook-install` + `apm-install` — runs once
per clone. Individual tasks:

- `cargo make hook-install` — wires `.git/hooks/pre-push` to
  `cargo make check`.
- `cargo make apm-install` — runs `apm install`, which compiles
  the [renri] skill (declared in `apm.yml`) into
  `.claude/skills/` + `.gemini/skills/` + `.github/skills/`.
  **Requires the [APM](https://github.com/microsoft/apm) CLI on
  `PATH`** — `scoop install apm` (Windows),
  `brew install microsoft/apm/apm` (macOS), `pip install apm-cli`,
  or `curl -sSL https://aka.ms/apm-unix | sh`. teravars itself is
  a Rust library and ships no agent primitives, but contributors
  who use AI agents to develop it benefit from having renri
  available (parallel-branch worktrees, jj workspaces). Pinned to
  `yukimemi/renri#v0.1.5` in `apm.yml`; lockfile in
  `apm.lock.yaml`.

`cargo make check` mirrors CI exactly. The pre-push hook runs it,
so failed checks block push.

[renri]: https://github.com/yukimemi/renri

## Resilience principle

teravars is a **library** — its job is to surface failures clearly
and let the caller decide policy. Specifically:

- Tera render failure → walk `err.source()` and surface the actual
  cause inside `Error::Render`. No bare `__tera_one_off` messages.
- `extract_vars` parse failure → `Error::Extract` with the file's
  TOML error attached.
- `resolve` non-convergence → `Error::ResolveNotConverged
  { iterations }`. The `&mut Table` keeps its partial state so the
  caller can warn and continue.
- `load_merged` per-file failure → bail with the offending path
  in the error message; don't try to be clever about which subset
  is recoverable.
- Include cycle → `Error::IncludeCycle { path }`.

The library never uses `tracing` for anything important. The
`tracing` feature (off by default) wires events for callers who
want visibility.

## Git workflow

- **No direct push to `main`.** Open a PR.
  - Exception: trivial typo / whitespace / docs wording fixes.
  - Exception: standalone version bumps (`Cargo.toml` + `Cargo.lock`
    refresh + `git tag vX.Y.Z`).
- Branch names describe the change (`feat/...`, `fix/...`).
- **PR titles + bodies in English. Commit messages in English.**
- Tag-based releases: `git tag vX.Y.Z && git push origin vX.Y.Z`.
  The release workflow verifies tag-vs-Cargo.toml and publishes
  to crates.io.

### PR review cycle

- Every PR triggers **Gemini Code Assist** and **CodeRabbit** reviews.
  Wait for both, address comments (push fixes to the PR branch),
  and merge only after feedback resolves.
- **Reply to the reviewer after pushing a fix.** Post a reply with
  `@gemini-code-assist` / `@coderabbitai` so the bot knows the
  feedback was acted on.
- **Settle rule**: a thread settles when the latest bot reply is
  ack-only. New actionable comments un-settle it.
- **Stop conditions**: all open threads settled, OR no bot reply
  for 30 min after the last actionable comment.
- **Merge gate**: review bots stopped posting actionable comments
  AND @yukimemi has approved.
- **Bot-authored PRs (Renovate / Dependabot)**: review bots skip
  them by default. Merge if CI is green and owner approves.

## Useful invocations

```sh
# Run only the merge integration tests
cargo test --features merge --test merge

# Single-feature focused testing during dev
cargo test --features shell helpers::shell

# Doctest in lib.rs (the README quickstart)
cargo test --doc

# Render error chain visible at the CLI
RUST_BACKTRACE=1 cargo test error::tests::tera_error_chain_is_flattened
```

## Consumers

teravars is consumed by:

- [renri](https://github.com/yukimemi/renri) — git worktree + jj
  workspace manager. First production consumer; uses `load_merged`,
  the `include` directive, system context, and the `hash` /
  `port_offset` filters.
- (planned) yui, rvpm, todoke, shun, spyrun — see ROADMAP.md.

When changing teravars's public API, prefer additive over breaking;
when breaking is unavoidable, coordinate with each consumer's PR.

## Version + changelog

Version lives only in `Cargo.toml`. `cargo check` refreshes
`Cargo.lock` after a bump. Commit titles follow
`<type>(<scope>): <summary> (vX.Y.Z)` (e.g.
`feat(filters): hash + port_offset (v0.1.3)`) so the release
surface is traceable from `git log`.

[Tera]: https://keats.github.io/tera/
