# 002 — Documentation Site Location

Status: Approved

Date: 2026-09-02

Supersedes: none

Related: CONSTITUTION.md §4.1.2, §4.2.4; ADR 001

## Context

`docs/` is the home of **engineering documentation** — design proposals,
implementation plans, and ADRs, each carrying a
`Draft | In Review | Approved | Superseded | Rejected` lifecycle and reviewed
like code. It is never a published artifact (§4.1.2, §4.2.4).

A **user-facing documentation site** (quick starts, user guide, API reference,
tutorials) will be introduced later. A site like this is a buildable product of
its own: it depends on a mature site system (mdBook, MkDocs, Docusaurus,
Astro, ...), brings node/runtime dependencies, themes, navigation, search,
language routing, and a deployment pipeline. The question is where it lives
in this repository.

## Candidates

| Candidate | Assessment |
|---|---|
| **`site/` at the repository root** | Preferred. Toolchain, build artifacts, and publishing pipeline stay isolated from engineering docs. Leaves room for landing page, blog, and versioned docs (`/v1/`) in the same tree. |
| `docs/site/` (nested) | Collides with the conventions of this tree (ADR 001): ADR numbering, Status headers, sparse language trees, tree-level completeness checks make no sense for user guides. Node dependencies and build output inside the engineering-doc tree are pollution. Note also that MkDocs' default content directory is `docs/`, so a site inside `docs/` produces stuttering paths. |
| The site consumes `docs/` directly (e.g. mdBook `src: docs/`) | Rejected. Publishing engineering docs violates §4.2.4 — Draft/Rejected designs would leak into the public site, and the content goals of the two document classes are not the same. |

## Decision

When a documentation site is introduced, it lives in **`site/` at the repository
root** (`website/` is an equivalent alternative name; this ADR standardizes on
`site/` for brevity and because the tree can later host landing pages and blog).

`docs/` remains exclusively engineering documentation and does not change.

## Rules

1. `docs/` never hosts site build files (`package.json`, `book.toml`, `node_modules`,
   generated output, site assets); `site/` never hosts engineering docs, proposals,
   or ADRs.
2. All build output and dependency directories of the site are git-ignored
   (`site/node_modules/`, `site/dist/` or generator equivalents, `site/book/`).
3. The public site publishes only confirmed results (§4.2.4). Engineering
   documents are never copied into the site content.
4. API reference is generated from rustdoc (docs.rs when published); it does not
   consume a directory in this repository.
5. The site uses the site system's own i18n mechanism for its content languages;
   the repository's language rules (ADR 001) apply to engineering docs only.

## Consequences

- Nothing is built yet; this decision removes the need to re-litigate the
  location when the site lands.
- At introduction time, add a `site/` entry to the repository layout in
  `README.md`, and amend `CONSTITUTION.md` §4.1.3 if its structure block should
  list the new directory.
