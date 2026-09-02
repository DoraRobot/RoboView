# 001 — Documentation Localization Layout

Status: Approved

Date: 2026-09-02 (revised 2026-09-02)

Supersedes: none

Related: CONSTITUTION.md §1.5, §1.7, §1.8, §4.1

## Context

English is the canonical language of RoboView; Chinese (Simplified) is the second
language today, and more languages (ja, de, ...) are a planned trajectory. All
translations must exist as separate files (`CONSTITUTION.md` §1.5). Two layouts
were considered:

- **A — Side-by-side mirrors:** `foo.md` (English) with `foo.zh-CN.md` (Chinese)
  beside it in the same directory.
- **B — Language trees:** English is the default at the top of a document tree;
  every other language is one directory mirroring the same structure
  (`docs/zh-CN/...`, `docs/ja/...`).

An important constraint that splits the decision: repository front-door files
(`README.md`, `CONSTITUTION.md`) are bound to the repo root by tooling and by
long-standing convention — they can only ever be layout A.

## Evaluation (for `docs/`)

| Criterion | A — side-by-side | B — language trees |
|---|---|---|
| Many languages (planned) | Each language adds one file per document — contact point grows with every doc | One directory per language; adding a language is a tree, not an edit sweep |
| Docs-site routing (mdBook/mkdocs `/zh-CN/`) | Tooling must pair files by name | Native convention |
| Relative links inside a tree | Identical paths for both languages | Identical paths inside each language tree; links stay tree-internal |
| Discovery in repo browsing | Translations adjacent to the original | Translations grouped under `docs/<lang>/`, discoverability handled by the tree index |
| Translation completeness check (CI) | Shell loop over `.zh-CN.md` pairs | Tree-level `diff` of file lists between languages |
| English-only documents | Allowed; "no mirror" is obvious | Allowed; absence inside the language tree is self-explanatory |
| Migration cost if adopted later | — | High: every doc and every relative link across the tree must be revisited |

## Decision

**Hybrid, adopted now:**

- **Root level stays layout A** — `README.md` + `README.zh-CN.md`,
  `CONSTITUTION.md` + `CONSTITUTION.zh-CN.md`. Tooling-constrained; unchanged.
- **`docs/` uses layout B from day one** — English is the default at the top of
  the tree; other languages are directory trees:

```
docs/
  README.md            # English index & conventions; hosts the language switcher
  plans/               # English defaults (proposals & implementation plans)
  design/              # English defaults (architecture & detailed design)
  decisions/           # English defaults (ADRs)
  zh-CN/               # one tree per supported language
    README.md
    plans/
    design/
    decisions/
```

Rationale: multi-language is a committed direction, so the only question is
whether to pay the migration cost now or later — and the set is still two files.
Adopting layout B today costs one rename; deferring it costs tens of documents.

## Rules

1. Language directory names follow BCP-47: `zh-CN`, `ja`, `de`. English has no
   directory — it is the tree default.
2. Inside each language tree, relative links stay tree-internal (a Chinese
   document links Chinese documents). Cross-language navigation goes through the
   tree index / language switcher, which lists the current language without a
   self-link (§1.8).
3. Language trees may be sparse: an untranslated document is simply absent.
   Every file that does exist must be a complete, current 1:1 translation (§1.7).
4. The `.zh-CN.md` suffix convention is reserved for root-level files and must
   not be used inside `docs/` (and vice versa: layout B is never applied at the
   root).
5. A new language starts with `docs/<lang>/README.md` plus a mirrored structure
   for whatever is translated.

## Consequences

- `docs/README.zh-CN.md` and `docs/decisions/001-...zh-CN.md` move to
  `docs/zh-CN/...` as part of this decision.
- `CONSTITUTION.md` §1.5/§4.1.4 are amended to describe the two-zone layout;
  this ADR holds the operational detail.
- A translation completeness check (tree-level diff) becomes a sensible future CI
  gate; not mandated here.
