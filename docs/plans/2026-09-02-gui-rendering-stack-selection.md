# GUI and Rendering Stack Selection

Status: Approved

Date: 2026-09-02

Related: ADR 006; CONSTITUTION §2.4.1–2.4.4

## Context

The repository needs a stack decision for the visualization engine: cross-platform
desktop (macOS / Windows / Linux), GPU acceleration by design, and a rendered
domain of point clouds, grids, paths, frames, markers and computation graphs.
The stack must not disturb the layered architecture: the core library stays
GUI-free (§2.4.1). This plan records the assessment and the chosen shape; the
binding record is ADR 006.

## Candidates assessed

| Shape | Assessment |
|---|---|
| General-purpose engine (app/plugin/ECS shell) | Fastest road to rich rendering macros, but the core would inherit engine machinery beyond rendering (windowing, scheduling, asset pipeline), breaking §2.4.1; scene model targets game scenes, not batch-heavy data displays; expect repeated breaking 0.x reworks and a heavy dependency graph. |
| **Composable parts: GPU API + math + immediate-mode GUI + own rendering core** | **Adopted.** Clean layering (render/math/IO in core, GUI in app), domain fit (custom display-type traits), moderate dependency count, conservative API evolution, permissive licenses matching ADR 005. |
| Ready-made viewer platform as a dependency | Least implementation work; but its data model and APIs are product-specific and unstable, and the custom rendering core is this project's core value. |

## Decision

- Stack: `wgpu` (GPU backend for all three platforms), `glam` + `bytemuck`
  (math, GPU data), `eframe` wgpu backend + `egui`/`egui-wgpu`/`egui_dock`/
  `egui_plot` (app shell and panels), our own rendering core inside
  `roboview-core` (`render/`, `scene/`, `displays/`).
- Dependency direction: app → core only; the core builds headless and its
  module tree mirrors future crate boundaries.
- No feature development starts from this decision. The first SDD feature
  (`docs/specs/`) will exercise the stack end to end (window, GPU batch,
  camera) before deeper display types are built.
- Per-license check: the `deny.toml` allowlist covers the chosen crates.

## Consequences

- ADR 006 records the binding decision.
- Next design step after this plan: the first feature spec (SDD workspace)
  around a single GPU-exercised viewport slice.
