<!--
Title (required): Conventional Commits, English, imperative, ≤ 72 characters:
    type(scope): subject
Examples:
    feat(renderer): add point cloud rendering pipeline
    fix(scene): correct frame transform order
-->

## What and why

<!-- One paragraph: what this change does, and why (link the problem). -->

## Related documents

<!-- Link the plan/ADR when the change is design-significant (§6.1). Required:
     feature work → its `docs/specs/<feature-id>/` documents. -->

## Checklist

- [ ] Conventional Commits title (type(scope): subject, English, ≤ 72 chars)
- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo test --workspace --all-targets` passes
- [ ] Tests added for new behavior (or explained why not)
- [ ] `docs/` updated when design-significant, with the `zh-CN` mirror at the
      same relative path (exception: `docs/specs/`, §1.9 / ADR 004)
- [ ] `cargo audit` passes (dependency changes)
