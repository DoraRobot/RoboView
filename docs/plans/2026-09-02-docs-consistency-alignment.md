# Docs Consistency Alignment — Workspace Gates and Index Conventions

Status: Approved

Date: 2026-09-02

Related: CONSTITUTION §2.3.1, §2.9.4, §6.4 (amended); ADR 001; ADR 003 (zh mirror)

## Context

A full review of the repository documents after the workspace split surfaced
four discrepancies:

1. **CI gate commands were single-crate era.** `cargo clippy --all-targets ...`
   and `cargo test --all-targets` at the workspace root operate on the default
   members only, so `roboview-core` would escape both gates. `cargo fmt` and
   `cargo audit` are unaffected (fmt covers the whole workspace by default).
2. **ADR 003's Chinese mirror was stale.** It still described the single-crate,
   root-`assets/` situation while the English ADR had been revised for the
   workspace.
3. **The language-switcher convention (§1.8) was not restated in the `docs/`
   index conventions**; the Chinese index also lacked the `Languages` section
   entirely.
4. **ADR 001 had no reference to the no-self-link switcher rule (§1.8).**

## Decision

- Qualify the clippy/test gates with `--workspace` in CONSTITUTION §2.3.1,
  §2.9.4, §6.4 and in the `Gates` sections of both READMEs.
- Resync `docs/zh-CN/decisions/003` with the revised English ADR.
- Add the switcher rule to the `docs/` index `Languages` section (English and
  Chinese).
- Extend ADR 001 Rule 2 with the §1.8 reference (English and Chinese).

## Constitution amendment

- Gate commands corrected for the workspace; header version bumped
  0.2.2 → 0.2.3.
