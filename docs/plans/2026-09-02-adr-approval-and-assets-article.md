# ADR Approval and Constitution Assets Article

Status: Approved

Date: 2026-09-02

Related: ADR 001; ADR 003; CONSTITUTION §1.5, §2 (amended)

## Context

ADR 001 (documentation localization layout) and ADR 003 (asset location) have
been operative since the first day — §1.5 cites ADR 001 as the operational
detail, and the workspace split applied ADR 003's per-crate asset rule — but
both remained formally Draft. ADR 003 also tied an Assets article in
CONSTITUTION §2 to its approval.

## Decision

- Approve ADR 001 and ADR 003 (Draft → Approved).
- Add §2.11 (Assets) to the constitution: resources live beside the crate that
  uses them (ADR 003) and follow the embed-at-build vs. load-from-disk policy.
- Finalize ADR 003's consequences line: the §2 article exists.

## Constitution amendment

- §2.11 added; header version bumped 0.2.3 → 0.2.4.
