# ADR 批准与宪法资源条款

状态：Approved（已批准）

日期：2026-09-02

相关：ADR 001；ADR 003；CONSTITUTION §1.5、§2（已修订）

> 本文件是英文版 [`docs/plans/2026-09-02-adr-approval-and-assets-article.md`](../../plans/2026-09-02-adr-approval-and-assets-article.md) 的中文镜像；如有冲突以英文版为准。

## 背景

ADR 001（文档本地化布局）与 ADR 003（资源位置）自第一天起就在生效——§1.5 引用 ADR 001
作为操作细节，workspace 拆分执行了 ADR 003 的"资源随 crate"规则——但两者在形式上仍是
Draft。ADR 003 还把"宪法 §2 新增资源条款"与自身批准绑在一起。

## 决策

- 批准 ADR 001 与 ADR 003（Draft → Approved）。
- 宪法新增 §2.11（资源）：资源放在所属 crate 旁（ADR 003），并遵守
  构建期嵌入 / 运行时磁盘加载策略。
- 敲定 ADR 003 的影响行：§2 条款已存在。

## 宪法修订

- 新增 §2.11；头部版本 0.2.3 → 0.2.4。
