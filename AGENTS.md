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
  comments.rs   — strip_toml_comments: TOML/Tera-aware `#` comment stripper
  engine.rs     — Engine: wraps tera::Tera; new()/new_minimal()/render()/
                   render_toml()/register_function()/tera_mut()
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
  merge.rs      — integration tests for load_merged / include / comments
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
- **TOML comments are stripped before rendering, not respected by
  Tera.** Tera has no idea `#` starts a TOML comment, so rendering a
  config file also rendered its comments — and the two most common
  comment styles in a documented config are hard load failures:
  `# port = "{{ vars.not_defined }}"` (field not defined) and
  `# disable with {% if false %}` (unexpected end of input). Every
  consumer hit this. `strip_toml_comments` (comments.rs) removes
  comment text at the entry points that know they are looking at a
  TOML document — `extract_vars`, `Engine::render_toml`, and once per
  file in `load_merged` — while plain `Engine::render` stays literal
  because it renders arbitrary text where `#` means nothing. The
  scanner tracks TOML string state (`"…"`, `'…'`, `"""…"""`,
  `'''…'''`, including the 3–5-quote terminating runs TOML permits)
  and treats `{{ }}` / `{% %}` / `{# #}` as opaque — following Tera's
  own string literals inside them, so a `}}` in `replace(from="}}")`
  does not end the tag early. A URL fragment or a `replace(from="#")`
  argument therefore survives; it drops only `#`-to-end-of-line,
  keeping line numbers so error locations still match the file on
  disk. In `load_merged` the removal is unobservable, since the
  rendered text is parsed into a `toml::Table` and the source
  discarded; `Engine::render_toml` hands back the rendered `String`,
  so a caller that inspects it does see comment-free output.
  Caveat for doctests: rustdoc treats a line whose first two
  characters are `#` and a space as a hidden line, even inside a
  string literal, so examples with commented TOML use `##` or
  `concat!`.

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
- **Releases are PR-driven and tagging is automatic** — in repos that
  ship a release pipeline. Bump the version in the project's own
  manifest in a `chore/release-vX.Y.Z` PR; on merge to `main` the
  language layer's `auto-tag.yml` detects the bump, pushes the
  `vX.Y.Z` tag, and that tag is what fires `release.yml`. **Do not run
  `git tag` by hand** — the bot tag will collide and the manual push
  fails. The specifics belong to the layers shipping those two
  workflows, which are not the same layer: `kata:agents:rust:*` for
  which file holds the version and for `auto-tag.yml`,
  `kata:agents:rust-{cli,lib}:*` for what `release.yml` builds and
  publishes. A repo with no `auto-tag.yml` has no release pipeline at
  all: nothing tags, and the version field in its manifest may well
  be decoration.

### Pre-merge review

Review happens **before the pull request, on the operator's machine**,
via [magi](https://github.com/yukimemi/magi). This layer no longer
ships PR-side review bots: `claude-review.yml` and `claude.yml` were
removed from it. Their scope was
human-authored PRs — their own job-level `if:` already excluded
`chore/release-*`, `kata-apply/auto`, `apm-bump/auto` and
Renovate / Dependabot — which is exactly the set magi reviews, so
keeping them meant reviewing the same diff twice, a
`CLAUDE_CODE_OAUTH_TOKEN` secret per repository, Actions minutes on
private repos, and one trap that silently cost reviews: a PR editing
either workflow was skipped by `claude-code-action`'s
workflow-validation check and merged with a green check and no
review attached.

**"Removed" is a statement about this template layer, not about
every repo's current state.** Dropping a `[[file]]` entry stops kata
from managing the rendered file — it does not delete it. A repo that
had these workflows before this change keeps `claude-review.yml` /
`claude.yml` (and the `CLAUDE_CODE_OAUTH_TOKEN` secret) under
`.github/workflows/` until someone deletes them by hand, and until
then they still fire on every human-authored PR. Check
`.github/workflows/` before treating a PR as unreviewed-except-magi:
if either file is still there, its comments are a real review, not
noise to ignore.

- **`magi review <branch>`** runs only the review + verification +
  gate half of magi's graph: nothing competes, no implementation, no
  judging, no vote. That is the mode for hand-written work.
  `magi run "<task>"` is the full competition, for work handed over
  whole. Both end at the same gate.
- What the loop actually does: each reviewer gets its **own detached
  worktree pinned at the commit under review** (no reviewer can
  perturb the tree, and the fixer never races one); `verify.e2e` runs
  in the branch's worktree and its output is fed to the fixer;
  finding ids (`R2-1-3`) are assigned by magi, not by the agent, so
  the fixer's adoption report can be matched against them; the loop
  is bounded by `review_rounds`; `verify.gate` must exit 0 before any
  merge is attempted.
- **`magi.toml` is repo-owned, not kata-managed.** Point
  `verify.gate` at the exact command CI runs, so a local pass means a
  green PR, and point `verify.e2e` at the invocation that actually
  covers the repo — feature flags included. A gate that differs from
  CI turns a clean magi run into a red PR, which is the one failure
  this arrangement cannot absorb.
- **If you did not run magi, the change was not reviewed, and nothing
  will tell you.** Do not open a PR for a hand-written change before
  `magi review` comes back clean; if you must, say so in the PR body
  and say why. What does *not* count as a substitute: a green CI run
  (it compiles and tests, it does not review), and CodeRabbit's
  silence.
- **CodeRabbit stays installed and is not part of the gate.** It does
  not auto-review repositories under 10 stars — the common case here —
  so treat it as absent unless it posts. When it does post, its
  findings are a real review: address them, reply **in the inline
  thread** with an `@coderabbitai` mention (the review-comment
  *replies* endpoint,
  `gh api repos/<owner>/<repo>/pulls/<N>/comments/<id>/replies -f body=…`),
  and reply even when declining — say why, because a silent skip
  reads as overlooked. A "review limit reached" quota notice carries
  no findings and counts as quiet; re-trigger with
  `@coderabbitai review` when the quota refills if you want a real
  pass.
- **Read the report, not the exit status.** A reviewer seat that
  times out is logged as `WARN agent timed out seat=review-2` and
  then summarised as "raised 0 finding(s)" — indistinguishable from a
  genuinely clean pass in both the summary and `magi stats`. Check
  for timeouts before believing a clean round: a round where half the
  panel never answered is not a clean round.
- **Review artifacts stay local.** magi comments on a pull request
  only when it *stops* landing one. Findings, the fixer's adoption
  report and reviewer precision live in the run directory
  (`magi show`, `magi stats`). When the PR needs a record — a
  non-obvious fix, a finding declined with an argument — paste that
  part into the PR body or a comment yourself.
- With `merge = "pr"`, magi opens the pull request and keeps going:
  watches the checks, reads the review comments (human and bot), runs
  a bounded fix round when either is unhappy, pushes, and asks before
  merging. `land_approval` is on by default and **silence is a
  hold** — nothing merges unanswered. `magi answer` (or the web UI)
  is where it asks. Out of rounds leaves the PR open with a comment
  saying what still fails; `checks: unknown` never merges.
- **Merge gate**: magi's gate green — or CI green for a change magi
  never touched — **and** every review that did post resolved (a
  leftover `claude-review.yml`, CodeRabbit, a human) **and** the
  owner's explicit approval. The irreversible step stays a human
  decision.
- **No review-monitoring poll loop for bots this layer no longer
  ships.** The old loop existed to wait on them. Where a repo still
  has `claude-review.yml` (see above) the old cadence still applies
  until it is deleted; otherwise, after opening a PR wait for CI and
  report the wait state to the owner. When magi is landing the PR
  (`land = true`), magi does the watching.
- Bot-authored PRs (Renovate / Dependabot) need no review pass at
  all: CI green + owner approval.
- **Version-bump-only PRs** — a single `chore/release-vX.Y.Z` branch
  whose entire diff is `[workspace.package].version` /
  `[package].version` plus the matching inter-crate refs and the
  lockfile — likewise. There is nothing in a version bump for a
  reviewer to find, and the release pipeline downstream of merge
  (auto-tag → `release.yml`) is time-sensitive.

### Worktree workflow

> **Before your FIRST edit to any file, run `renri add` — NEVER edit the
> main checkout.** Read-only inspection (Read / Grep / Glob) stays on the
> main checkout; the instant you intend to *change* a file, you must
> already be in a worktree. The trap that keeps catching agents: diving
> into a fix the moment the diagnosis lands and editing in place. A
> concurrent agent shares the main checkout — your in-place edits will
> clobber theirs or be clobbered, and in a jj-colocated repo a stray
> working-copy commit entangles unrelated WIP into your branch. If you
> slip and edit in the main checkout, capture the diff first (jj already
> snapshotted it into the working-copy commit, so `jj diff > patch`; for
> git, `git stash` or save a patch — if you got as far as committing on a
> branch, just push it). Then reset the main checkout to pristine main
> (`jj new main@origin`, or `git switch -`), `renri add` a worktree, and
> re-apply the captured diff there.

Use [`renri`](https://github.com/yukimemi/renri) for any
commit-bound change. From the main checkout:

```sh
renri add <branch-name> --from main@origin            # create a worktree (jj-first), off latest upstream main
renri --vcs git add <branch-name> --from origin/main  # force a git worktree, off latest upstream main
renri remove <branch-name> -y --non-interactive  # cleanup after merge (agent-safe; see note)
renri prune                        # GC stale worktrees
```

Read-only inspection can stay on the main checkout.

**Always pass `--from <upstream main>`** (`main@origin` for jj,
`origin/main` for git). Without it, `renri add` forks off the *cwd
worktree's current HEAD* — in a long-lived main checkout that often
lags upstream, so the PR later shows up CONFLICTING against a `main`
that had already moved (e.g. a refactor merged upstream before the
branch was cut), forcing a manual re-port of the whole change.
`renri add` does fetch first, but fetching only updates `main@origin`
— it never moves the checkout's HEAD, so an explicit `--from` is what
guarantees a fresh base.

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
…) instead.

The marker scopes are layered, one per applied layer:
`kata:agents:base:*` is this section, and each layer adds its own
(`kata:agents:rust:*`, `kata:agents:rust-cli:*`,
`kata:agents:pnpm:*`, `kata:agents:firebase:*`, …). Which ones apply
*here* is a grep away: `<!-- kata:` in this file.

### This project's own conventions

Everything a layer ships is generic by construction: it describes the
stack the template assumed, not what this repo grew into. **Bytes
outside every marker pair are yours and survive `kata apply`** — so
project-specific conventions belong in a section of their own, outside
the markers (conventionally at the end of the file; if a later layer
appends its block below yours, no matter — kata only ever rewrites
between its own markers). Same mechanism as the `.gitignore` /
`.gitattributes` blocks.

Write those conventions down there rather than leaving them in one
agent's head, in commit archaeology, or in a README the agent will not
read. What earns a line:

- **Any layer default that does not hold here.** A layer states its
  assumption flatly ("Hosting is the primary target", "these rules are
  a placeholder to replace"). When the project has diverged, say so and
  say why — the layer's text keeps asserting the opposite on every
  apply, and an agent that only reads the blocks will act on it.
- **Facts duplicated across files with no compiler in between** — an
  address or a path that appears in code *and* in a rules/config file
  that cannot import it, a timeout that has to stay inside another
  timeout. List every copy, so the next edit finds them all.
- **kata-shipped files this project deleted on purpose**, together with
  the `once_applied = true` line in `.kata/applied.toml` that keeps
  them deleted. Otherwise someone helpfully restores one.
- **Shapes the runtime forces but no tool checks** — an export form a
  platform requires, import specifiers that must (or must not) carry a
  file extension, a directory whose contents are reachable by URL.
- **Invariants that money or access rest on**, naming the file and line
  that actually enforces them.
- **Which language the code speaks versus what a user reads**, when the
  two differ.

A repo whose `AGENTS.md` is nothing but kata blocks is a repo where
every agent re-derives all of that from scratch — and gets the layer
defaults wrong the same way each time.
<!-- kata:agents:base:end -->
<!-- kata:agents:rust:begin -->
### Rust workflow

This repo follows the shared Rust toolchain conventions. The
language-agnostic conventions block above (`kata:agents:base:*`)
covers git workflow, PR review cycle, and worktree usage.

### Build / lint / test

```sh
cargo make check                    # editorconfig-check + fmt --check + clippy + test + lock-check (the pre-push gate)
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

**In a workspace, the version is in more than one place.** A member
that is published and depended on by another member is declared
with both a `path` and a `version` — crates.io needs a
requirement it can resolve for somebody who is not building from
the checkout, so a bare `path` will not do:

```toml
my-core = { path = "crates/my-core", version = "0.4.2" }
```

That literal does not follow `[workspace.package] version`.
Nothing in Cargo makes it, and the release above will not either.

**It fails late and quietly.** `version = "0.4.2"` means `^0.4.2`,
so a stale pin keeps resolving through every *patch* release and
stops only at the first bump that crosses the minor — where
`cargo build` refuses with `candidate versions found which didn't
match`, in the middle of cutting the release. Two repos on these
templates hit exactly this, one of them three releases after its
pins were last correct, and the other had already written the
hazard down in prose and drifted anyway.

So bump the pins in the same commit, keep them in
`[workspace.dependencies]` rather than in each member, and assert
it rather than remembering it. A test is the cheapest place —
`cargo test` already runs in CI, and it needs no toolchain a Rust
workspace does not have. [pj-rust-workspace's
README](https://github.com/yukimemi/pj-rust-workspace#the-internal-version-pin-and-the-check-for-it)
carries one to copy into any member's
`tests/check_versions.rs`: `internal_pins_match_the_workspace_version`
fails when a pin and the workspace version disagree, and
`members_inherit_the_workspace_version` fails when a member writes
its own version or reaches for a sibling by path.

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
