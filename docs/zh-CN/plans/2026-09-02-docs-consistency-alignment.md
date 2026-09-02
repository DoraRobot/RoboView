# 文档一致性对齐——workspace 门禁与索引约定

状态：Approved（已批准）

日期：2026-09-02

相关：CONSTITUTION §2.3.1、§2.9.4、§6.4（已修订）；ADR 001；ADR 003（zh 镜像）

> 本文件是英文版 [`docs/plans/2026-09-02-docs-consistency-alignment.md`](../../plans/2026-09-02-docs-consistency-alignment.md) 的中文镜像；如有冲突以英文版为准。

## 背景

workspace 拆分后对仓库文档做了全量审查，发现四处不一致：

1. **CI 门禁命令停留在单 crate 时代。** `cargo clippy --all-targets ...` 与
   `cargo test --all-targets` 在 workspace 根目录只作用于默认成员，
   `roboview-core` 会逃过两道门禁。`cargo fmt` 与 `cargo audit` 不受影响
   （fmt 默认覆盖整个 workspace）。
2. **ADR 003 的中文镜像滞后。** 英文版已为 workspace 修订，中文版仍描述
   "今天是单 crate、根级 `assets/`"。
3. **语言切换行约定（§1.8）未写入 `docs/` 索引约定**；中文索引还整节缺失 `Languages`。
4. **ADR 001 未引用无自链接切换行规则（§1.8）。**

## 决策

- 在 CONSTITUTION §2.3.1、§2.9.4、§6.4 及两份 README 的 Gates 一节中，
  为 clippy/test 门禁加上 `--workspace`。
- 将 `docs/zh-CN/decisions/003` 重新同步为修订后的英文 ADR。
- 在 `docs/` 索引的 Languages 一节加入切换行规则（英文与中文）。
- 在 ADR 001 规则 2 中补充 §1.8 引用（英文与中文）。

## 宪法修订

- 门禁命令为 workspace 修正；头部版本 0.2.2 → 0.2.3。
