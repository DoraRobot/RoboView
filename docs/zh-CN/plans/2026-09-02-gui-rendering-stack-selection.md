# GUI 与渲染技术栈选型

状态：Approved（已批准）

日期：2026-09-02

相关：ADR 006；CONSTITUTION §2.4.1–2.4.4

> 本文件是英文版 [`docs/plans/2026-09-02-gui-rendering-stack-selection.md`](../../plans/2026-09-02-gui-rendering-stack-selection.md)
> 的中文镜像；如有冲突以英文版为准。

## 背景

仓库需要为可视化引擎做技术栈决策：跨平台桌面（macOS / Windows / Linux）、
设计上使用 GPU 加速，渲染领域是点云、网格、路径、帧、标记与计算图。
技术栈不得扰乱分层架构：核心库保持无 GUI（§2.4.1）。本方案记录评估与所选形态；
约束性记录是 ADR 006。

## 候选评估

| 形态 | 评估 |
|---|---|
| 通用引擎（app/plugin/ECS 外壳） | 丰富渲染宏的捷径，但核心层会继承渲染之外的引擎机制（窗口、调度、资产管线），破坏 §2.4.1；场景模型针对游戏场景而非批量重度数据展示；预期 0.x 频繁破坏重写与重度依赖图。 |
| **可组合部件：GPU API + 数学 + 即时模式 GUI + 自研渲染核心** | **采纳。** 分层干净（render/math/IO 在 core，GUI 在 app）、领域契合（自研显示类型 trait）、依赖量适中、API 演进保守、宽松许可符合 ADR 005。 |
| 现成查看器平台作为依赖 | 实现工作量最小；但数据模型与 API 产品专属且不稳定；自研渲染核心正是本项目的主要价值。 |

## 决策

- 技术栈：`wgpu`（三平台统一 GPU 后端）、`glam` + `bytemuck`（数学、GPU 数据）、
  `eframe` wgpu 后端 + `egui`/`egui-wgpu`/`egui_dock`/`egui_plot`
  （应用壳与面板）、`roboview-core` 内自研渲染核心（`render/`、`scene/`、`displays/`）。
- 依赖方向：仅 app → core；核心可 headless 构建，模块树镜像未来 crate 边界。
- 本决策不启动任何功能开发。首个 SDD 功能（`docs/specs/`）先端到端演练
  技术栈（窗口、GPU 批量、相机），再扩展深入显示类型。
- 许可检查：`deny.toml` 白名单覆盖所选 crate。

## 影响

- ADR 006 承载约束性决策。
- 本方案之后的下一步设计：第一个功能规格（SDD 工作区），围绕单个
  走通 GPU 的视口切片展开。
