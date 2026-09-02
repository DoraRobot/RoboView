# 文档

`docs/` 目录的索引与约定。约束性规则见
[`CONSTITUTION.md`](../../CONSTITUTION.md) §4——本文件是实用手册。

**语言：** [English](../README.md) · 中文

> 本文件是英文版 [`docs/README.md`](../README.md) 的中文镜像。英文版为准。

## 目录

英文（默认层）：
- `plans/` — 项目级计划：治理变更、里程碑、架构方向
- `specs/` — 功能级 SDD 工作区——中文工作文档，无镜像（ADR 004）
- `design/` — 架构与详细设计文档
- `decisions/` — 架构决策记录（ADR）：`NNN-title.md`，编号，批准后不可变

中文树（`zh-CN/`）：结构与默认层镜像，仅收录已翻译文档；未翻译的文档不出现。

## 约定

- 本目录所有文档**以英文为准则语言**（`CONSTITUTION.md` §1.3）。中文译文放在本语言树的
  对应路径（如 `zh-CN/decisions/001-xxx.md`），放进树的同时保证内容是完整最新译文（§1.7）。
- 布局细节（全向规则、链接规则、稀疏许可）见 [`decisions/001-doc-localization-layout.md`](decisions/001-doc-localization-layout.md)。
- 文件名 kebab-case，如 `point-cloud-rendering.md`。ADR 使用补零编号：
  `001-layered-architecture.md`、`002-gpu-backend.md`。
- 每篇文档开头包含元信息：标题、状态、日期。

  ```
  # <标题>

  Status: Draft | In Review | Approved | Superseded | Rejected

  Date: 2026-09-02
  ```

- 状态具有约束力：`Approved` 之前不得实现（`CONSTITUTION.md` §6.1）。

## 语言

- 支持：`zh-CN`（中文）。语言树允许稀疏——没有译文的文档在该树中不存在。
- 添加新语言：创建 `docs/<lang>/README.md`（BCP-47 名称），并为已翻译内容镜像相应结构。
- 语言切换行不含自链接：当前语言以纯文本呈现（`CONSTITUTION.md` §1.8）。
- 按读者一无所知的标准写作：说明问题、候选方案、决策与理由。
- 属设计层面的代码改动，必须在 PR 描述中链接对应文档。
- `docs/` 中每篇文档随创建同步提供 `zh-CN` 翻译，存放于同一相对路径（语言树允许稀疏，§1.7——本项目保持译文处于最新）；`docs/specs/` 除外（ADR 004）。
- 跨 workspace 引用必须全限定：如 `001-point-cloud-viewport spec.md A7`；裸编号（如 A7）只在当前 workspace 内有效。
- SDD 功能工作区以 `NNN-<kebab 名>` 命名，`NNN-` 为三位零填充序号（`001-`、`002-`…），按创建顺序递增；序号保持唯一且递增，旧工作区不改号。
- SDD workspace 的 spec/plan/tasks 三个头部状态在一次状态变更中同步更新。

## 流程（简述）

1. 在 `plans/YYYY-MM-DD-<主题>.md` 建立 `Draft` 方案。
2. 讨论后改为 `In Review`。
3. `Approved` → 开始实现；方案作为记录保留。
4. 深层架构选择在 `decisions/` 中建立 ADR。
