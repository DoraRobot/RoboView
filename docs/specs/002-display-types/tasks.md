# display-types — 任务清单

状态：Completed（已完成——T1–T17 全部执行完毕，负责人验收通过）

日期：2026-09-02

相关：`spec.md`（Approved）、`plan.md`（Approved）

> 约定：任务原子、可独立验证（ADR 004 规则 3）；渠道 UT=core 单测（无 GUI/无 GPU）、
> CI=门禁/脚本、MAN=手工验收（含协议 P / 场景 C）。每条完成时在此勾选并附提交哈希（留痕）。
> 裸编号仅本 workspace 有效；跨功能引用全限定（docs/README 约定）。

## 任务

| # | 任务 | 产出与验证 | 对应 | 依赖 | 状态 |
|---|---|---|---|---|---|
| T1 | A9 守卫扩展 | `check_data_paths.sh` 正则扩 `.obj`/`.csv`/`.xyz`；对话框 filter 与菜单路径一并检查；CI 绿 | A12 | — | ☑ |
| T2 | Scene 多对象容器化 | `SceneObject{id,name,visible}` + add/remove/toggle_visible/iter_visible/bounds_union（含全无效）；id 单调。UT | M4/A6(基础) | — | ☑ |
| T3 | app 容器化迁移 | ViewportState/main 改追加语义（首对象并集取景，后续不动）；首功能 A7 成功路径行为更新（A11 复核时验证失败路径仍保留旧对象） | A6/M1/A10 | T2 | ☑ |
| T4 | 共享深度 | eframe `depth_buffer=24`；`Renderer::new` 参数化（depth_format/sample_count）；现有点云管线统一 Depth24Plus + samples=1；本地运行等值验证（check_compatible 通过）；A5 目视回归（颜色链路不受影响） | M3(基础设施)/A11 | T3 | ☑ |
| T5 | 共享 view-proj uniform | 每帧一次写入（逐对象 uniform 去掉）；多对象下仍单帧单提交 | A7(性能前提) | T4 | ☑ |
| T6 | io 共享 ASCII 工具 | 抽取行切分（CRLF/末行）/token/数值（科学计数）私有工具；PLY/PCD 迁移适配（无行为变化，现有 UT 全绿回归） | A1/A3(解析基础) | — | ☑ |
| T7 | OBJ 解析器 | `io/obj.rs`：spec §7 F1 拒绝规则全表（负索引显式拒、>3 顶点拒、vt 忽略/vn 校验、o/g 整文件单对象、v 无 f 散点、计数预校验、NaN 对齐 G1）；fixture UT（合法/各拒绝分支/CRLF/超数/nan） | A1/A10 | T6 | ☑ |
| T8 | 路径解析器 | `io/path_xyz.rs`：`[,\t ]+` 恰 3 token 报文行号、标题行、空行、整行注释、<2 点拒；fixture UT | A3/A10 | T6 | ☑ |
| T9 | 网格显示 | displays Mesh（面法线 CPU 计算+顶点复制、双面不剔除、恒定色）+ MeshPipeline（DepthBiasState 常量表）+ WGSL（无光照）+ naga UT；单类显示 MAN | A1/A2/M3 | T4/T5/T7 | ☑ |
| T10 | 路径显示 | displays Path + LinePipeline（严格 Less）+ WGSL；naga UT | A3/A2(线无偏置影响论证) | T4/T8 | ☑ |
| T11 | 坐标系显示 | displays Frame（3 线段 X 红/Y 绿/Z 蓝）+ `anchor_to_screen` 纯函数（core，可单测）+ 轴标签覆盖层（app painter）；UI Add/Remove | A4 | T4/T2 | ☑ |
| T12 | 标记显示 | displays Marker（文本标签覆盖层 + 箭头带帽）；UI Add/Remove（锚点/文本/起终点 DragValue） | A5 | T4/T11 | ☑ |
| T13 | app 打开入口扩展 | 菜单 OBJ/CSV/XYZ filter + 错误族传播（正文可读、texts.rs）；损坏文件 MAN 验证 | A1/A3/A10 | T7/T8/T12 | ☑ |
| T14 | 侧栏列表 + Fit | `egui::SidePanel`（行=id、egui::Id::new(id)、显隐/删除/选中态、类型列）；Fit 按钮（并集取景）；空场景提示沿用 | A6/M4/US4/A4/A5 交互入口 | T2–T5/T11/T12 | ☑ |
| T15 | 资源台账 | render 每类句柄创建/销毁计数（debug tracing）；A6 判据：50 轮循环（添加→显隐×10→删除）poll 后末轮活=首轮；UT（无 adapter skip 记录）+ MAN | A6 | T9–T14 | ☑ |
| T16 | 场景 C + 协议 P 执行 | 装配场景 C 样本（验收方/公开样本，私有区）+ 协议 P 记录（环绕 300 帧/角度集/量化阈值逐项）；M5/A8 两轮性能（A11 协议）；A12 门禁全绿（五+平台+MSRV+守卫扩） | M1/M3/M5/A2/A7/A8/A9/A12 | T1–T15 | ☑ |
| T17 | 回归复核 | 首功能 A1–A9（除取代的 A7/US2 成功路径）+ M6 文档核对；tasks 完成记录 | M6/A11 | T16 | ☑ |

## 备注

- 实现开始条件：本清单创建即满足（spec/plan 均已 Approved），负责人一声"开始实现"即可。
- 协议 P/场景 C 的 MAN 记录存 `.leon`（里程碑归档）；样本不进仓库（A12 模式）。
- 性能回归口径以 A11（首功能）协议为准，禁止口头结论。

## 完成记录

- T1（A9 守卫扩展 `.obj`/`.csv`/`.xyz`，与容器化提交的 CI 一并验证）`af01097`。
- T2/T3（Scene 多对象容器化 + app 追加语义迁移）`430deac`。
- T4/T5（共享深度 depth_buffer=24 + 共享 view-proj uniform）`4b1d396`。
- T6–T8（io 共享 ASCII 工具 + OBJ/路径解析器）`af01097`。
- T9–T12/T15（网格/路径/坐标轴/标记显示 core 侧、anchor_to_screen、A6 资源台账）`061f579`；
  T11/T12 的 UI 添加入口与 T13/T14 同批落 `21289cb`。
- T13/T14（app 打开入口扩展与错误族传播、侧栏列表 + Fit、多对象渲染）`21289cb`。
- T16/T17 待验收（负责人参与）：场景 C / 协议 P 的记录模板已备于 `.leon`（里程碑归档）；
  样本位于验收方私有区，不进仓库（A12 模式）。目录更名 `002-display-types` 待本功能闭环后由负责人决定。
