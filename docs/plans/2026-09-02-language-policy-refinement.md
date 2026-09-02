# Language Policy Refinement — Switcher Convention and Clause Format

Status: Approved

Date: 2026-09-02

Related: CONSTITUTION §1 (amended), §2–§7 (formatting)

## Context

Two issues surfaced in the constitution after the first amendments:

1. **Switcher convention.** Bilingual index files (READMEs) list the available
   languages without a self-link: the current language is plain text, only the
   other languages are linked. This convention is already in use in the
   repository but is not stated as a rule in §1.
2. **Clause rendering.** Consecutive numbered clauses with no blank line
   between them merge into a single paragraph in the preview. Every clause
   must render as its own block.

## Decision

- Add §1.8: language-switcher lines in bilingual index files carry no
  self-link; the current language is plain text, other languages are linked.
- The constitution's own header gains a switcher line in both languages.
- Standardize clause formatting across §1–§7: every numbered clause is its
  own block (blank line between clauses); bullet lists stay as lists.

## Constitution amendment

- §1.8 added; §5.2/§7.1 references to the repository convention unchanged;
  clause formatting standardized; header version bumped 0.2.1 → 0.2.2.
