# Workspace Split — Adopt the Cargo Workspace Crate Structure

Status: Approved

Date: 2026-09-02

Related: CONSTITUTION §2.4.1–2.4.4 (amended, 0.2.0); ADR 003

## Context

RoboView is committed to a layered architecture: a GUI-free core (rendering,
scene graph, math, IO) separated from the app (GUI, platform shell)
(CONSTITUTION §2.4.1). §2.4.2 named the target crates (`roboview-core`
library + `roboview` binary) and called for the workspace split at the
moment the separation demanded it. The repository today has no GUI stack
and almost no code: splitting now costs the move of a placeholder entry
point, while splitting later means retrofitting boundaries onto code that
will grow in two directions (GPU rendering vs. GUI shell) at full speed.

## Decision

Adopt the Cargo workspace now.

- Members: `roboview-core` (library) and `roboview` (binary).
- The workspace root `Cargo.toml` is virtual (no `[package]`);
  `default-members` is `roboview`, so `cargo run` at the root launches the app.
- Crate directories live under `crates/` at the repository root
  (`crates/roboview/`, `crates/roboview-core/`).
- Dependency direction is one-way: `roboview` → `roboview-core`; the core
  crate never depends on the app or on GUI crates.
- The core crate exposes its module tree from day one: `scene/`, `render/`,
  `io/`, `displays/`. Modules are feature-oriented (§2.4.3) and own their main
  types and error types.
- New crates (e.g., display-type plugins `roboview-displays-*`) join the
  workspace with their own `assets/` (ADR 003).

## Options considered

- **Split later, single crate in the meantime.** The move stays cheap only
  while the crate is nearly empty; the layered architecture is the project's
  stated invariant from day one (§2.4.1), so the split is a when, not an if.
  Rejected: doing it now costs one move of a placeholder, doing it later costs
  the migration of real code.
- **Flat crate directories** (`roboview/`, `roboview-core/` at the repository
  root). Shorter paths and a shallower layout drawing. Rejected: the plugin
  trajectory (`roboview-displays-*`) will add several crate directories;
  grouped under `crates/`, the repository root keeps only repository-level
  content (`docs/`, `site/`, tooling), and the member tree reads as one unit.
  The switch is nearly free now and its cost grows with the crate count.
- **Three crates now** (`roboview-core` + a separate data-schema/IO crate +
  `roboview`). The schema crate has no second consumer today: data and protocol
  types stay inside `roboview-core` until a headless consumer (CLI converter,
  off-screen renderer) appears.
- **Feature-gated single crate now** (`#[cfg(feature = "ui")]` keeps the core
  GUI-free). Rejected: the gate is a hand-rolled imitation of the boundary
  that must still be removed later; the workspace is the machine-enforced
  version of the same rule.

## Constitution amendment

- §2.4.2 rewritten for the adopted workspace; constitution version bumped
  0.1.0 → 0.2.0 (§7.1).
- Follow-up amendment, 0.2.1: §5.2 and §7.1 clarify that `CHANGELOG.md`
  exists from the first release onward; before that, amendment records live
  in the carrying plan document.
