# UI Feature Roadmap — Phases 004–009 (Registered)

Status: Approved

Date: 2026-09-03

Related: CONSTITUTION §1.9, §4.1; ADR 004, ADR 006; `docs/specs/001-point-cloud-viewport/` … `docs/specs/007-interaction-polish/`

## Scope

Records the UI feature sequence that follows the closed infrastructure phases
(001 point-cloud-viewport, 002 display-types closed; 003 i18n-system-fonts
implemented, full-matrix CI acceptance pending) and the registered UI phases.
Phase specs live in `docs/specs/NNN-name/`; feature-level detail, review results
and acceptance claims are governed there — this document records sequence and
dependency edges only.

## Sequence and Dependencies

| Phase | Feature | Status | Dependencies |
|---|---|---|---|
| 004 | ui-blueprint — four-zone skeleton (tree/viewport/properties/status), main menu bar (macOS native via muda + in-window fallback), per-object appearance uniform, viewport auxiliary layer (ground grid, orientation gizmo), camera math | Draft (reviewed; D1–D5 resolved) | 002, 003 |
| 005 | picking-selection — CPU picking, three-zone selection mirror, F/Delete (focus-gated) | Draft (reviewed; decisions resolved) | 004 (selection semantics + highlight uniform channel) |
| 006 | dock-layout — dockable panels, layout persistence (eframe storage), 3 presets, panel registry | Draft (reviewed; decisions resolved) | 004 (fixed skeleton), ADR 006 |
| 007 | interaction-polish — shortcuts (dual-path), context menus, DragValue spec, icons (egui-phosphor 0.10.0), message center (supersedes 003 error window), HUD expansion | Draft (reviewed; decisions resolved) | 004, 005, 006 |
| **008** | **object-transform** — move/rotate/scale gizmos and drag transforms | **Draft (spec created 2026-09-03)** | **005 selection semantics** (selection = one subject across tree/viewport/properties; transform commands target the selected object); **004 per-object appearance uniform channel** (gizmo handles and affected-object state build on the same per-object rendering state) |
| **009** | **timeline** — scrub / playback panel | **Draft (spec created 2026-09-03)** | **006 dockable/reserved-panel mechanism** (panel registry, multi-surface floating, layout persistence) to host the timeline panel |

Implementation order: 004 → 005 → 006 → 007 → 008 → 009.

## Far-term (not scheduled)

- Multi-scene / view layers (v2)
- Command palette / plugin-based UI extensions (future evaluation)

## Notes

- 008/009 were confirmed by the owner on 2026-09-03 following the gap analysis
  against mature 3D tool conventions (008 registered in the 005 non-goals as
  "路线图序号 008"; 009 in the 004 non-goals as "路线图序号 009");
  spec drafts created the same day at `docs/specs/008-object-transform/` and
  `docs/specs/009-timeline/` (Draft, decision points open).
- Each phase follows the SDD workflow: spec → plan → tasks; four-lens review
  before approval; wave-parallel implementation split by minimal functional
  points.
