# RoboView

A cross-platform 3D data visualization tool for robotics and AI data, built in Rust —
rendering robot sensor data, scene frames, and computation graphs in an interactive
3D viewport.

**Languages:** English · [中文](README.zh-CN.md)

**Repository:** [github.com/DoraRobot/RoboView](https://github.com/DoraRobot/RoboView) (canonical) · [AtomGit](https://atomgit.com/DoraRobot/RoboView) (Chinese mirror)

![CI](https://github.com/DoraRobot/RoboView/actions/workflows/ci.yml/badge.svg)
![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-informational)

> **_Status:_** Early phase. The foundation (standards, language policy) and the
> rendering/GUI stack (ADR 006) are ratified; the first feature slice — opening
> and viewing point clouds (PLY/PCD) in a GPU viewport — works. Breaking changes
> are still expected.

## Highlights (planned)

- 🎮 Interactive 3D scene — pan, orbit, zoom, pick
- 📡 Data display — point clouds, grids, paths, frames, markers
- 🔌 Plugin-style display types
- 🖥️ Cross-platform desktop — macOS / Windows / Linux
- 🌍 Internationalized UI, English first

## Getting started

```sh
cargo run
```

Requires the stable Rust toolchain (see the workspace `Cargo.toml` for the version).

## Repository layout

```
.
├── CONSTITUTION.md        # Binding project standards (language, Rust, git, docs)
├── CONSTITUTION.zh-CN.md  # Chinese mirror of the constitution
├── README.md              # This file — English, canonical
├── README.zh-CN.md        # Chinese mirror of this README
├── LICENSE-MIT            # Dual license, MIT part (ADR 005)
├── LICENSE-APACHE         # Dual license, Apache-2.0 part (ADR 005)
├── CONTRIBUTING.md        # Contributor guide (mirror: CONTRIBUTING.zh-CN.md)
├── CODE_OF_CONDUCT.md     # Community code of conduct (mirror: CODE_OF_CONDUCT.zh-CN.md)
├── SECURITY.md            # Vulnerability reporting policy (mirror: SECURITY.zh-CN.md)
├── Cargo.toml             # Workspace manifest (virtual root, CONSTITUTION §2.4.2)
├── Cargo.lock             # Locked dependency versions (committed)
├── crates/
│   ├── roboview/          # GUI binary crate: desktop app (UI panels, platform shell)
│   │   └── src/
│   │       ├── main.rs    # Application entry point
│   │       └── ui/        # UI modules
│   └── roboview-core/     # Core library crate — no GUI dependencies
│       └── src/
│           ├── lib.rs     # Crate root (module tree)
│           ├── scene/     # Scene graph, frames, transforms
│           ├── render/    # GPU rendering core
│           ├── io/        # Data IO (formats, transports)
│           └── displays/  # Display-type traits & built-in display types
├── docs/
│   ├── README.md          # Documentation index & conventions (English)
│   ├── plans/             # Feature proposals & implementation plans
│   ├── design/            # Architecture & detailed design documents
│   ├── decisions/         # Architecture Decision Records (ADR)
│   └── zh-CN/             # Chinese language tree (mirrors of translated docs)
└── site/                  # User documentation site (placeholder, see ADR 002)
```

## Documentation

- **[CONSTITUTION.md](CONSTITUTION.md)** — the binding standards: language policy
  (§1), Rust development standards (§2), git & commit style (§3), docs conventions (§4)
- **[docs/](docs/)** — all design plans and technical proposals live here

## Contributing

Please read the [CONSTITUTION.md](CONSTITUTION.md) before contributing. The short
version:

- **English only** in code, comments, commits, issues, and PRs; Chinese appears only
  as separate `.zh-CN.md` mirror files — never mixed into an English document
- **Conventional Commits**: `<type>(<scope>): <subject>`,
  e.g. `feat(renderer): add point cloud rendering pipeline`
- **Design before code**: non-trivial work starts with a proposal in `docs/plans/`
- **Gates**: `cargo fmt --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace --all-targets`, `cargo deny check`, `cargo audit`
- Full contributor guide: [CONTRIBUTING.md](CONTRIBUTING.md)

## License

Dual-licensed under **MIT OR Apache-2.0** (ADR 005):
[LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).
