# Security Policy

**Languages:** English · [中文](SECURITY.zh-CN.md)

RoboView is under active development; please treat security reports
seriously and report them privately.

## Reporting a vulnerability

- **Do not open a public issue** for a suspected vulnerability.
- Use GitHub's private vulnerability reporting (Security → Report a
  vulnerability) to create a draft security advisory, or email the
  maintainers directly if you have a private channel.
- Include in your report:
  - the affected version / commit and platform,
  - a description of the vulnerability and its impact,
  - reproduction steps, preferably a minimal proof of concept,
  - any fix suggestion you have.
- Please keep the details private until a fix is released. Public disclosure
  before a fix puts users at risk; we will publish an advisory with the fix.

## Response

- Security issues are triaged before feature work; we aim to acknowledge a
  report within a few days and will coordinate a fix and advisory.
- Reproducible, confirmed issues get a fix on the current branch; the
  advisory is published after affected users can upgrade.

## Scope

This policy covers the RoboView application and its `roboview-core` library.
Vulnerabilities in third-party dependencies are handled through `cargo audit`
in CI — report those upstream rather than here.
