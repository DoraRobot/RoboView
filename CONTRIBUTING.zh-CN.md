# 参与 RoboView 贡献

**语言：** [English](CONTRIBUTING.md) · 中文

感谢你为 RoboView 做贡献。具有约束力的规则见
[`CONSTITUTION.md`](CONSTITUTION.md)——本指南是其简版。
有冲突时以宪法为准。

> **状态：** 项目初期。预期会有破坏性变更与快速演进的约定。

## 快速开始

```sh
cargo run
```

需要稳定版 Rust 工具链（`rust-toolchain.toml` 已钉定）。

## 先设计后编码

非平凡工作始于文档，而非 diff：

- **项目级变更**（治理、里程碑、架构方向）：`docs/plans/YYYY-MM-DD-<主题>.md` 提案，
  状态 `Draft → In Review → Approved`。深层架构选择另建 ADR（`docs/decisions/`）。
- **具体功能**：SDD 工作区 `docs/specs/<feature-id>/`
  （`spec.md` → `plan.md` → `tasks.md`），中文撰写——这是英文唯一例外
  （CONSTITUTION §1.9）。

文档批准后才开始实现。

## 提交

- **Conventional Commits**，英文：
  `<type>(<scope>): <subject>` —— 如 `feat(renderer): add point cloud pipeline`。
- 一次提交一件逻辑变更；每个提交可编译且测试通过。
- subject 不超过 72 字符、祈使句、结尾无句号。

## 分支与 PR

- 分支命名：`<type>/<简短-kebab-描述>`（如 `feat/point-cloud`）。
- 先 rebase 到 `main`；以 squash merge 合并，标题用 Conventional Commits 规范。
- PR 描述写明改了什么、为什么，并在存在时链接方案/ADR。
- 合并前至少一个批准；所有 CI 门禁须通过。

## CI 门禁（全部强制）

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo deny check
cargo audit
```

## 语言

代码、注释、提交信息、issue 与 PR 一律英文。中文只以镜像文件出现
（根目录 `*.zh-CN.md`、`docs/zh-CN/` 树内），`docs/specs/` 工作区除外
（中文，无镜像）。

## 代码标准（摘要）

- rustfmt 默认配置（`cargo fmt`）、Clippy 默认 lint 集合、`-D warnings`。
- 库代码用类型化错误（`thiserror`），可执行文件用带上下文的传播（`anyhow`）；
  禁止静默吞错。
- 仅在必要时使用 `unsafe`，每处必须带 `// SAFETY:` 注释。
- 库代码诊断用 `tracing`——禁止 `println!`。
- 发布路径禁止 `dbg!()` 与 `todo!()` 占位。

## 行为准则

所有互动遵循[行为准则](CODE_OF_CONDUCT.zh-CN.md)。

## 许可

**MIT 与 Apache-2.0 双许可**（[LICENSE-MIT](LICENSE-MIT)、
[LICENSE-APACHE](LICENSE-APACHE)）。贡献即视为同意以相同条款授权你的贡献。
