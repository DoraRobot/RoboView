# 006 — Rendering and GUI Stack

Status: Approved

Date: 2026-09-02

Supersedes: none

Related: CONSTITUTION §2.4.1–2.4.4, §2.8; plan `docs/plans/2026-09-02-gui-rendering-stack-selection.md`

## Context

RoboView is a cross-platform desktop tool (macOS / Windows / Linux) with GPU
acceleration for robot and AI data: point clouds, grids, paths, frames,
markers, and computation graphs. Beyond those requirements, the stack must
respect the layered architecture (§2.4.1): `roboview-core` is a GUI-free
library and must build headless. Its dependency graph is what this decision
fixes.

Three candidate shapes were assessed:

- **A general-purpose engine** (app/plugin/ECS shell with scene management).
  Rich rendering out of the box, but the core layer would depend on engine
  machinery beyond rendering (windowing, scheduling, assets) — breaking the
  GUI-free boundary — and the engine's scene model does not match a
  data-visualization domain dominated by large batch display types.
- **Composable parts** — a modern GPU API + math library + an immediate-mode
  GUI, plus a rendering core of our own.
- **A ready-made viewer platform as a dependency** — the closest product-shaped
  analog. Least work, but its data model and APIs are product-specific and
  not stable, and owning the rendering core is this project's main value.

## Decision

- **`roboview-core`** (GUI-free) depends on:
  - `wgpu` — the unified GPU backend across platforms (and the road to
    off-screen rendering),
  - `glam` and `bytemuck` — math and GPU-friendly data layout,
  - our own rendering internals: scene, geometry buffers, shader management,
    picking, depth sorting (modules `render/`, `scene/`, `displays/`).
- **`roboview`** (app) depends on:
  - `eframe` with its GPU backend + `egui-wgpu`,
  - `egui` panels, `egui_dock` (panel docking), `egui_plot` (2D plots),
  - plugin loading and the platform shell.
- Dependency direction stays one-way: app → core. Core must build and run
  headless; the renderer is structured so an off-screen path is possible.
- Every chosen crate is permissive-licensed and compatible with ADR 005; the
  allowlist in `deny.toml` reflects this stack.
- This decision pins the stack only; no feature work starts from it. The first
  feature spec (SDD, `docs/specs/`) exercises the full slice: window, GPU
  batch, math.

## Rules

1. GUI crates (`eframe`/`egui` family) belong to the `roboview` crate only —
   never into `roboview-core`.
2. Any new core dependency states its demonstrated need in the commit message
   (§2.8.1) and stays permissive-licensed (§2.8.3).
3. Cross-platform means the CI platform matrix (this operating system family
   of runners) stays green on every change.

## Consequences

- The first dev cycle on `crates/roboview-core` starts with render/scene
  foundations over `wgpu` + `glam`.
- If the rendering core ever grows large, it can be split into its own crate
  without touching the app boundary (see CONSTITUTION §2.4.2).
