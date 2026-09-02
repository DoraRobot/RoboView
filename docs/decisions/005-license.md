# 005 — License: MIT OR Apache-2.0

Status: Approved

Date: 2026-09-02

Supersedes: none

Related: CONSTITUTION §0, §2.8.3

## Context

The repository is public, but the README still declared the license
"To be decided". A public codebase without a license cannot be used by
anyone with certainty, contributions carry no clarity on how they may be
redistributed, and the downstream dependency surface (rendering stack,
GUI widget set) is dominated by permissive licenses — MIT/Apache-2.0
dependencies combine cleanly with a permissive project license, while a
single restrictive choice (e.g. copyleft, or a project-license mismatch
with the ecosystem's permissive core) creates friction for potential
adopters and for the project's own dependency policy (§2.8.3).

The dual MIT + Apache-2.0 pattern is the common shape of the surrounding
ecosystem: both licenses are short, well-understood, and permissive; the
combination lets adopters pick according to their own constraints and
keeps the codebases mutually interoperable.

## Candidates

| Candidate | Assessment |
|---|---|
| **Dual MIT OR Apache-2.0** | Preferred. Ecosystem-standard, permissive, de facto compatible with the project's expected dependency set; contributors can be asked for either. |
| MIT only | Fine, but Apache-2.0's explicit patent grant adds value in a software-heavy field; dual is the established shape. |
| Apache-2.0 only | Acceptable; slightly less standard than the dual shape in this ecosystem. |
| Copyleft (GPL-family) | Rejected: conflicts with the expected permissive dependency set and narrows adoption without a legal need. |

## Decision

- RoboView is **dual-licensed under MIT OR Apache-2.0** (SPDX: `MIT OR Apache-2.0`).
- License texts live at the repository root as `LICENSE-MIT` and `LICENSE-APACHE`;
  the README License section states the dual grant and points at both files.
- All Cargo manifests declare `license = "MIT OR Apache-2.0"` (through
  `[workspace.package]`), so published crates carry the SPDX identifier.
- Copyright holder for the project texts: *RoboView contributors* (year 2026).
- Dependency policy (§2.8.3) is confirmed against this choice: permissive
  dependencies are the baseline; a copyleft dependency requires a written
  rationale in the commit message and is expected to stay exceptional.

## Rules

1. Any new dependency must be license-compatible with `MIT OR Apache-2.0`
   (§2.8.3); incompatibilities block the merge.
2. License texts are never edited once approved; a change of license leads
   through a new ADR.

## Consequences

- README License section replaced "To be decided" with the dual grant.
- CONSTITUTION §0 gains a License row; version 0.3.0 → 0.3.1.
- Cargo manifests declare the SPDX identifier; the binary crate is marked
  `publish = false` until otherwise decided.
