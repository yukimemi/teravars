# AGENTS.md

Guidance for AI agents (Claude / Codex / Gemini) working in this
repo. The yukimemi/* shared conventions live in the
`<!-- kata:agents:* -->` blocks below, sourced from
`yukimemi/pj-base` / `pj-rust` / `pj-rust-lib` via `kata apply` —
see those for git workflow, PR review cycle, build/lint/test
commands, library release flow, and renri worktree usage.

The sections above the marker blocks are teravars-specific and
consumer-owned: edit them freely; `kata apply` won't touch them.

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

## teravars-specific tooling notes

The base / rust / rust-lib marker blocks below cover the
yukimemi/* common toolchain (cargo make, renri, jj-first
worktrees, library release flow). Two repo-specific elaborations
that matter when working in teravars:

### jj-first colocation

This repo is colocated git+jj. `renri add` defaults to **jj**,
which creates a non-colocated jj workspace where `jj` commands
work and `git` does not — see
[jj-vcs/jj#8052](https://github.com/jj-vcs/jj/issues/8052) for
why secondary colocation isn't possible yet. Stick to the jj
default unless there's a specific reason to use git tooling.

### Hooks in jj workspaces don't fire

The pre-push hook installed by `cargo make hook-install` lives
in the main repo's `.git/hooks/pre-push`.

- **git worktrees** share that hook directory, so plain
  `git push` from a worktree triggers `cargo make check`
  automatically.
- **jj workspaces** push via `jj git push`, which uses libgit2
  directly and **does not fire git hooks**. From a jj workspace,
  run `cargo make check` manually before
  `jj git push --bookmark <branch-name>` — there's no automatic
  gate.

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

<!-- kata:agents:base:begin -->
## Shared conventions

This file is the agent-agnostic source of truth (per the
[agents.md](https://agents.md) convention). The matching
`CLAUDE.md` and `GEMINI.md` files are thin shims that point back
here so each tool's auto-load behaviour still finds something.
**Edit AGENTS.md, not the shims.**

### Git workflow

- **No direct push to `main`.** Open a PR.
  - Exception: trivial typo / whitespace / docs wording fixes.
- Branch names: `feat/...`, `fix/...`, `chore/...`.
- **PR titles + bodies in English. Commit messages in English.**
- **Releases are PR-driven, tagging is automatic.** Bump
  `[workspace.package].version` (workspace) or `[package].version`
  (single crate) in a `chore/release-vX.Y.Z` PR. On merge to `main`,
  `.github/workflows/auto-tag.yml` (kata-managed) detects the bump,
  pushes the `vX.Y.Z` tag, and that tag fires `release.yml` for
  binary builds + crates.io publish. **Do not run `git tag` by
  hand** — the bot tag will collide and the manual push fails.

### PR review cycle

- Every PR runs reviews from **Gemini Code Assist** and
  **CodeRabbit**. Wait for both bots to post, address their
  comments (push fixes to the PR branch), and merge only after
  feedback is resolved.
- **After opening a PR, immediately enter the review-monitoring
  loop — do not ask the user whether to start it.** Drive the
  cadence with `/loop` — fixed-interval mode (e.g.
  `/loop 60s …`) schedules ticks via `CronCreate`; dynamic mode
  (no interval, `/loop …`) self-paces via `ScheduleWakeup`. The
  agent actively pulls fresh state each tick with
  `gh pr view <N> --json state,reviews,comments,statusCheckRollup`
  and `gh api repos/<owner>/<repo>/pulls/<N>/comments` (the
  latter covers inline review comments, which `gh pr view`
  does not surface) and reacts to new bot feedback. Passive
  watchers (background `gh` polls, file watchers, hooks) cannot
  trigger active follow-up, so they are not a substitute —
  without an active wake-up the agent never re-reads the PR.
- **Default polling interval: 60s.** Gemini Code Assist /
  CodeRabbit historically reply within ~1–3 minutes of a push or
  thread reply, so a 60s tick catches them on the next wake-up
  without burning cache: 60s sits well inside the 5-minute
  prompt-cache TTL, so the conversation context stays cached
  across ticks. Do **not** stretch the interval to 300s — that
  is the worst-of-both window (you pay the cache miss without
  amortizing it). If the PR is idle but a bot re-review is still
  expected (e.g. a CodeRabbit rate-limit refill window), step
  **up** to 1200–1800s instead.
- **Stop the loop entirely when only owner approval is missing.**
  Once review bots are quiet (or quiet-by-exception — version-bump
  skip, Renovate/Dependabot skip), CI is green, and there is no
  other expected follow-up, the *only* remaining action is human
  approval. GitHub already notifies the owner; the agent
  re-entering on every cron tick to find the same "still waiting
  on owner" state burns cache and adds no value. Stop scheduling
  further wake-ups (`CronDelete` in fixed-interval mode; simply
  omit the next `ScheduleWakeup` in dynamic mode) and report the
  wait state to the user. The owner restarts the loop after their
  next push if a fresh bot pass is wanted, or merges directly.
  (A CodeRabbit rate-limit window doesn't qualify on its own — a
  re-review is still expected once the quota refills, so step up
  to 1200–1800s instead and let it ride. Stopping is only correct
  when the owner has explicitly chosen to skip the bot pass per
  the rate-limit exception below.)
- **Reply to reviewers after pushing a fix.** Reply on the
  corresponding review thread with an **@-mention**
  (`@gemini-code-assist` / `@coderabbitai`). Silent fixes are
  invisible to reviewers and cost the audit trail.
- A review thread is **settled** the moment the latest bot reply
  is ack-only ("Thank you" / "Understood" / a re-review summary
  with no new findings) or 30 minutes elapse with no actionable
  comment.
- **Merge gate**: review bots quiet AND owner explicit approval.
- Bot-authored PRs (Renovate / Dependabot) skip the bot-review
  gate; CI green + owner approval is enough.
- **Version-bump-only PRs** (a single `chore/release-vX.Y.Z`
  branch whose entire diff is `[workspace.package].version` /
  `[package].version` + the matching inter-crate refs +
  `Cargo.lock`) **also skip the bot-review gate.** There is
  nothing for the bots to find in a version bump, and the
  release pipeline downstream of merge (auto-tag → release.yml)
  is time-sensitive. CI green + owner approval is enough.
- **Treat CodeRabbit rate-limit notices as "quiet" for the
  merge gate.** If CodeRabbit only posts a "Review limit
  reached" quota-exhaustion message (no findings, no inline
  comments), it has produced no review content — there is
  nothing to address. Re-trigger with `@coderabbitai review`
  once the quota refills if you want a real pass; for small or
  time-sensitive PRs, merge on owner approval without waiting.

### Worktree workflow

Use [`renri`](https://github.com/yukimemi/renri) for any
commit-bound change. From the main checkout:

```sh
renri add <branch-name>            # create a worktree (jj-first)
renri --vcs git add <branch-name>  # force a git worktree
renri remove <branch-name> -y --non-interactive  # cleanup after merge (agent-safe; see note)
renri prune                        # GC stale worktrees
```

Read-only inspection can stay on the main checkout.

**Agents / non-interactive shells:** `renri remove` prints a details
panel and waits for a confirmation prompt — without `-y` it **hangs**,
and `--non-interactive` *alone* errors asking for `-y`. Always pass
`-y`, and add `--non-interactive` so a mistyped/omitted name fails
instead of opening a fuzzy picker (the same picker-fallback applies to
`remove` / `cd` / `exec` with no name). Use `-f`/`--force` to remove a
worktree that still has uncommitted changes or conflicts. To sweep
every merged-PR worktree in one shot: `renri remove --merged -y`.

### kata-managed sections

Several files in this repo are managed by `kata apply` from the
[`yukimemi/pj-presets`](https://github.com/yukimemi/pj-presets)
templates — the bytes between `<!-- kata:*:begin -->` and
`<!-- kata:*:end -->` markers, plus the overwrite-always files
listed in `.kata/applied.toml`. **Editing those bytes locally
won't survive the next `kata apply`** — push the change to the
upstream template repo (`yukimemi/pj-base` / `yukimemi/pj-rust` /
…) instead. The marker scopes are layered:

- `kata:agents:base:*` — language-agnostic conventions (this section).
- `kata:agents:rust:*` — added when `pj-rust` applies.
- `kata:agents:rust-cli:*` — added when `pj-rust-cli` applies.
<!-- kata:agents:base:end -->
<!-- kata:agents:rust:begin -->
### Rust workflow

This repo follows the shared Rust toolchain conventions. The
language-agnostic conventions block above (`kata:agents:base:*`)
covers git workflow, PR review cycle, and worktree usage.

### Build / lint / test

```sh
cargo make check                    # fmt --check + clippy + test + lock-check (the pre-push gate)
cargo make setup                    # one-time hook install + apm install
cargo build                         # debug build
cargo build --release               # release build
cargo test                          # tests; add -- --nocapture for stdout
```

`cargo make check` is what `.github/workflows/ci.yml` runs and what
the local pre-push hook calls — anything that passes locally
should pass on CI and vice versa. Don't paper over a failing
clippy by sprinkling `#[allow(clippy::...)]`; fix the underlying
issue or push back on the lint with reasoning.

### Toolchain pin

The Rust toolchain is pinned via `rust-toolchain.toml` and the
project compiles with the `stable` channel. Don't introduce
nightly-only features without a real reason; if you do, document
the reason in the relevant module.

### Lint / format policy

`rustfmt.toml` and `clippy.toml` are kata-managed (sourced from
`yukimemi/pj-rust`). Edits to those files in this repo won't
survive the next `kata apply`; if a setting is wrong, push the
fix to `yukimemi/pj-rust` so every Rust project using these templates picks
it up.

### CI workflow

`.github/workflows/ci.yml` is also kata-managed. The source lives
in `yukimemi/pj-rust/.github/workflows/ci.yml.template` (the
`.template` suffix keeps GitHub Actions from running the source
itself in pj-rust); each Rust project receives the rendered
`ci.yml` via `kata apply`. Action versions are bumped centrally
by Renovate at `yukimemi/pj-rust` and propagate down on the next
apply, so don't bump them locally — Renovate is configured
(via the kata-distributed `renovate.json`) to ignore
`.github/workflows/ci.yml` and `.github/workflows/release.yml`
in each PJ to avoid the bump→clobber loop.

### Releasing: version bump PR + auto-tag

Releases are triggered from `main` by a Cargo.toml version
change. `.github/workflows/auto-tag.yml` is kata-managed (source:
`yukimemi/pj-rust/.github/workflows/auto-tag.yml.tera`). It
watches `main` and, whenever a commit lands that changes the
top-level `version = "..."` in `Cargo.toml`, it pushes a matching
`vX.Y.Z` tag — no manual `git tag` step is needed. The tag push
then fires `release.yml`; see `kata:agents:rust-lib:*` or
`kata:agents:rust-cli:*` for what release.yml does in each
crate shape.

Cut a release via a small PR — never `git push` the bump
straight to `main`, even though the base block lists version
bumps as an exception to "no direct push". `auto-tag.yml` only
fires on `main`-branch pushes, so the bump must land via a merge
either way; using a PR also gives CI a chance to gate the
release. Enable automerge so CI green = release start:

```sh
git switch -c chore/release-vX.Y.Z
# Edit `package.version` in Cargo.toml, then:
cargo build                     # let Cargo.lock follow
git commit -am "chore: release vX.Y.Z"
git push -u origin chore/release-vX.Y.Z
gh pr create --fill
gh pr merge --auto --squash --delete-branch
```

Once CI is green the PR auto-merges. `auto-tag.yml` then pushes
`vX.Y.Z`, which fires `release.yml`.

**Repo settings to set once:** enable
`delete_branch_on_merge=true` (Settings → General →
"Automatically delete head branches"). The `--delete-branch`
flag on `gh pr merge --auto` is effectively a no-op — gh
returns as soon as automerge is enabled, so the deletion has to
happen server-side, which requires the repo setting.

**Why `KATA_APPLY_TOKEN`:** GitHub refuses to fire downstream
workflows from tags pushed by the default `GITHUB_TOKEN`, so
`auto-tag.yml` pushes with `KATA_APPLY_TOKEN` (the same PAT
`kata-apply.yml` already uses). Each consumer repo needs a
`KATA_APPLY_TOKEN` secret set; if a version-bump merge silently
doesn't fire `release.yml`, the missing PAT is the first thing
to check.
<!-- kata:agents:rust:end -->
<!-- kata:agents:rust-lib:begin -->
### Rust library release flow

This is a Rust **library** crate, so the release pipeline is
publish-only: a successful tag push runs `cargo publish` to
crates.io and stamps the matching GitHub release page with
auto-generated notes. **No binaries** are uploaded — the
canonical artifact for a library is the `crates.io` tarball;
the GH release page exists for historical visibility and so
Renovate's release-notes manager (and any other tooling that
consumes GitHub Releases) has something to find.

Releases are triggered by a Cargo.toml version bump landing on
`main`. The bump flow itself (PR with automerge → `auto-tag.yml`
pushes `vX.Y.Z` → `release.yml` runs) is documented in
`kata:agents:rust:*` under "Releasing: version bump PR +
auto-tag" — that block also covers the `KATA_APPLY_TOKEN` and
`delete_branch_on_merge` setup. What `release.yml` then does for
a **library** crate:

1. Creates a GitHub Release at the tag with auto-generated
   notes (PRs since the previous tag).
2. `cargo publish --locked` to crates.io using the
   `CARGO_REGISTRY_TOKEN` repo secret.

Set the `CARGO_REGISTRY_TOKEN` secret once per repo (`gh secret
set CARGO_REGISTRY_TOKEN`) before the first release. If the
crate is internal-only and shouldn't go to crates.io, drop the
`publish` job locally (release.yml is `when = "once"` so the
edit survives subsequent applies) or set `package.publish = false`
in `Cargo.toml`.

### MSRV / SemVer caveats for library authors

Unlike CLIs (where lockfile-pinned versions are what users
consume), libraries publish version *ranges* in their downstream
projects' `Cargo.toml` files. Two things to keep in mind when
bumping:

- **MSRV signalling.** Setting `package.rust-version` in
  `Cargo.toml` tells cargo the minimum Rust this crate will
  build with. Bump it deliberately (e.g. when adopting a stable
  feature that requires a newer toolchain) and call out the bump
  in the release notes — downstream pinning their own MSRV
  needs the visibility.
- **`rangeStrategy` in renovate.json.** This template inherits
  pj-rust's `rangeStrategy: "bump"`, which is right for binary
  crates but raises the MSRV ceiling for library downstreams
  more than necessary. If a downstream of this library
  complains, override locally with `rangeStrategy: "replace"`
  (and consider whether the broader template default should
  flip — track upstream).

### `cargo publish --dry-run`

Before opening the version bump PR, validate the publish step
locally with `cargo make publish-dry` (defined by pj-rust).
Catches metadata issues — missing `description`, `license`,
`repository`, `readme` — that crates.io rejects on the actual
publish. Doing this before the PR is cheaper than catching it
post-merge, where the only recovery is bumping again.
<!-- kata:agents:rust-lib:end -->
