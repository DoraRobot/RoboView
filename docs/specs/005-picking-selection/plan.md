# picking-selection — 方案

状态：Approved（已批准）

日期：2026-09-04

相关：`spec.md`（Approved）；ADR 004、ADR 006；`docs/specs/001-point-cloud-viewport/`（A11 固定测量协议/A6 台账）、`docs/specs/002-display-types/`（显示类型/资源台账/语义色）、`docs/specs/004-ui-blueprint/`（视口辅助层/相机数学/外观 uniform 通道/树上选中/属性面板；本功能的前置载荷——A4 三区镜像依赖 004 的选中主语与高亮通道）、`docs/specs/008-object-transform/`（依赖本功能产物：选择集与拾取，008 未实现前对象无运行时变换——命中一律按**原生世界坐标**，008 落地时在本功能命中入口加一次变换应用，属 008 修订面）

修订记录：2026-09-04 起草（按 spec 批准版全套约束；含 2026-09-04 键位/框选裁定 A9–A12 的 HOW）。

## 1. 概述

本方案确定 HOW：视口**点选/框选**、**默认键位**（005 A11 键位表——中键轨道/Shift+中键平移/左键点选+框选/滚轮光标锚定缩放）、**选择集**（app 层，对象级，不动 core 数据模型）与**三区镜像 + 多选降级**（单选维持 004 A3/A4，多选=属性面板概要）。

分层：core 侧=拾取数学（pick.rs：射线-三角形/射线-线段/点云近邻/文字锚框）+ 光标锚定缩放纯函数（camera_math.rs/camera.rs）；app 侧=键位适配（camera_input.rs）+ 点选/框选接线与选择集状态（viewport.rs/objects_panel.rs/properties_panel.rs/status_bar.rs）。

新增依赖：**零**（拾取=CPU 纯数学；点云近邻=自建均匀桶索引，不引入 kd-tree crate；mesh 命中=线性逐三角形 + 规模判据（1e6 面 <8ms/次，超出记录并留 007 优化位；005 §6 已定 CPU 路线）。

## 2. 关键判据

| 判据 | 定值 | 出处 |
|---|---|---|
| 点选阈值 δ | 视口逻辑高 × 0.5%（mesh/线类投影距离）；点云 r=8px（egui 点；物理=r×pixels_per_point） | spec A2 |
| 点击/拖动分界 | 4px（位移 >4px 进入框选；≤4px 抬起=点选） | spec A9 |
| 框选语义 | 投影包围框（8 角世界包框投影后的屏幕矩形）与拖出矩形接触即选 | spec A9 |
| 光标锚定漂移 | 缩放前后光标下世界点重投影漂移 ≤ 视口高 × 0.5% | spec A11 (UT) |
| 命中序 | 等距/重合时：先绘制(加序)者优先？——**就近优先，距离等价取后绘制者**（路径/箭头线族同在近处有语义） | spec A2 引 |
| 性能 | 点选/框选结算 ≤ 8ms（采样场景 C 交互帧不劣化；A7/A12 高速不崩、无残留） | spec A12 |

## 3. 选定实现要点

### 3.1 拾取模块 `crates/roboview-core/src/render/pick.rs`（新增，纯函数，零依赖）

命中对象 = 005 可拾取集合（DisplayObject 全部 5 型；frame 三轴线段按线族 capsule 命中；marker 文字=屏幕锚框命中/箭头=capsule；**地面网格与原点三轴不入**——004 辅助层语义）。

- `PickMesh`：射线-三角形（Möller–Trumbore，逐三角形线性）；1e6 面实测 <8ms，超标记入 plan §5（007 优化位）。
- `PickPointCloud`：均匀桶索引（桶尺寸=r，建一次 O(n)，命中 O(桶内)；点投影屏幕半径 r=8px 判据：世界半径 = 8px/px_per_m(目标平面折算需按命中点深度换算——**射线与点的屏幕距离**最简：投影后屏幕距离 ≤ r）；实现：投影步进——投影全部点大场景太贵 → 桶索引 + 投影校验。
- `PickLine`：射线-线段最短距离（capsule 半径 = δ 世界距离≈δ×depth（按命中深度折算 δ 世界 = δ_screen·(2·d·tan(fov/2)/视口高)）。
- `PickMarkerText`：锚点 → anchor_to_screen（既有）→ 屏幕锚框矩形命中（文字宽高近似：字符数×字号或存度量——**近似框**，spec A1 判据宽松）。
- 统一入口：`pick_objects(scene_iter, ray, ...) -> Option<PickHit { id, kind, t }>` 按 **t 最近优先**，等距取后加者；非有限/退化包围盒不 panic。
- 框选：`pick_rect(vp, rect, objects) -> Vec<u64>`（投影 8 角世界 Aabb → 屏幕矩形相交）。

### 3.2 光标锚定缩放（core）

`pointer_world` 已有（target 平面/地面参照）。实现：zoom 前取光标世界点 w(t=目标平面)；`camera.zoom(delta)`；缩放后再把 w 重投影并使 target 平移 `w - w'`（`camera.pan` 语义反向）——纯函数 `camera.zoom_at_cursor(delta, w, view_proj...)` 或 app 侧序列（core 纯函数放 camera_math.rs：`cursor_world_after_zoom`）；**UT 断言漂移 ≤视口高×0.5%**。

### 3.3 app 接线

- `camera_input.rs`：键位表 A11——middle=orbit；middle+shift=pan；滚轮=zoom_at_cursor；左键按下报告 `Press(click_or_drag)` 由 viewport 解析（<4px=点选，否则=框选开始）；左键抬=结算。互斥：框选期间相机冻结。
- `viewport.rs`：点选→pick（依 004 A2 精度）；`set_selected`/appearance 通道置位（004 已有）；选择集= `SelectSet { primary: Option<u64>, all: HashSet<u64> }`；框选橡皮筋=2D overlay（viewport rect painter）；Esc/Shift/Ctrl 修饰；F/Delete 集合版（聚焦相机取景到并集 bounds；Delete 经 004 删除链）。
- 面板联动：单选=004 A3/A4 原状；多选=属性面板概要（"已选 N 个对象"，禁编辑）+树行合集高亮+状态条计数（texts 新键：`selection_count`）。

### 3.4 文本（texts.rs）

新键：`pick_hint`？非必需。`selection_count(n)`（概括面板）+ 工具提示（框选橡皮筋提示）按需。A11 键位表首行 = 状态栏工具提示（004 M5 已设）。

## 4. 功能点拆分与波次（原子、可独立验证、文件互斥）

split：C1=拾取数学 core（W1，单 owner pick.rs）；C2=光标锚定缩放 core（W1 并行，camera.rs/camera_math.rs）；C3=键位适配（W2，camera_input.rs）；C4=点选接线（W2 并行？依赖 C1/C2 → W3）；C5=框选+选择集+修饰键（W3，viewport.rs）；C6=多选联动（W4，objects/properties/status_bar.rs——三文件三并行）；C7=回归与性能（W5）。

| 波次 | 功能点 | 文件（互斥） |
|---|---|---|
| W1 | C1 拾取数学 / C2 光标锚定缩放 | pick.rs（新）/ camera_math.rs+camera.rs |
| W2 | C3 键位适配 | camera_input.rs |
| W3 | C4 点选接线 / C5a 框选+集合 | viewport.rs（同 owner 连续） |
| W4 | C5b 修饰/键盘 / C6 面板联动 | viewport.rs（续）/ objects_panel.rs、properties_panel.rs、status_bar.rs |
| W5 | C7 回归、性能、文档、冒烟 | 各文件+spec/plan 校订 |

单 owner 纪律延续 004：pick.rs 全部 C1 一人连续完成（类 line.rs T6→T7）。

## 5. 风险与对策

| 风险 | 影响 | 对策 |
|---|---|---|
| 大 mesh 线性命中慢 | 高（A2 精度场景 C） | 1e6 面实测判据；超标→007 提交 BVH 优化（本 plan 记录，不扩范围） |
| 点云桶索引内存 | 中 | 桶=相对世界原点量化，场景米级；超大场景（>1e8 点）记录+分片索引留后续 |
| 键位改动影响 004 已验收 MAN | 高 | 004 MAN 项均为键位无关（验证逐条核对）；005 A11 为唯一键位事实源 |
| 多选降级与 004 A3(单对象编辑) 冲突 | 中 | 单选语义下完全不变；多选=概要禁编辑（A10 已定格） |
| 命中语义与 008 变换叠加 | 低 | 008 未实现；入口留 apply-transform 挂点（核心注释） |

## 6. 后续衔接

- tasks.md：T 系列（原子、依赖列、A 映射、文件互斥列）；实现按波次启动。
- 007 依赖本功能：右键表基于选择集（005 的选择主语）。
- 008 依赖本功能：选择集/拾取入口 + 变换挂点。
- 006 依赖：无新增（选择集=对话期状态，持久化面 006 仅持久 ViewportState 开关——选择集不持久）。
