# 004 — SDD Specs Workspace (Chinese Work Documents)

Status: Approved

Date: 2026-09-02

Supersedes: none

Related: CONSTITUTION §1.9, §4.1.2–4.1.3, §6.1; ADR 001; CONSTITUTION §4.2.4

## Context

RoboView adopts Spec-Driven Development (SDD) as its default implementation
workflow (plan: `docs/plans/2026-09-02-sdd-workflow.md`). SDD produces three
documents per feature in a dedicated workspace: a spec (WHAT), a plan (HOW),
and an atomic task list. The repository already has `docs/plans/`, but those
documents are project-level: governance changes, milestones, architecture
direction. A single feature's spec/plan/tasks do not belong there.

SDD's working documents are written for the team, in Chinese. That is a
deliberate exception to §1.3/§1.5 (English canonical, language trees): these
are internal working artifacts, and the team's working language here is
Chinese. They are not published material.

SDD also leaves execution traces (`debug/`, prompts, logs). Those are process
records and must never enter the repository (§4.2.4).

## Decision

- **`docs/specs/<feature-id>/`** is the feature-level SDD workspace, holding
  `spec.md` (WHAT: problem, success metrics, user stories, acceptance
  criteria, non-goals, constraints), `plan.md` (HOW: architecture, modules,
  interfaces), and `tasks.md` (independently verifiable atomic tasks).
- **Written in Chinese, no mirror.** No English version, no `docs/zh-CN/specs/`
  tree — this is the only exception to the bilingual rules (§1.9); the 1:1
  requirement of §1.7 does not apply inside this tree.
- **Project-level plans stay in `docs/plans/`** — English with a `zh-CN`
  mirror, as before. The two levels are distinct and do not merge.
- **Execution traces stay private:** prompts, `agent.log`, `debug/` content
  remain in the private archive (`.leon/`, git-ignored), never in `docs/specs/`
  or anywhere else in the repository (§4.2.4).
- **Escalation:** a feature whose design becomes architecture-level public
  design moves to `docs/design/` as an English document (with mirror),
  alongside its ADR where applicable.

## Rules

1. `docs/specs/<feature-id>/` contains exactly `spec.md`, `plan.md`,
   `tasks.md`; the id is kebab-case (`point-cloud-renderer`).
2. Spec quality gates: the six elements are present, success metrics are
   testable, non-goals are explicit, and the "granularity test" passes
   (the spec survives a technology-stack swap). The spec is reviewed before
   implementation starts (§6.1).
3. Tasks in `tasks.md` are atomic and independently verifiable; completion
   status lives here, execution traces stay in `.leon/` (Rule above).
4. This workspace is exempt from the language trees (§1.5, ADR 001) and their
   completeness checks.

## Consequences

- `docs/plans/` keeps its project-level role; `docs/specs/` is the feature
  level. No existing document moves.
- CONSTITUTION §1.9, §4.1.2, §4.1.3, §6.1 reference this ADR.
