# RoboView Constitution

**Version:** 0.3.2 · **Ratified:** 2026-09-02 · **Amended:** 2026-09-02 · **Status:** Normative

**Languages:** English · [中文](CONSTITUTION.zh-CN.md)

This document is the binding set of development standards for the RoboView project.
Every commit, pull request, code change, and document **must** comply with it.
When this document conflicts with any other instruction or habit, this document wins
(except security or legal requirements).

The Chinese translation lives in [`CONSTITUTION.zh-CN.md`](CONSTITUTION.zh-CN.md);
the English version above is always the governing text.

---

## 0. Project Identity

| Item | Value |
|---|---|
| Name | RoboView (crate: `roboview`) |
| Vision | A cross-platform 3D data visualization tool for robotics and AI data, built in Rust |
| Platform | Cross-platform desktop: macOS / Windows / Linux |
| Language | English (canonical) + Chinese (secondary, mirror files) |
| Versioning | Semantic Versioning (SemVer) |
| Repo | Single git repository, linear history on `main` |
| License | MIT OR Apache-2.0, dual license (ADR 005) |

---

## 1. Language Policy

1.1 **English is the canonical language** of this project. RoboView is an international
project: contributors and consumers of all languages must be able to read everything.

1.2 Chinese (Simplified) is the **secondary** language. It appears only where it serves
Chinese-speaking contributors, and always as a translation of the English original.

1.3 **English-only, not negotiable:**

- Code identifiers, types, and function names
- Code comments and `rustdoc` documentation
- Commit messages (see §3)
- Issue, PR, and discussion titles and bodies
- All technical documentation in `docs/` (except `docs/specs/`, see §1.9)
- Build scripts, CI configuration comments, and tooling

1.4 **Chinese is permitted in:**

- The mirror documents (`README.zh-CN.md`, `CONSTITUTION.zh-CN.md`, etc.)
- Chat and informal discussion among Chinese-speaking contributors

1.5 **Bilingual layout — two zones, by design** (operational detail: ADR 001):

- **Root level** (`README.md`, `CONSTITUTION.md`): mirror files — `foo.md` is
  English and canonical, `foo.zh-CN.md` is its 1:1 translation beside it. Root
  files are bound to the root by tooling and can only use this layout.
- **`docs/` and every other document tree planned to grow multi-language:**
  per-language directory trees. English is the default at the top of the tree
  (`docs/plans/...`); each other language is one directory mirroring the same
  structure (`docs/zh-CN/plans/...`). The `.zh-CN.md` suffix is reserved for
  root-level files and must not be used inside `docs/`.

1.6 Never interleave English and Chinese paragraphs in a single file. Technical terms,
path names, and code fragments inside Chinese text stay verbatim in backticks —
they are quoted, not translated.

1.7 When the Chinese text and the English text disagree, the **English text governs**.
Translations must be complete and kept in sync with the English original;
a translation that lags behind is still subordinate. In `docs/`, a language tree
may be sparse — a document without a translation simply does not exist there — but
any file that does exist must be a complete, current 1:1 translation.

1.8 Language-switcher lines in bilingual index files list the available languages
without a self-link: the current language appears as plain text, and only the
other languages are linked. Example:
`**Languages:** English · [中文](README.zh-CN.md)`.

1.9 Exception — **`docs/specs/` is a Chinese workspace** (ADR 004). Feature
specs are internal working documents for the team, not published material:
they are written in Chinese, need no English version and no mirror, and the
1:1 requirement of §1.7 does not apply inside this tree. Everything else in
`docs/` stays English per §1.3.

---

## 2. Rust Development Standards

### 2.1 Toolchain

- Use the **latest stable** Rust toolchain; the crate uses **edition 2024**.
- Declare `rust-version` in `Cargo.toml` and refresh it deliberately — current MSRV is 1.85+.
- `rustc`, `rustfmt`, and `clippy` are the baseline tools; all three must pass on every change.
- Do not use nightly-only features without a documented rationale in the pull request.

### 2.2 Formatting

2.2.1 Run `cargo fmt` on every change; the CI runs `cargo fmt --check` as a hard gate.

2.2.2 Keep rustfmt defaults — do not add a custom `rustfmt.toml` except on a documented
decision. 100-column width, 4-space indent, trailing commas, standard import grouping.

2.2.3 Never commit code that reformats files unrelated to the change.

### 2.3 Lints

2.3.1 `cargo clippy --workspace --all-targets -- -D warnings` must pass in CI.

2.3.2 Never silence a lint project-wide. If a lint is false-positive, add a scoped
`#[allow(...)]` at the smallest enclosing item with a one-line comment stating why.

2.3.3 Prefer compiler lints and Clippy's default set; enabling extra lint groups is a
per-module decision, documented in the commit message.

### 2.4 Code Organization

2.4.1 Layered architecture: separate **core** (rendering, scene graph, math, IO — no GUI
dependencies) from **app** (GUI, platform shell) from the start.

2.4.2 Cargo workspace, adopted 2026-09-02 (plan:
`docs/plans/2026-09-02-workspace-split.md`). The repository root is a virtual
workspace manifest; the members are:

| Crate | Kind | Purpose |
|---|---|---|
| `roboview-core` | library | Rendering core, scene graph, math, IO, display-type traits. No GUI dependencies. |
| `roboview` | binary | Desktop app: GUI shell, UI panels, platform integration. |

- Member directories live under `crates/` at the repository root
  (`crates/roboview/`, `crates/roboview-core/`).
- The workspace root has no `[package]`; `default-members` is `roboview`, so
  `cargo run` at the root launches the app.
- Dependency direction is one-way: `roboview` depends on `roboview-core`; the
  core crate never depends on the app and never pulls GUI crates.
- New crates (e.g., display-type plugins `roboview-displays-*`) join the
  workspace, each carrying its own `assets/` (ADR 003).

2.4.3 Modules are feature-oriented, not type-oriented (e.g., `scene/`, `render/`,
`io/`, `ui/`), and each module owns its main types and error types.

2.4.4 Public API surface is small, stable, and tested; the core layer is a library that
must compile without the GUI feature set.

### 2.5 Error Handling

2.5.1 Library code: define typed errors with `thiserror` (`enum` + `#[derive(Error)]`).

2.5.2 Binary/entrypoint code: use `anyhow` for context-rich error propagation; the GUI
surface converts library errors into user-visible messages.

2.5.3 Never swallow errors silently. If an error is intentionally ignored, attach a
comment giving the reason, or prefer `?` with a contextual `.context(...)`.

2.5.4 Avoid `unwrap()`/`expect()` in library code reachable from public APIs. For
program-invariant failures, use `unreachable!` with an explanatory message. If a
`unwrap` is genuinely unavoidable, write a comment stating the invariant that makes it safe.

### 2.6 Unsafe Code

2.6.1 Write `unsafe` only when necessary — first look for a safe alternative.

2.6.2 Every `unsafe` block carries a `// SAFETY:` comment above it stating precisely which
invariants the code upholds.

2.6.3 Enable `#![deny(unsafe_op_in_unsafe_fn)]` and keep `unsafe` blocks as small as
possible; prefer safe wrappers (newtypes) that encapsulate the invariants.

### 2.7 Naming & Style

2.7.1 Follow the Rust API Guidelines: `snake_case` for functions/variables, `CamelCase`
for types and enum variants, `SCREAMING_SNAKE_CASE` for constants, statics, and import
aliases. Clarity over brevity; prefer full words (`num_vertices`) over abbreviations
(`n_verts`) in public APIs.

2.7.2 Module and file names: `snake_case.rs`.

2.7.3 No `dbg!()` and no `todo!()`-style placeholder reachable in release paths — keep a
named issue open for any tracked deficit instead.

### 2.8 Dependencies

2.8.1 Keep the dependency graph minimal; every crate must have a demonstrated need at
addition time (state it in the commit message).

2.8.2 Commit `Cargo.lock` for this project (binary crate plus libraries that will ship).

2.8.3 All dependencies must be maintained, widely used, and license-compatible. CI runs
`cargo audit` and `cargo deny check` (licenses); any vulnerability blocks the merge.

2.8.4 Update dependencies in dedicated commits, not buried in feature commits.

### 2.9 Tests

2.9.1 Unit tests live in the same module (`#[cfg(test)] mod tests`); integration tests in
`tests/`; public examples in rustdoc become runnable doctests.

2.9.2 New non-trivial behavior ships with tests in the same commit — a "no test" change
must explain why in its commit message.

2.9.3 Tests must be deterministic: no wall-clock sleeps, no network, no port collisions
(the design provides injectable time and interfaces).

2.9.4 `cargo test --workspace --all-targets` is a CI gate.

### 2.10 Performance & Logging

2.10.1 Rendering and frame paths are performance-critical: avoid per-frame allocations,
measure before optimizing, and never optimize at the cost of clarity without data.

2.10.2 Use `tracing` (or `log`) for diagnostics — never `println!`/`eprintln!` in library
code; structured fields carry context; verbose spans are debug-only.

### 2.11 Assets

2.11.1 Resources live beside the crate that uses them: `<crate>/assets/`
(ADR 003). Engine-owned assets (shaders, core data) travel with
`roboview-core`; app-owned assets (icons, fonts, locale catalogs) travel with
`roboview`; each plugin crate carries its own `assets/`.

2.11.2 Assets that must always accompany the binary are embedded at build time
(`include_str!`/`include_bytes!`); large or user-replaceable data is loaded
from disk at runtime. Operational detail: ADR 003.

---

## 3. Git & Commit Standards

### 3.1 Commit Message Format

3.1.1 Follow **Conventional Commits 1.0.0**:

```
<type>(<scope>): <subject>
<BLANK LINE>
<body>
<BLANK LINE>
<footer>
```

3.1.2 **All commit messages are written in English.** Lowercase, no Chinese, no emojis.

3.1.3 Allowed `type`:

| Type | Meaning |
|---|---|
| `feat` | new feature |
| `fix` | bug fix |
| `docs` | documentation only |
| `style` | formatting, no behavior change |
| `refactor` | code change, no bug/feature change |
| `perf` | performance improvement |
| `test` | tests only |
| `build` | build system / dependencies |
| `ci` | CI configuration |
| `chore` | maintenance, no production code change |
| `revert` | revert a commit |

3.1.4 `scope`: the module or area affected, kebab-case and lowercase, e.g. `renderer`,
`scene`, `view`, `io`, `ci`, `docs`. Omit when no module is identifiable.

3.1.5 `subject`: imperative present tense ("add", "fix", "remove" — never "added" or
"fixes"), no leading capital, no trailing period, **≤ 72 characters**.

3.1.6 `body`: optional; explains **why** the change happened, one paragraph per line,
wrapped at 72 characters; complete sentences.

3.1.7 `footer`: reserved for `BREAKING CHANGE: <description>` — breaking changes MUST be
marked — and reference footers (`Fixes #12`).

### 3.2 Commit Discipline

3.2.1 **One logical change per commit.** Each commit must compile and keep the test
suite green; never commit unrelated changes together (use the staging area).

3.2.2 No WIP commits, no `fixup` commits pushed to shared branches, no `Merge branch` —
history stays linear and readable.

3.2.3 Commit early, commit often, with granular messages — but never a commit that breaks
the build.

3.2.4 Never commit generated artifacts (`target/`, logs, screenshots, local config);
`git status` must be clean of them.

### 3.3 Examples

Correct:

```text
feat(renderer): add point cloud rendering pipeline

Add GPU mesh handling for point cloud entities, including buffer
management and color strategies. Initial support for RGB colors.

CI: renderer tests cover 16M-point workload smoke cases.
```

```text
fix(view): correct camera projection matrix

The near/far depth range was signed when the projection used an
OpenGL-style depth clip; remap to normalized depth range so the
shapes near the far plane do not z-fight.

BREAKING CHANGE: projection ordering now matches the z-prepass pass.
```

Forbidden:

```text
add point cloud stuff          # missing type, vague
FIXED camera bug!!!            # uppercase, non-imperative, emojis
feat: implement many things    # vague subject, no scope
```

### 3.4 Branch & Workflow

3.4.1 Branch naming: `<type>/<short-kebab-desc>` — `feat/point-cloud`,
`fix/camera-projection`, `docs/design-proposals`, `refactor/error-model`.

3.4.2 Work on short-lived feature branches forked from `main`; `main` is always
buildable and releasable.

3.4.3 Merge via **squash merge** with a Conventional Commits title; rebase your branch
onto `main` before review/merge. All commits must pass CI before merge.

3.4.4 Issues and PRs are written and discussed in English.

3.4.5 Tags follow SemVer: `v0.1.0`, `v1.4.2` — only on `main` tip commits.

---

## 4. Documentation Standards

### 4.1 Location & Structure

4.1.1 `README.md` is the project front door — English canonical — with a full Chinese
mirror at `README.zh-CN.md` (§1.5).

4.1.2 **All design plans and technical proposals live in `docs/`** — not in the root, not
in chat archives. The constitution is the exception, kept at the root. `docs/` is
exclusively engineering documentation; a user-facing documentation site, when it is
introduced, lives in `site/` at the repository root (ADR 002). `docs/` has two levels:

- `docs/plans/` — **project-level** plans: governance changes, milestones, and
  architecture direction (English, with `zh-CN` mirrors).
- `docs/specs/<feature-id>/` — **feature-level** SDD workspace for a single
  piece of work: `spec.md` (WHAT), `plan.md` (HOW), `tasks.md` (atomic tasks),
  written in Chinese with no mirror (§1.9, ADR 004).

4.1.3 Structural conventions:

```
docs/
  README.md        # index + conventions for this folder
  plans/           # project-level plans (governance, milestones, direction)
  specs/           # feature-level SDD workspace (Chinese, ADR 004)
  design/          # architecture & detailed design documents
  decisions/       # ADRs: NNN-title.md, numbered, immutable once approved
```

4.1.4 File naming: kebab-case (`point-cloud-rendering.md`). ADRs: zero-padded numbers
(`001-layered-architecture.md`). Chinese mirrors live in the `docs/zh-CN/` tree at
the same relative path (`docs/zh-CN/decisions/002-gpu-backend.md`) — the
`.zh-CN.md` suffix applies to root-level files only (§1.5).

### 4.2 Content Rules

4.2.1 Every plan/design document opens with a header — Title, Status
(`Draft | In Review | Approved | Superseded | Rejected`), Date. Approved status is
normative; a `Superseded` document is kept for history.

4.2.2 Write documents assuming the reader knows nothing: state the problem, the options,
the decision, and the rationale (why not the alternatives).

4.2.3 Documents are written in English per §1.3; the Chinese mirror is written in
Chinese per §1.5. A commit that changes code behavior without documenting the design
in `docs/` (when the change is design-significant) is incomplete.

4.2.4 **Documents describe RoboView only.** No provenance, inspiration, or comparison
statements about external projects, tools, or platforms, and no record of internal
discussion, design conversation, or authoring process. Documents carry the confirmed
decision and its rationale — never how it was reached. In particular, project chat,
prompts, and negotiation are private and never become repository content, except for
files explicitly created to hold them (see the private, git-ignored conversation
archive used by the project owner).

---

## 5. Versioning & Release

5.1 Semantic Versioning (SemVer): `MAJOR.MINOR.PATCH`. While the library API is
unstable (`0.x`), any minor release may break; notable breakage is still called out.

5.2 `CHANGELOG.md` is introduced with the first release and then maintained at the root,
grouped by release version, structured by Conventional Commit types. Until the first
release, the file need not exist.

5.3 Release procedure: bump version → `CHANGELOG` entry → tag `vX.Y.Z` on `main` →
CI builds release artifacts. Releases happen from `main` only.

---

## 6. Workflow & Review

6.1 **Design before code:** anything non-trivial starts with a proposal in
`docs/plans/` (§4.1.2) and goes `Draft → In Review → Approved`; implementation starts
only after approval (or after a documented exception). A concrete feature
follows the SDD workflow in `docs/specs/<feature-id>/` — spec → plan → tasks →
implement → validate (ADR 004); a feature that reaches shared architecture
still earns its own project-level plan or ADR.

6.2 Every change is a small PR with a clear title, a description stating what and why,
and a link to the related plan/ADR when one exists.

6.3 At least one approval is required before merge; the author applies review feedback
in follow-up commits (never force-push a shared history).

6.4 CI gates — all mandatory: `cargo fmt --check`,
`cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace --all-targets`, `cargo deny check`, `cargo audit`.

6.5 Anything that ships to users must respect the language policy (§1) — UI copy is
English-first with i18n-ready structure from day one (this is an international project).

---

## 7. Enforcement

7.1 The constitution is reviewed at each milestone and amended via the same
`docs/plans/` process; every amendment bumps the version and is recorded in
`CHANGELOG.md` once that file exists. Before the first release, the amendment
is recorded in the plan document that carries it.

7.2 Reviewers are expected to cite the constitution in review comments when standards
are violated (`CONSTITUTION §2.3`).

7.3 No rule is waived by exception without a written note in the PR that is later
folded back into this document.
