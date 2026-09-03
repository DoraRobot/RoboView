# ui-blueprint — 方案

状态：Approved（已批准）

日期：2026-09-03

相关：`spec.md`（Approved）；ADR 004、ADR 006；`docs/specs/001-point-cloud-viewport/`（A11 固定测量协议）、`docs/specs/002-display-types/`（A6 台账/语义色）、`docs/specs/003-i18n-system-fonts/`（texts.rs/字体链/错误机制）；`docs/plans/2026-09-03-ui-feature-roadmap.md`

修订记录：2026-09-03 起草（按 spec 重审落位后的全套约束）；同日经负责人评审通过（Draft → Approved）。同日运行时修正：地平面 Z=0→Y=0（见 spec 修订行）。

## 1. 概述

本方案确定 HOW：四区固定骨架（左对象树/中视口/右属性/底状态栏+轻量消息条）、
视口辅助层（Y=0 地面网格+世界原点三轴+方位指示器，均不入场景树）、per-object 外观 uniform 通道
（004 与 005 共享的核心渲染演进）、相机数学前移（screen_to_ray 族）、语义色板 token 化、
主菜单双路径（macOS muda 原生 + Win/Linux 窗口内，共享 `AppAction`）、属性面板（002 参数全集
可编辑，经新增单对象提交服务 ≤1 帧生效）、树上右键三项+搜索+组默认色、添加▾内联（浮动窗口移除）、
i18n 新键、最小窗口（480×360）复验承检。

分层：core 侧=渲染通道+相机数学+网格生成（无 GUI 依赖）；app 侧=全部 UI。
新增依赖：仅 `muda 0.19.3`（macOS 目标门控）；其余零新增（egui_dock/eframe persistence 属 006）。

## 2. 依赖清单（新增，逐个理由，§2.8.1）

| crate | 归属层 | 用途 | 许可（已核验） | MSRV |
|---|---|---|---|---|
| `muda 0.19.3` | app（`crates/roboview`） | macOS 原生菜单栏（`init_for_nsapp` 注入、items 层 set_text 重建 locale）；**仅** `[target.'cfg(target_os = "macos")']` 依赖 + `default-features = false`（default 含 Linux 向 gtk/libxdo——gtk 为 LGPL 且 Linux impl 仅 gtk 一条路，门控为编译必需） | Apache-2.0 OR MIT（原始 .crate 的 Cargo.toml 核验） | 1.73 |
| `crossbeam-channel` / `keyboard-types` | muda 传递 | 菜单事件通道/键位解析 | 均 MIT OR Apache-2.0（deny allowlist 内，零改动） | ≤1.73 |

已具备：`objc2 0.6.4`/`objc2-app-kit 0.3.2`（Cargo.lock 已含，rfd 线）——muda 引入**零重复版本线**。
红线：独立依赖提交（§2.8.4）；提交信息写明用途；不启用额外 feature（muda 需 `objc2` 显式 NS* feature 集——按 muda manifest 默认即可）。

## 3. 模块设计（跨层契约）

### 3.1 core 渲染外观通道（`render/` 族）

```text
// 现状：单一 view-proj uniform（group(0) binding(0)），三管线共享；顶点色烧录；mesh FACE_COLOR=WGSL 常量
// 目标：group(1)/binding(0) 每对象 64B uniform {albedo: vec4, flags: u32}（外观色覆盖+选中标识）
//   - 每对象 1 个 uniform buffer + 1 个 bind group（wgpu 25 禁 binding 数组含 Uniform——选型以此为准；
//     单 buffer + has_dynamic_offset 为备选，256B 对齐 4× 浪费，不采用）
//   - 3 管线布局扩为 [bg0, bg1]；3 WGSL 加 group(1)（mesh FACE_COLOR 常量改读 uniform，fragment 可见）
//   - Renderer 为单一来源（layout/buffer 创建与 accessor）；上传/更新 API：set_appearance(id, u64 句柄…) 
//     —— 外观变更=就地更新该对象 uniform（queue 写），不触发 renderer/场景重建
//   - per-object uniform buffer 与几何句柄同生共死：同一 upload 入口创建、display Drop 释放——
//     002 A6 资源台账的 created/destroyed 平衡语义不变，无新增 ledger 行
//   - 命中（005）与选中置位复用同一通道（005 只加"写标识"逻辑）
```

### 3.2 相机数学（core `render/camera_math.rs`，纯函数可单测）

```text
pub fn screen_to_ray(view_proj: &Mat4, viewport_size: Vec2, pos: Vec2) -> Option<(Vec3, Vec3)>;  // 两点反投影
pub fn pointer_world(view_proj: &Mat4, viewport_size: Vec2, pos: Vec2, plane: WorldPlane) -> Option<Vec3>;
//   WorldPlane = 世界 Y=0（up +Y）| 相机目标平面（无网格时 M5 坐标口径）
pub fn orientation_gizmo(view_proj: &Mat4, rect: Rect) -> [(Vec2, bool); 3];
//   view-proj 线性 3×3 列归一取 .xy + y 翻转；列 xy 符号即朝向（不做 w≤0 取反——T3 实测钉住）；
//   轴与视线精确平行时该轴不可见（不复用 anchor_to_screen 的 None 语义）
```

### 3.3 地面网格（core 生成 + line 管线接入）

```text
pub struct GridView { … }  // 可见窗口生成：主线 1m/次线 0.2m；默认 ±100m 内随相机外扩/内收；线距分级 LOD
pub fn grid_strips(view: &GridView) -> Vec<Strip>;   // 纯函数；生成端裁剪（blend=None，无 alpha 淡出）
// line 管线：持久 LineMesh 容量预建 + `LinePipeline::update_mesh`（queue.write_buffer 就地刷新；
//   不触碰 counters/DisplayKind——"每帧新建 buffer"与 A6 台账失衡，明确禁用）
// 绘制次序：同 pass 线族之首（先于场景线对象）；深度参与测试、不写深度
// app 侧（viewport.rs）：空场景门控放宽为"场景非空或网格开"；关卡状态存 ViewportState（会话态）
```

### 3.4 菜单双路径（app `ui/menu.rs`）

```text
pub enum AppAction { Open, Fit, AddFrame, AddMarker, ToggleGrid, ToggleAxes, Language(Locale), … }
// macOS：muda 菜单树 —— App::new 早期 init_for_nsapp；MenuEvent::set_event_handler(OnceCell 一次注册)
//   + egui Context::request_repaint 唤醒；事件入内部队列、update() 内 dispatch
// Win/Linux：egui 窗口内菜单栏（MenuBar），按钮 id → 同一 AppAction
// locale 重建：遍历 items 层 set_text（顶层 Menu 无 set_text）；App 菜单（Quit/Cmd+Q）就位；
//   加载单飞期 Open 项 set_enabled 同步
```

### 3.5 属性编辑（app `properties_panel.rs` + 单对象提交服务）

```text
// properties_panel：分组卡片 + DragValue（speed/min_decimals 规范）；A3 全集可编辑
// 单对象提交服务（ViewportState）：改字段 → 按 kind 重调上传单臂（frame/marker 几何微小，重传即可；
//   点云/网格外观经 3.1 通道 64B 队列写）→ ≤1 帧生效；A6 台账语义不变
// mesh 颜色 CPU 承载：app 层 id → Appearance 注册表（core 数据模型不动；重建/重传时重放）
```

## 4. 最小功能点拆分与编排（依赖为序，波内并行、文件互斥）

| FP | 功能点 | 层 | 依赖 | 文件（互斥所有权） |
|---|---|---|---|---|
| C1 | 相机数学三纯函数 + UT | core | — | `core/render/camera_math.rs` |
| C2 | 轴色常量 pub 化（语义色登记点） | core | — | `core/render/line.rs`（仅常量） |
| C3 | 网格生成纯函数（可见窗口/LOD） + UT | core | — | `core/render/grid.rs`（新） |
| C4 | LinePipeline 持久 mesh + 就地刷新 API | core | — | `core/render/line.rs` |
| C5 | per-object 外观通道（3 管线/WGSL/更新 API） | core | C2、C4 | `core/render/renderer.rs`、`mesh.rs`、`line.rs`、`assets/shaders/*.wgsl` |
| A1 | theme.rs 语义色板 + 3-token 断言 UT | app | C2 | `app/ui/theme.rs`（新） |
| A2 | texts 新键（EN/ZH，约 20 键） | app | — | `app/ui/texts.rs` |
| A3 | muda 依赖落地 + 接线 spike（macOS 冒烟） | app | 依赖落地 | `Cargo.toml`（macOS 目标节）、`app/ui/menu_bridge.rs`（新，spike） |
| A4 | 菜单树 + AppAction + 双路径 + locale 重建 | app | A3（spike 通过）、A1 | `app/ui/menu.rs`（新）、`app/main.rs` |
| A5 | 四区骨架（固定分区 + 空态与辅助层共存） | app | A1 | `app/main.rs`（壳，预留插槽） |
| A6 | 对象树升级（分组/折叠/眼睛/右键三项/搜索/组默认色） | app | A1 | `app/ui/objects_panel.rs` |
| A7 | 视口辅助层接入（网格 C3+C4、原点三轴、指示器、HUD 双入口开关、空态门控） | app | C1/C3/C4、A1 | `app/ui/viewport.rs` |
| A8 | 属性面板骨架 + 只读展示（分组卡片/DragValue） | app | A6 | `app/ui/properties_panel.rs`（新） |
| A9 | 状态栏 + 轻量消息条 | app | C1、A5 | `app/ui/status_bar.rs`（新） |
| A10 | 单对象提交服务 + 属性编辑生效链（≤1 帧） | app | A7、A8（C5 通道） | `app/ui/viewport.rs` |
| A11 | 添加▾内联（浮动窗口移除、默认值+回车即加） | app | A6、A5 | `app/ui/objects_panel.rs`、`app/main.rs` |
| A12 | 性能协议执行（M9/A12） | app | C5、A7、A10 | 记录、A9 守卫无涉 |
| A13 | 回归（M6/A10/A13：001–003 En 全量 + zh 480×360 复验 + 路径守卫 + 五门禁） | 全部 | 全部 | — |

**波次编排**（指示性；同波 agent 间文件互斥）：

| 波 | FP | 说明 |
|---|---|---|
| W0 | A3（依赖+spike） | 依赖落地为独立提交；spike=macOS 本机验证注入/唤醒/locale 重建，**结论回填 A4 前置** |
| W1 | C1 ∥ C2 ∥ C3 | core 三件，文件互斥；各带 UT |
| W2 | C4 →（串行）C5 | line.rs 单一 owner 纪律：C4 先落 update_mesh，C5 在同一 agent 连贯签名（严禁并行双改） |
| W3 | A1 ∥ A2 ∥ A4（骨架版） | menu.rs 可先行落地（A3 通过后）；A4 完整收尾以待 W5+ |
| W4 | A5 ∥ A6 | main.rs 壳与 objects_panel 并行 |
| W5 | A7 ∥ A8（骨架）∥ A9 | viewport.rs / properties_panel.rs（新）/ status_bar.rs（新）三文件互斥 |
| W6 | A10 → A11 | viewport.rs 已释放；A11 移除浮动窗口（objects_panel.rs+main.rs） |
| W7 | A12 → A13 | 协议执行与回归；A13 含 MAN 验收清单 |

## 5. 实施限制与纪律

- **line.rs 单一 owner**：C4/C5 同一 agent 连续完成（波内串行），其余波不得触碰。
- **mes 颜色/外观**：CPU 承载=app 层 `id → Appearance` 注册表 + C5 通道；core 数据模型不动（spec §5 层界）。
- **A6 台账**：C5 的 per-object uniform 与几何句柄同生共死，不新增 counters 行——实现时以 002 A6 判据回归验证（50 轮循环）。
- **路径守卫**：无 `.ply/.pcd/.obj/.csv/.xyz` 字面量进入源码；网格/指示器为纯代码生成。
- **macOS 平台**：muda 相关代码 cfg 门控；ubuntu 侧编译面不得出现 muda 踪迹（门控为编译必需）。
- **muda spike 结论（T2 实测回填）**：① `MenuId` 为 `pub String`（非 u32），事件映射按字符串设计；
  ② muda `Menu`/`Submenu` 为 `Rc` 非 Send——**菜单树必须由 app 持有保活**（BridgeCtx 字段模式），
  static 仅放事件队列（`VecDeque<MenuEvent>`）；③ handler 内**即时 `request_repaint()`** 端到端可靠
  （osascript 实点验证），`request_repaint_after` 预排程不可靠（一次性丢、尾部量化）；④ 注入时序/
  items 层 set_text/OnceCell 单次注册/Quit 均落实测；手动项=无闪烁目视、Cmd+Q、bundle 名，记入 004 MAN。
  若与 spec 声明冲突以实测为准并修订 spec（本次无：spec 声明全部成立）。

## 6. 测试与验证

- core UT：camera_math 三函数（已知 view_proj 定点断言）、网格生成（窗口边界/LOD 无零长段）、
  通道 uniform（上传/更新/句柄同生共死）、naga headless 3 WGSL 编译。
- app UT：theme 断言（3-token 与 core 轴色/002 语义色）、texts 键对齐、A9 守卫。
- MAN：spec A1–A13 逐项（含 480×360、空场景、网格开关语义、指示器旋转、右键三项、搜索、属性编辑
  ≤1 帧、muda 菜单与 locale 重建）。
- 性能：A12=A11 固定协议、release/参考机、场景 C + 网格开关 × 选中通道轮询、p95≤33ms 且网格开 ≤ 关 +1ms。

## 7. 风险表

| 风险 | 等级 | 缓解 |
|---|---|---|
| per-object 通道触及共享深度/三管线契约（check_compatible 严格相等） | 中 | 通道只加 group(1)，不改 pass/深度格式（已核验 pass 一致性只比深度/sample）；W2 后 naga+深度亲和冒烟 |
| A2 属性编辑 ≤1 帧与外观通道写入竞争 | 中 | 单对象提交服务集中入口；打开 A11 协议采样 |
| muda 接线与 eframe 空闲循环（菜单无响应风险） | 中 | W0 spike 前置；set_event_handler+request_repaint 已验证 API，实测回填 |
| 地面网格绘制次序（盖住地面路径/贴地平板观感） | 低 | 线族之首 + 协议场景记录开关状态（spec 已裁） |
| 002 A6 台账被 per-object uniform 破坏 | 低 | 句柄同生共死（§5 纪律）；50 轮循环回归 |
| 008/005 共享通道后续修改触发本功能返工 | 低 | 通道为 004 契约（spec §6 声明 005 只置位标识）；接口冻结入 core 文档 |

## 8. 后续衔接

- tasks.md：按本编排生成（T 系列 + 依赖列 + A 映射）；实现按波次启动（每波门禁=所属 FP 的 UT/协议通过）。
- 006 依赖本功能产物：固定四区骨架（默认布局）、面板最小宽度、ViewportState 开关状态（不持久化）。
- 005 依赖本功能产物：C1 screen_to_ray/指针交点、C5 选中置位通道、A6 树上选中主语、A2/A3 属性编辑。
