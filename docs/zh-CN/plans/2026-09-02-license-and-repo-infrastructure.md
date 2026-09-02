# 许可与仓库基础设施

状态：Approved（已批准）

日期：2026-09-02

相关：ADR 005；CONSTITUTION §0、§6.4

> 本文件是英文版 [`docs/plans/2026-09-02-license-and-repo-infrastructure.md`](../../plans/2026-09-02-license-and-repo-infrastructure.md)
> 的中文镜像；如有冲突以英文版为准。

## 背景

对照成熟公开仓库的常见形态审查后，发现治理层完整，但骨架缺四块地基：
许可（README 仍写"待定"）、CI（宪法 §6.4 规定四道门禁却无执行者）、
本地/CI 一致的工具链钉定，以及面向贡献者的入口（贡献指南、安全政策、
PR/issue 模板）。Cargo 清单也没有发布元数据。

## 决策

- **许可：** MIT OR Apache-2.0 双许可（ADR 005）——`LICENSE-MIT` + `LICENSE-APACHE`，
  经 `[workspace.package]` 声明 SPDX；可执行 crate `publish = false`。
- **CI：** `.github/workflows/ci.yml` 执行宪法四门禁
  （`cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、
  `cargo test --workspace --all-targets`、`cargo audit`）。
- **工具链：** `rust-toolchain.toml` 钉定 `stable` + rustfmt/clippy 组件
  （宪法 §2.1 基线）。
- **贡献入口：** `CONTRIBUTING.md`（+ 中文镜像）向贡献者转述宪法；
  `SECURITY.md`（+ 中文镜像）供私有漏洞报告；
  `.github/pull_request_template.md` 强制执行规范标题/方案链接/门禁清单；
  bug 与功能请求的 issue 模板。
- **清单元数据：** `[workspace.package]` 补充 license、description、keywords、
  categories 与 repository（`https://github.com/DoraRobot/RoboView`，2026-09-02 加入）。
  AtomGit 镜像承载中文面向的副本。

## 宪法修订

- §0 增加 License 一行（MIT OR Apache-2.0，ADR 005）；版本 0.3.0 → 0.3.1。
