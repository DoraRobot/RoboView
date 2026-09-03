# ui-blueprint — 任务清单

状态：In Review（待实现；spec/plan 已 Approved，实现开始条件已满足）

日期：2026-09-03

相关：`spec.md`（Approved）、`plan.md`（Approved）

> 约定：任务原子、可独立验证（ADR 004 规则 3）；渠道 UT=app/core 单测、CI=门禁/脚本、MAN=手工验收。
> 跨 workspace 引用全限定；本工作区仅 A1–A13（路径守卫 = `001-point-cloud-viewport spec.md` A9）。
> 波次编排按 plan §4（同波并行且文件互斥；line.rs 单一 owner 纪律：T6→T7 同一实现者连续完成）。

## 任务

| # | 波 | 任务 | 产出与验证 | 对应 | 依赖 | 状态 |
|---|---|---|---|---|---|---|
| T1 | W0 | muda 依赖落地 | `crates/roboview` 加 `[target.'cfg(target_os = "macos")'].dependencies` `muda = { version = "0.19.3", default-features = false }`（独立提交+用途说明 §2.8.1/§2.8.4）；传递 crossbeam-channel/keyboard-types 进 lock；deny 无白名单改动 | A3(依赖) | — | ☐ |
| T2 | W0 | muda 接线 spike（macOS 本机） | 冒烟验证：App::new 早期 `init_for_nsapp`（winit default_menu 覆盖时序）；`set_event_handler`(OnceCell 一次注册)+`egui::Context::request_repaint` 唤醒空闲循环；事件入队、`update()` dispatch 到动作；locale 重建=items 层 `set_text`；Quit/Cmd+Q 项；结论（含与 spec 声明的出入）回填 plan §5、A4 前置——**实测为准** | A3(spike)/批准前置① | T1 | ☐ |
| T3 | W1 | 相机数学三纯函数 | `core/render/camera_math.rs`：`screen_to_ray`（view-proj 逆两点反投影）、`pointer_world`（Z=0 网格面/相机目标平面两种参照）、`orientation_gizmo`（线性 3×3 列归一取 .xy + y 翻转、w≤0 取反）；每函数 UT（定点断言+退化用例）；`anchor_to_screen` 回归不破 | C1/A7、A9、M5 | — | ☐ |
| T4 | W1 | 轴色常量 pub 化 | `core/render/line.rs` AXIS_*_COLOR_SRGB 提为 pub（语义色登记点）+ core 侧对表断言（002 语义色 X红/Y绿/Z蓝） | C2/A9、M8 | — | ☐ |
| T5 | W1 | 网格生成纯函数 | `core/render/grid.rs`（新）：`GridView` 可见窗口（主线 1m/次线 0.2m、默认 ±100m 随相机伸缩、线距分级 LOD）、`grid_strips` 纯函数（生成端裁剪——blend=None 无 alpha）；UT（窗口边界/LOD 无 pop 跳动/无零长段/非有限输入不 panic） | C3/A7、A11、M7 | — | ☐ |
| T6 | W2 | LinePipeline 持久 mesh + 就地刷新 | `line.rs`：容量预建 `LineMesh`（≥最大线数）；新 `update_mesh`（`queue.write_buffer` 就地刷新，勿新建 buffer/bind group）；**不触碰 counters/DisplayKind**；UT（重复 update 零新分配）+ 既有上传路径回归 | C4/A7、M7 | T5 | ☐ |
| T7 | W2 | per-object 外观通道 | `renderer.rs`/`mesh.rs`/`line.rs`/`assets/shaders/*.wgsl`：group(1)/binding(0) 每对象 64B uniform（albedo+flags）+ 每对象 bind group；三管线布局扩 `[bg0, bg1]`；mesh FACE_COLOR 常量改读 uniform（fragment 可见）；Renderer 单源 accessor；`set_appearance` 就地更新（**不触发重建**）；uniform 与几何句柄同生共死；naga 无头 3 WGSL 编译；002 A6 台账 50 轮循环回归（无新增 ledger 行） | C5/A3、A4、M2、M9 | T4、T6 | ☐ |
| T8 | W3 | theme 语义色板 + 断言 | `app/ui/theme.rs`（新）：token（选中高亮橙/网格线/原点轴 RGB/HUD/面板背景/指示器底/视口底）；A9 断言 UT：3-token 与 core 轴色（T4 pub）及 002 语义色对表 | A1/A9、M8 | T4 | ☐ |
| T9 | W3 | texts 新键 EN/ZH | `app/ui/texts.rs`：约 20 键（菜单/工具提示/组/右键/属性组/DragValue 入口/辅助层开关/空态等）EN+ZH；两表对齐 UT；不变量 const 照旧（AXIS_X/Y/Z 不翻译） | A2/i18n、A10 | — | ☐ |
| T10 | W3 | 菜单树 + AppAction + 双路径 | `app/ui/menu.rs`（新）：`AppAction` 枚举；macOS=muda 原生树（按 T2 结论）、Win/Linux=egui 窗口内 MenuBar；双路径同 handler；locale 重建=items 层 set_text；App 菜单 Quit/Cmd+Q；单飞期 Open `set_enabled` 同步 | A4/A1、M1 | T1、T2、T8 | ☐ |
| T11 | W4 | 四区骨架 | `main.rs`：SidePanel(左树)/CentralPanel(视口)/SidePanel(右属性)/TopBottomPanel(底+消息条) 固定分区；空态提示与辅助层共存插槽；最小宽度约束（480×360 构图，A13 基础） | A5/A1、M1 | T8 | ☐ |
| T12 | W4 | 对象树升级 | `objects_panel.rs`：按类型分组+组折叠/眼睛、搜索过滤（无匹配空态）、右键三项（重命名/显隐/删除——改名即时反映树/属性/场景）、组级默认色（仅新建成员继承） | A6/A6、A8、M4 | T8 | ☐ |
| T13 | W5 | 视口辅助层接入 | `viewport.rs`：地面网格（T5 生成+T6 刷新；绘制次序=线族之首；深度参与不写深度）、世界原点三轴、方位指示器 2D（T3 方向函数）、HUD 双入口开关（工具条+角标，状态存 ViewportState）、空场景门控放宽"非空或网格开"；MAN 目视（旋转/平移线距稳定无闪烁） | A7/A11、M7 | T3、T5、T6、T8 | ☐ |
| T14 | W5 | 属性面板骨架 + 只读展示 | `properties_panel.rs`（新）：分组卡片 + DragValue（规范参数）展示 A3 全集（名称/显隐+类型专用：网格→颜色/frame→原点+长度/marker→锚点+文本/箭头→起终点），只读阶段 | A8/A3(展示侧)、M2 | T12 | ☐ |
| T15 | W5 | 状态栏 + 轻量消息条 | `status_bar.rs`（新）：加载/帧率/鼠标世界坐标（T3 参照面：Z=0 网格/相机目标平面）/当前工具提示（≥3 项）+ 轻量消息条（时间戳内联、分级色；错误窗并存期按 spec） | A9/A7、M5 | T3、T11 | ☐ |
| T16 | W6 | 单对象提交服务 + 编辑生效链 | `viewport.rs`：`ViewportState` 单对象更新 API（改字段→按 kind 重传/外观 uniform 64B 写）；app 层 `id → Appearance` 注册表（mesh 颜色 CPU 承载，core 模型不动）；T14 接编辑（≤1 帧生效并与视口/树同步）；002 A6 语义保持 | A10/A3、A4、M2 | T13、T14、T7 | ☐ |
| T17 | W6 | 添加▾内联 | `objects_panel.rs`+`main.rs`：浮动 Add 窗口移除；工具栏"添加▾"内联表单（默认值+回车即加）；对话框生命周期清理；002 A6 回归（浮动窗口相关判据按 spec 取代声明语义等价执行） | A11/A5、M3 | T12、T11 | ☐ |
| T18 | W7 | 性能协议执行 | A12：场景 C + 网格开/关 × 选中通道轮询，A11 固定协议（release/参考机/采样），p95≤33ms 且网格开 ≤ 关 +1ms；记录入 `.leon` | A12/M9 | T7、T13、T16 | ☐ |
| T19 | W7 | 回归 | M6/A10：001–003 全量（En 语义等价；002 被取代子句="入口/展示形态、属性编辑面板非目标"除外；A6 台账判据保留）；A13：zh 480×360 最小窗口复验（003 A4 判据）；路径守卫；五门禁/三平台/msrv | A10、A13/M6 | 全部 | ☐ |

## 备注

- 实现开始条件：本清单创建即满足（spec/plan 已 Approved）；负责人指示后开工。
- 波次行动规则：同波 agent 起并并行（文件互斥）；T6→T7 由同一实现者连续完成（line.rs 单一 owner）；
  每波完成跑本地门禁（fmt/clippy/core+app UT），全波后由集成轮统一验证。
- muda 相关代码仅 macOS 编译（T1 门控）；ubuntu 侧不得出现 muda 踪迹。
- MAN 记录存 `.leon`（004 验收归档）；性能记录按 A11 协议口径。

## 完成记录

- （待实现后逐条登记：T# 提交哈希 + 验收证据）
