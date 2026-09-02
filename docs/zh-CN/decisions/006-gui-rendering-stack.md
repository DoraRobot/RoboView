# 006 — 渲染与 GUI 技术栈

状态：Approved（已批准）

日期：2026-09-02

取代：无

相关：CONSTITUTION §2.4.1–2.4.4、§2.8；方案 `docs/plans/2026-09-02-gui-rendering-stack-selection.md`

> 本文件是英文版 [`docs/decisions/006-gui-rendering-stack.md`](../../decisions/006-gui-rendering-stack.md)
> 的中文镜像；如有冲突以英文版为准。

## 背景

RoboView 是面向机器人与 AI 数据的跨平台桌面工具（macOS / Windows / Linux），
使用 GPU 加速：点云、网格、路径、帧、标记与计算图。除这些需求外，技术栈必须
符合分层架构（§2.4.1）：`roboview-core` 是无 GUI 的库，必须能 headless 构建。
本决策固定的是它的依赖图。

评估了三种候选形态：

- **通用引擎**（app/plugin/ECS 外壳 + 场景管理）。渲染开箱即用，但核心层会依赖
  渲染之外的引擎机制（窗口、调度、资产）——破坏无 GUI 边界——且引擎的场景
  模型与"大体积批量显示类型主导"的数据可视化领域不匹配。
- **可组合部件**——现代 GPU API + 数学库 + 即时模式 GUI，外加自研渲染核心。
- **现成查看器平台作为依赖**——最接近形态的同类产品。工作量最小，但其数据模型
  与 API 是产品专属、不稳定；渲染核心的掌控正是本项目的主要价值。

## 决策

- **`roboview-core`**（无 GUI）依赖：
  - `wgpu`——跨平台统一 GPU 后端（也是离屏渲染的道路），
  - `glam` 与 `bytemuck`——数学与 GPU 友好的数据布局，
  - 自研渲染内核：场景、几何缓冲、shader 管理、拾取、深度排序
    （模块 `render/`、`scene/`、`displays/`）。
- **`roboview`**（应用）依赖：
  - `eframe`（GPU 后端）+ `egui-wgpu`，
  - `egui` 面板、`egui_dock`（面板停靠）、`egui_plot`（2D 曲线），
  - 插件加载与平台外壳。
- 依赖方向保持单向：app → core。core 必须可 headless 构建运行；渲染器按
  支持离屏路径设计。
- 所选 crate 均为宽松许可，与 ADR 005 兼容；`deny.toml` 的白名单
  反映本技术栈。
- 本决策只固定技术栈；不从此开始任何功能工作。首个功能规格（SDD，`docs/specs/`）
  将演练完整切片：窗口、GPU 批量、数学。

## 规则

1. GUI 相关 crate（`eframe`/`egui` 系）只属于 `roboview` crate——
   绝不进入 `roboview-core`。
2. 核心层新增依赖必须在提交信息中说明用途（§2.8.1），且保持宽松许可（§2.8.3）。
3. 跨平台意味着每次改动后 CI 平台矩阵（对应操作系统系列的 runner）保持全绿。

## 影响

- `crates/roboview-core` 的第一个开发周期从 `wgpu` + `glam` 之上的
  render/scene 基础开始。
- 渲染核心若将来变得庞大，可在不动应用边界的前提下拆成独立 crate
  （见 CONSTITUTION §2.4.2）。
