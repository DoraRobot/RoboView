# License and Repository Infrastructure

Status: Approved

Date: 2026-09-02

Related: ADR 005; CONSTITUTION §0, §6.4

## Context

A review against the typical shape of mature public repositories found the
governance layer complete but the skeleton missing four foundation pieces:
a license (README still said "To be decided"), CI (constitution §6.4 mandates
four gates but nothing enforces them), a toolchain pin for local/CI
consistency, and the contributor-facing entry points (contributor guide,
security policy, PR/issue templates). Cargo manifests also carried no
publish metadata.

## Decision

- **License:** dual `MIT OR Apache-2.0` (ADR 005) — `LICENSE-MIT` +
  `LICENSE-APACHE`, SPDX via `[workspace.package]`; binary crate
  `publish = false`.
- **CI:** `.github/workflows/ci.yml` running the four constitution gates
  (`cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace --all-targets`, `cargo audit`).
- **Toolchain:** `rust-toolchain.toml` pinning `stable` with rustfmt + clippy
  components (constitution §2.1 baseline).
- **Contributor entry:** `CONTRIBUTING.md` (+ zh mirror) summarizing the
  constitution for contributors; `SECURITY.md` (+ zh mirror) for private
  vulnerability reporting; `.github/pull_request_template.md` enforcing the
  conventional-title/plan-link/gate checklist; issue templates for bugs and
  feature requests.
- **Manifest metadata:** license, description, keywords, categories and
  repository (`https://github.com/DoraRobot/RoboView`, added 2026-09-02) in
  `[workspace.package]`. An AtomGit mirror hosts the Chinese-facing copy.

## Constitution amendment

- §0 gains a License row (MIT OR Apache-2.0, ADR 005); version bumped
  0.3.0 → 0.3.1.
