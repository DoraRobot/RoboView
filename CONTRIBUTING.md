# Contributing to RoboView

**Languages:** English · [中文](CONTRIBUTING.zh-CN.md)

Thank you for contributing to RoboView. The binding rules live in
[`CONSTITUTION.md`](CONSTITUTION.md) — this guide is the short version.
When in doubt, the constitution wins.

> **Status:** Early phase. Expect breaking changes and fast-moving conventions.

## Quick start

```sh
cargo run
```

Requires the stable Rust toolchain (`rust-toolchain.toml` pins it).

## Design before code

Non-trivial work starts with a document, not a diff:

- **Project-level change** (governance, milestone, architecture direction):
  a proposal in `docs/plans/YYYY-MM-DD-<topic>.md`, status
  `Draft → In Review → Approved`. Deep architectural choices also get an ADR
  in `docs/decisions/`.
- **A concrete feature**: the SDD workspace `docs/specs/<feature-id>/`
  (`spec.md` → `plan.md` → `tasks.md`), written in Chinese — this is the one
  deliberate exception to the English-only rule (CONSTITUTION §1.9).

Implementation starts only after the document is approved.

## Commits

- **Conventional Commits**, in English:
  `<type>(<scope>): <subject>` — e.g. `feat(renderer): add point cloud pipeline`.
- One logical change per commit; each commit compiles and keeps tests green.
- Subject ≤ 72 characters, imperative, no trailing period.

## Branches and pull requests

- Branch naming: `<type>/<short-kebab-desc>` (e.g. `feat/point-cloud`).
- Rebase onto `main`; merge via squash merge with a Conventional title.
- PR description states what and why, and links the plan/ADR when one exists.
- At least one approval before merge; all CI gates must pass.

## CI gates (all mandatory)

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo deny check
cargo audit
```

## Language

English only in code, comments, commits, issues, and PRs. Chinese appears
only as mirror files (`*.zh-CN.md` at the root, `docs/zh-CN/` inside `docs/`),
except for the `docs/specs/` workspace (Chinese, no mirror).

## Code standards (summary)

- rustfmt defaults (`cargo fmt`), clippy default lints, `-D warnings`.
- Typed errors in library code (`thiserror`), context-rich propagation in the
  binary (`anyhow`); no silent error swallowing.
- `unsafe` only when necessary, every block with a `// SAFETY:` comment.
- `tracing` for diagnostics in library code — never `println!`.
- No `dbg!()` and no `todo!()` placeholders in release paths.

## Code of conduct

All interactions follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## License

Dual-licensed **MIT OR Apache-2.0** ([LICENSE-MIT](LICENSE-MIT),
[LICENSE-APACHE](LICENSE-APACHE)). By contributing, you agree to license your
contribution under the same terms.
