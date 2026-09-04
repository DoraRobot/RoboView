# picking-selection — 任务分解

状态：In Progress（实现完成 T1–T13；MAN 目视验收进行中，见完成记录）

日期：2026-09-04

相关：`spec.md`（Approved）、`plan.md`（Approved）

> 约定：任务原子、可独立验证（ADR 004 规则 3）；渠道 UT=core/app 单测、MAN=手工验收、CI=门禁。
> 跨 workspace 引用全限定；本工作区仅 A1–A12（路径守卫 = `001-point-cloud-viewport spec.md` A9）。
> 文件互斥：同一文件任何时刻仅一个执行者；pick.rs 单 owner 连续（T1–T6 同一实现者）。
> 网络：实现者如遇网络问题，按既有约定启用代理通行（细则仅存于 `.leon/`，不入仓）。

## 任务表

| # | 子任务 | 文件（互斥） | 依赖 | 验证（UT/CI） | A 映射 | 波次 |
|---|---|---|---|---|---|---|
| T1 | pick.rs 骨架 + 射线-三角形（Möller–Trumbore, 逐三角形） | `core/render/pick.rs`（新） | — | 单测：投影/平移/退化三角/NaN | A2 (mesh) | W1 |
| T2 | 点云桶索引（均匀哈希 bucket）+ 屏幕半径 r=8px 近邻 | 同上（续） | T1 | 单测：已知点命中/间隙未命中/桶重分配 | A2/A3 | W1 |
| T3 | 线族 capsule 命中（path/frame 三轴/marker 箭头）；δ 随命中深度折算 | 同上（续） | T1 | 单测：远近距离阈值/轴线段 | A2 (line) | W1 |
| T4 | marker 文字锚框命中（anchor_to_screen 复用，近似框） | 同上（续） | T1 | 单测：锚框/遮挡语义 | A1/A2 (marker) | W1 |
| T5 | pick_objects 仲裁（t 最近、等距取先加者、非有限/退化守卫） | 同上（续） | T2–T4 | 单测：仲裁序/守卫 | A2/A5 | W1 |
| T6 | pick_rect 框选（投影 8 角 Aabb 屏幕矩形相交；接触即选） | 同上（续） | T5 | 单测：接触/离散/空矩形 | A9 | W1 |
| T7 | 光标锚定缩放纯函数（缩放前后重投影漂移 ≤ 视口高×0.5%） | `core/render/camera_math.rs` | 现有 pointer_world | 单测：漂移界/极限 zoom | A11 (UT) | W1 (并行) |
| T8 | camera_input 键位表（中键轨道/Shift+中键平移/滚轮=光标锚定缩放/左键 press/drag 事件原语） | `app/ui/camera_input.rs` | T7 | 单测：互斥与方向/事件原语 | A11 | W2 |
| T9 | viewport 点选接线（≤4px=点选→pick→选中=appearance+树+属性镜像） | `app/ui/viewport.rs` | T5/T8 | UT+MAN（三区镜像 A4） | A1/A3/A4 | W3 |
| T10 | viewport 框选+橡皮筋（2D overlay）+选择集状态（SelectSet） | 同上（续） | T6/T9 | UT：矩形冻结相机/橡皮筋/结算 | A9/A10 | W4 |
| T11 | 修饰键 Shift/Ctrl/Esc + F/Delete 集合版 + 面板概要入口 | 同上（续） | T10 | UT+MAN | A6/A10 | W5 |
| T12a | 属性面板多选降级（"已选 N 个对象"概要，禁编辑）+ texts 新键 | `app/ui/properties_panel.rs`、`app/ui/texts.rs` | T10 | 单测：概要/禁用 | A10 | W5 (并行) |
| T12b | 状态栏选择集计数/工具提示 | `app/ui/status_bar.rs` | T10 | 单测：计数 | A10 | W5 (并行) |
| T12c | 树合集高亮（多选行亮色组） | `app/ui/objects_panel.rs` | T10 | 单测：合集 | A10 | W5 (并行) |
| T13 | 回归与性能（A5 相机无关/A7/A12 高速、001-004 全量、A9 守卫、release 冒烟、plan/spec 对账） | 各文件 | T8–T12 | CI+MAN | A5/A7/A8/A12 | W6 |

## 波次编排

- W1：T1–T6（pick.rs 单 owner 连续）+ T7（并行，另一文件）。
- W2：T8（依赖 T7 与相机数学）。
- W3：T9（点选接通，先让点可选 + 三区镜像）。
- W4：T10（框选+选择集）。
- W5：T11 + T12a/b/c（视图补全，并行写互斥文件）。
- W6：T13（全量回归/性能/文档）。

## 完成记录

- 运行修正（轴线悬浮终修，2026-09-05）：`origin_rows` 从 grid strips 直接提取（y=0 行=X 红、x=0 列=Y 绿），viewport 只做着色——不含独立几何；网格覆盖到哪线就到哪（含"网格窗覆盖 one axis row 时仍只染那一行"的钉死 UT）。此前两个版本（独立窗计算）均已回退。
- 实现谱系：W1 `7b3548b`（pick.rs 拾取核心 55 UT + camera_math 光标锚定偏移 + line 常量共享）/ W2–W5 `f2a24d8`（键位表 + 光标锚定光标缩放 cursor_zoom + 点选/框选 + 选择集 + 修饰键/键盘协议 + 面板联动 + texts 键）。
- 运行修正（Magic Mouse 滑动语义反馈，2026-09-04）：Blender 式 **Magic Mouse Emulation** —— `Point` 单元（触控表面/魔法鼠标）滑动=轨道、Shift+滑动=平移、Cmd+滑动=缩放（光标锚定）；真滚轮（Line/Page）保持光标锚定缩放；中键/Alt 模拟三键全部保留。`apply_scroll_camera` 纯函数 + 3 UT。
- 运行修正（无中键鼠标反馈，2026-09-04）：Blender 式**模拟三键** —— Alt+左键=中键、Shift+Alt+左键=Shift+中键；Alt 按下时点选/框选不触发（spec A11 追加两行）。
- 门禁最终态：clippy 全零（block v0.1.6 future-incompat 除外）、`cargo test --workspace` 92+243 全绿（texts warn-once 为已知并行偶发、单跑即过）、A9 守卫绿、release 冒烟 0 panic。
- 实现偏差回填：①`PickContext.world_per_pixel_scale` 由 app 从 `vertical_fov()` 推导（fov 无法从组合矩阵反推）；②marker 文字为 overlay 类仲裁（最上层标签优先,再按 t）——与 005 描绘的标签在上层语义一致；③等距取**先加者**（最早绘制),tasks 表注释相应先行修正；④mesh 双面命中（渲染无剔除）；⑤点云桶索引锚定 Aabb.min；⑥箭头常量由 line.rs `pub(crate)` 共享（消除镜像漂移）；⑦spec §6 的 app 层索引缓存留待性能优化（当前先命中后 bucket,场景规模实测点云 ≤1e7 可接受,超规模记录）。
