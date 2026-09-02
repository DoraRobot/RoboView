# Adopt the SDD Workflow for Feature Implementation

Status: Approved

Date: 2026-09-02

Related: ADR 004; CONSTITUTION §1.9, §4.1.2–4.1.3, §6.1

## Context

The repository adopts Spec-Driven Development (SDD) as its default
implementation workflow: four phases (Specify → Plan → Implement → Validate),
with three documents per feature in a dedicated workspace
(`spec.md`/`plan.md`/`tasks.md`). The existing `docs/plans/` stays
project-level (governance, milestones, architecture direction); a small
feature's spec must not share that folder — the two levels are different
granularities and must stay separate.

## Decision

- Feature-level SDD workspace: `docs/specs/<feature-id>/` with `spec.md`
  (six elements, testable success metrics, explicit non-goals, granularity
  test), `plan.md`, `tasks.md`.
- The workspace is written in Chinese and carries no mirror — the single
  explicit exception to the bilingual rules (§1.9, ADR 004). Project-level
  `docs/plans/` stays English with `zh-CN` mirrors.
- Execution traces (prompts, `agent.log`, `debug/`) stay in the private
  archive (`.leon/`), never in the repository (§4.2.4).
- Spec is reviewed before implementation; Validate phase combines automated
  tests and human review (CONSTITUTION §6.1).
- Governance is amended: CONSTITUTION §1.3 (exemption note), new §1.9,
  §4.1.2/§4.1.3 (two-level layout), §6.1 (workflow reference); version
  0.2.4 → 0.3.0. ADR 004 records the decision.

## Constitution amendment

- §1.3 exemption note; §1.9 added (Chinese specs workspace);
  §4.1.2/§4.1.3 restructured for the two levels; §6.1 references the SDD
  workflow; version bumped 0.2.4 → 0.3.0.
