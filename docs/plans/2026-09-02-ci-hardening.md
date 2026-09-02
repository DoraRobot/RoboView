# CI Hardening and Contributor Tooling

Status: Approved

Date: 2026-09-02

Related: CONSTITUTION §2.8.3, §6.4 (amended); ADR 005

## Context

The gap review against mature repositories found the foundation CI lacking:
it ran on a single platform although RoboView is cross-platform desktop
(§0); the declared MSRV (1.85) was unverified; the licensing policy
(§2.8.3, ADR 005) had no machine enforcement; rustdoc warnings were
unguarded; line-ending and editor defaults were unnormalized; a community
code of conduct and a contributor editor experience were missing.

## Decision

- **CI jobs added:** test matrix on linux/macos/windows runners; MSRV job on
  1.85 (`cargo check`); docs job (`RUSTDOCFLAGS="-D warnings" cargo doc`);
  license check via `cargo-deny` (`deny.toml` at the root, permissive
  allowlist aligned with ADR 005; unlicensed and forbidden licenses deny,
  wildcard deps deny). The `cargo audit` job is kept.
- **Constitution:** §2.8.3 names `cargo deny check` alongside `cargo audit`;
  §6.4 adopts `cargo deny check` as a mandatory gate. Version 0.3.1 → 0.3.2.
- **Community:** Contributor Covenant v2.1 at the root with a `zh-CN` mirror;
  linked from CONTRIBUTING (EN + zh).
- **Editor normalization:** `.editorconfig`, `.gitattributes` (LF in repo,
  CRLF for `.bat`), `.vscode/extensions.json` recommending rust-analyzer.
- **Entrances synced:** README (EN + zh) and CONTRIBUTING (EN + zh) now list
  the five gates; README layout trees list the new root files.

## Constitution amendment

- §2.8.3, §6.4 amended for the deny gate; version 0.3.1 → 0.3.2.
