# 003 — Asset Location

Status: Approved

Date: 2026-09-02 (revised 2026-09-02)

Supersedes: none

Related: CONSTITUTION.md §2.4.2 (workspace, layered architecture), §2.8 (dependencies), §2.11 (assets)

## Context

RoboView ships several kinds of embedded or bundled resources: icons and images,
fonts, shader files, UI locale catalogs (i18n), and demo/sample data. The question
is where these files live in the repository, given that the project is now a
Cargo workspace (`roboview-core` library + `roboview` binary, §2.4.2) and may
later gain plugins.

## Candidates

| Candidate | Assessment |
|---|---|
| **`assets/` beside the crate** | Preferred. Assets live inside the crate tree, so the rule holds unchanged as crates are added or plugins appear. |
| One shared root `assets/` forever | Ambiguous ownership: core shaders vs. app icons vs. plugin resources all in one directory. Plugins will need their own resources and cannot live in a shared pool. |
| Assets inside `src/` | Mixes non-code artifacts with the source tree; packaging and embedding paths become unclear; shaders and locale catalogs are not Rust source. |

## Decision

**Assets live beside the crate that uses them:** `<crate>/assets/`.

- Now (workspace): engine-owned assets (shaders, core data) live in
  `crates/roboview-core/assets/`; app-owned assets (icons, fonts, locales) live
  in `crates/roboview/assets/`; each plugin crate carries its own `assets/`.

Naming: `assets/`, not `resources/` — in graphics programming, "resource" is already
taken by GPU resource concepts (e.g. wgpu), and the Qt-style "resources" naming
hides the runtime semantics.

## Rules

1. Category subdirectories below the `assets/` root: `icons/`, `fonts/`,
   `shaders/`, `locales/`, `demo/` — extend when a category actually appears.
2. **Embedding policy.** Assets that must always accompany the binary (icons,
   bundled fonts, small shaders, locale catalogs) are embedded at build time
   (`include_str!` / `include_bytes!`, or `rust-embed` once the catalog grows).
   Large or user-replaceable data (demo/sample datasets) is loaded from disk at
   runtime and is never embedded.
3. **Single source of truth.** A resource referenced by code exists in exactly one
   location; never copy an asset for reuse — copies break packaging.
4. Rust example binaries follow the standard `examples/` crate convention; their
   data lives in `assets/demo/`, not in `examples/`.
5. Locale catalog directories follow the BCP-47 naming used elsewhere in this
   repository (ADR 001).
6. Keep committed binaries small. If a demo dataset must exceed a few megabytes,
   evaluate git-lfs as a deliberate, documented decision rather than committing
   large blobs silently.

## Consequences

- A crate's `assets/` directory is created when that crate's first asset is
  added (no `.gitkeep` either until then).
- Constitution §2.11 (Assets) references this ADR (added 2026-09-02).
