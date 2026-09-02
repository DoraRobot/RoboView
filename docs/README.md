# Documentation

Index and conventions for the `docs/` directory.
Binding rules live in [`CONSTITUTION.md`](../CONSTITUTION.md) §4 — this file is the
practical handbook.

**Languages:** English · [Chinese](zh-CN/README.md)

## Layout

English is the default at the top of the tree. Every other language is one
directory mirroring the same structure ([ADR 001](decisions/001-doc-localization-layout.md)):

| Directory | Purpose |
|---|---|
| `plans/` | Project-level plans: governance changes, milestones, architecture direction |
| `specs/` | Feature-level SDD workspace — Chinese work documents, no mirror (ADR 004) |
| `design/` | Architecture & detailed design documents |
| `decisions/` | ADRs: `NNN-title.md`, numbered, immutable once approved |
| `zh-CN/` | Chinese language tree — mirror of the above, only translated docs |
| `README.md` | This index |

## Conventions

- **English is the canonical language** for all documents here
  (`CONSTITUTION.md` §1.3). A Chinese document is the same relative path inside
  `docs/zh-CN/` (§1.5). Language content is never interleaved in one file (§1.6).
- File naming: kebab-case, e.g. `point-cloud-rendering.md`. ADRs are zero-padded:
  `001-layered-architecture.md`, `002-gpu-backend.md`.
- Every document opens with a header: Title, Status, Date.

  ```
  # <Title>

  Status: Draft | In Review | Approved | Superseded | Rejected

  Date: 2026-09-02
  ```

- Status is normative: nothing is implemented before `Approved`
  (`CONSTITUTION.md` §6.1).
- Write for a stranger: state the problem, the options, the decision, and the rationale.
- Code changes that are design-significant must link the corresponding document from
  the PR description.
- Every document in `docs/` ships with its `zh-CN` translation at the same
  relative path (sparse trees are allowed, §1.7; this project keeps
  translations current). `docs/specs/` is exempt (ADR 004).
- Cross-workspace references are fully qualified: `001-point-cloud-viewport
  spec.md A7`; a bare `A7` is local to the current workspace only.
- SDD feature workspaces are named `NNN-<kebab-name>` with a zero-padded
  3-digit sequence (`001-`, `002-`, …) in creation order; sequences stay
  unique and incrementing, old workspaces keep their number.
- SDD workspace headers (spec/plan/tasks) update their status together on
  every status transition.

## Languages

- Supported: `zh-CN` (Chinese). A language tree may be sparse — documents without
  a translation simply do not exist there.
- Language-switcher lines carry no self-link for the current language
  (CONSTITUTION §1.8).
- Adding a language: create `docs/<lang>/README.md` (BCP-47 name) and mirror the
  structure for whatever is translated.

## Process (short form)

1. Open `plans/YYYY-MM-DD-<topic>.md` as `Draft`.
2. Discuss; flip to `In Review`.
3. Approved → implement; the plan stays as the record.
4. Deep architectural choices get an ADR in `decisions/`.
