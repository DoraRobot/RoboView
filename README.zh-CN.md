# RoboView

一个用 Rust 开发的跨平台 3D 数据可视化工具，服务机器人工程与 AI 数据处理领域——
在交互式 3D 视口中呈现机器人的传感器数据、场景坐标系与计算图。

**语言：** [English](README.md) · 中文

**仓库：** [github.com/DoraRobot/RoboView](https://github.com/DoraRobot/RoboView)（主仓库）· [AtomGit](https://atomgit.com/DoraRobot/RoboView)（中文镜像）

![CI](https://github.com/DoraRobot/RoboView/actions/workflows/ci.yml/badge.svg)
![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-informational)

> 本文件是英文版 [`README.md`](README.md) 的中文镜像。英文版为准：
> 若两者不一致，以英文版为准（见 [`CONSTITUTION.md`](CONSTITUTION.md) §1.7）。

> **_当前状态：_** 项目初期。基础规范（开发标准、语言政策）已通过；架构设计尚未开始。
> 预期会有破坏性变更。

## 规划中的特性

- 🎮 交互式 3D 场景——平移、环绕、缩放、拾取
- 📡 数据显示——点云、网格、路径、坐标系、标记
- 🔌 插件式显示类型
- 🖥️ 跨平台桌面——macOS / Windows / Linux
- 🌍 国际化界面，英文优先

## 快速开始

```sh
cargo run
```

需要稳定版 Rust 工具链（版本要求见 workspace 的 `Cargo.toml`）。

## 目录结构

```
.
├── CONSTITUTION.md        # 具有约束力的项目规范（语言、Rust、git、文档）
├── CONSTITUTION.zh-CN.md  # 宪法中文版
├── README.md              # 英文版（准则版本）
├── README.zh-CN.md        # 本文件，中文版
├── LICENSE-MIT            # 双许可的 MIT 部分（ADR 005）
├── LICENSE-APACHE         # 双许可的 Apache-2.0 部分（ADR 005）
├── CONTRIBUTING.md        # 贡献者指南（镜像：CONTRIBUTING.zh-CN.md）
├── CODE_OF_CONDUCT.md     # 社区行为准则（镜像：CODE_OF_CONDUCT.zh-CN.md）
├── SECURITY.md            # 漏洞报告政策（镜像：SECURITY.zh-CN.md）
├── Cargo.toml             # workspace 清单（虚拟根，见 CONSTITUTION §2.4.2）
├── Cargo.lock             # 锁定依赖版本（随仓库提交）
├── crates/
│   ├── roboview/          # GUI 可执行 crate：桌面应用（UI 面板、平台外壳）
│   │   └── src/
│   │       ├── main.rs    # 应用入口
│   │       └── ui/        # UI 模块
│   └── roboview-core/     # 核心库 crate——不依赖 GUI
│       └── src/
│           ├── lib.rs     # crate 根（模块树）
│           ├── scene/     # 场景图、坐标系、变换
│           ├── render/    # GPU 渲染核心
│           ├── io/        # 数据 IO（格式、传输）
│           └── displays/  # 显示类型 trait 与内置显示类型
├── docs/
│   ├── README.md          # 文档索引与约定（英文）
│   ├── plans/             # 功能提案与实施计划
│   ├── design/            # 架构与详细设计文档
│   ├── decisions/         # 架构决策记录（ADR）
│   └── zh-CN/             # 中文语言树（已翻译文档的镜像）
└── site/                  # 用户文档站（占位，见 ADR 002）
```

## 文档

- **[CONSTITUTION.md](CONSTITUTION.md)**（中文版：[CONSTITUTION.zh-CN.md](CONSTITUTION.zh-CN.md)）
  —— 强制规范：语言政策（§1）、Rust 开发标准（§2）、git 与提交风格（§3）、文档约定（§4）
- **[docs/](docs/)** —— 所有设计方案与技术提案都在这里

## 贡献

参与贡献前请先阅读 **[CONSTITUTION.md](CONSTITUTION.md)**。要点：

- 代码、注释、提交信息、issue 与 PR **一律使用英文**；中文只以独立的 `.zh-CN.md`
  镜像文件出现——绝不混入英文文档
- **Conventional Commits**：`<type>(<scope>): <subject>`，
  如 `feat(renderer): add point cloud rendering pipeline`
- **先设计后编码**：非平凡工作先提交方案到 `docs/plans/`
- **门禁**：`cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、
  `cargo test --workspace --all-targets`、`cargo deny check`、`cargo audit`
- 贡献者指南：[CONTRIBUTING.md](CONTRIBUTING.md)

## 许可证

**MIT 与 Apache-2.0 双许可**（ADR 005）：
[LICENSE-MIT](LICENSE-MIT) 与 [LICENSE-APACHE](LICENSE-APACHE)。
