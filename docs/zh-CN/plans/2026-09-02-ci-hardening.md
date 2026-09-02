# CI 加固与贡献者工具

状态：Approved（已批准）

日期：2026-09-02

相关：CONSTITUTION §2.8.3、§6.4（已修订）；ADR 005

> 本文件是英文版 [`docs/plans/2026-09-02-ci-hardening.md`](../../plans/2026-09-02-ci-hardening.md)
> 的中文镜像；如有冲突以英文版为准。

## 背景

对照成熟仓库的差距审查发现，基础 CI 不足：它只跑单一平台，而 RoboView
是跨平台桌面（§0）；声明的最低 Rust 版本（1.85）未经验证；许可政策
（§2.8.3、ADR 005）没有机器执行；rustdoc 告警无守卫；行尾与编辑器默认
未规范化；社区行为准则与贡献者编辑器体验缺失。

## 决策

- **新增 CI 作业：** linux/macos/windows runner 的测试矩阵；1.85 的 MSRV
  作业（`cargo check`）；docs 作业（`RUSTDOCFLAGS="-D warnings" cargo doc`）；
  经 `cargo-deny`（根目录 `deny.toml`，白名单与 ADR 005 对齐；无许可与
  禁止许可拒绝，通配依赖拒绝）的许可检查。保留 `cargo audit` 作业。
- **宪法：** §2.8.3 在 `cargo audit` 旁点名 `cargo deny check`；
  §6.4 采纳 `cargo deny check` 为强制门禁。版本 0.3.1 → 0.3.2。
- **社区：** 根目录 Contributor Covenant v2.1 + `zh-CN` 镜像；
  从 CONTRIBUTING（EN + zh）链接。
- **编辑器规范化：** `.editorconfig`、`.gitattributes`（仓库 LF，
  `.bat` 用 CRLF）、`.vscode/extensions.json` 推荐 rust-analyzer。
- **入口同步：** README（EN + zh）与 CONTRIBUTING（EN + zh）现列出五门禁；
  README 目录树列出新增根文件。

## 宪法修订

- §2.8.3、§6.4 因 deny 门禁修订；版本 0.3.1 → 0.3.2。
