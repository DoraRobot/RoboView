# display-types — 方案

状态：Approved（已批准）

日期：2026-09-02

相关：`spec.md`（Approved）；ADR 004、ADR 006；首功能 `docs/specs/point-cloud-viewport/`（plan.md §3.2 契约修订参见本文 §7 风险表）

修订记录：2026-09-02 起草；经负责人评审无异议批准（In Review → Approved）。

## 1. 概述

本方案确定 HOW：多对象场景容器化（稳定 id + 追加语义）、共享深度管线族、
OBJ/路径解析器、坐标系与标记（UI 添加）、固定侧栏对象列表与 Fit 取景、
A9 守卫扩展与全量回归。核心仍无 GUI 依赖；新增依赖为零（侧栏=egui 内置 `SidePanel`）。

## 2. 依赖清单

**无新增运行时依赖**：`egui::SidePanel` 即满足侧栏需求；egui_dock 按 spec §5 推迟。

## 3. 跨层契约（接口与数据流）

### 3.1 多对象场景（core/scene）

```text
pub struct SceneObject<D> {
    pub id: u64,            // 单调递增（core 内部计数器）
    pub name: String,       // 文件 stem 或 "PointCloud 2"（core 生成，UI 只显示）
    pub visible: bool,      // core 所有，驱动绘制跳过
    pub object: D,
}
pub struct Scene<D> { camera: OrbitCamera, objects: Vec<SceneObject<D>> }
// ops：add（分配 id 返回）、remove(id)、toggle_visible(id)、iter/iter_visible、
//      bounds_union() -> Option<Aabb>（Fit/首取景用；为空含全无效场景）
```
- `OrbitCamera::framing(bounds)` 扩展：新增 `Aabb::union_of(&[Aabb])`（core/io 数据层）与
  场景级求并入口（`scene` 提供，app 调 Fit/首取景）。
- 删除 = `Vec::remove` → drop（wgpu 延迟销毁，A6 由构造满足）。

### 3.2 显示类型闭集枚举（core/displays；非 trait object）

```text
pub enum DisplayObject { PointCloud(PointCloud), Mesh(Mesh), Path(Path), Frame(Frame), Marker(Marker) }
impl DisplayObject {
    pub fn kind(&self) -> DisplayKind;           // UI 类型列（文案在 app texts.rs）
    pub fn bounds(&self) -> Option<Aabb>;
}
```
- 每类型一个文件/模块 + 各自 `struct { data, gpu: Option<Arc<XxxMesh>> }` 模式（沿用首功能）。
- 上传/绘制分派仅出现在 render 的**一个 match 点**（编译器穷尽检查）；
  未来插件化时再开放为 trait-object + 管线注册表（迁移面窄）。

### 3.3 渲染契约（共享深度；首功能契约扩展点）

- **宿主侧**：`NativeOptions.depth_buffer = 24`（egui-wgpu 自动提供每帧 Clear(1.0)、
  随 surface 自动重建的 Depth24Plus）。
- **Renderer 参数化**：`Renderer::new(Arc<Device>, Arc<Queue>, target_format, depth_format: TextureFormat, sample_count: u32)`；
  所有场景管线统一按（Depth24Plus, samples=1）构建（管线与 pass 的深度/采样数必须严格相等——已核验 wgpu-core check_compatible）。
- **不变式**（沿首功能 §3.2）：设备/队列/交换链由宿主注入；单帧单提交；不建 encoder/pass/submit。
- **管线族**：`MeshPipeline`（索引缓冲、`DepthBiasState` 偏置、法线在顶点（CPU 面法线 × 顶点复制））、
  `PointPipeline`（现有，加 depth 配置 + 偏置 0）、`LinePipeline`（路径/坐标轴，严格 Less、无偏置影响）；
  共享 view-proj uniform（每帧一次写入，几何世界坐标直出——本期无逐对象变换）。
- **WGSL**：每管线自有着色器（复用点云 shader + 新增 mesh/path/lines），全部 naga headless 校验（沿用 T7 模式）。
- **覆盖层**：文本标签走 egui painter（屏幕空间投影：core 提供 `anchor_to_screen(view_proj, viewport) -> ScreenPos` 纯函数，可单测）；不参与深度。

### 3.4 加载与列表数据流（app）

```text
菜单（文件：OBJ/路径；侧栏：Add frame/Add marker）→ 后台线程（io 解析，纯函数，无 GUI）
  → Ok → app 主线程：场景 add（追加；首对象/空场景时按并集取景）→ renderer.upload_*（对象级）
  → 列表（egui::SidePanel，行 id=object id，egui::Id::new(id)）
  → 行操作：toggle_visible/remove（按 id）；Fit 按钮 → framing(场景并集)
  → Err → 保留已有对象 + 错误通知（A10）。
```
- 显隐只影响绘制跳过；删除 = 移除 + drop。
- 渲染资源台账：render 维护每类句柄创建/销毁计数（debug tracing），供 A6 判据。

### 3.5 解析器（core/io）

- `io/obj.rs`：ASCII 字节 token 化（复用 PLY 的私有 text 工具抽取为 `io/text.rs`？——抽取 `io/ascii_text.rs` 私有工具：行切分（CRLF/末行）、token 化、数值解析（含科学计数））；OBJ 规则按 spec §7 F1 拒绝规则表；计数预校验（u128）。
- `io/path_xyz.rs`：CSV/XYZ 规则按 spec §7 F2；含分隔符/标题行/行号错误。
- 错误：每格式族 `thiserror`（`ObjError`/`PathError`）+ 公共 `PointCloudError` 分发（新增变体或独立 `MeshLoadError`？——裁定：公共 `LoadError` 枚举变体网格/路径/点云 + 各解析器返回；沿用"扩展名+首行冒烟"双校验，明确弱化口径（spec §6 已定））。
- NaN/Inf：与 G1 对齐（保留 + 渲染裁剪）。

## 4. 关键实现决策

| 决策 | 选择 | 理由/候选放弃 |
|---|---|---|
| 深度路线 | 共享深度（depth_buffer=24） | 核验：egui pass 自带 depth、自动重建；离屏路线引入 resize/带宽/回读复杂度（收益本期无用）——两个评审一致 |
| 网格着色 | CPU 面法线 + 顶点复制（3v/面）、双面不剔除、恒定色、无光照或头灯 | 放弃 WGSL 屏幕导数（方向随屏幕导数翻转、背面不稳定）；无索引缓冲换实现简单与可单测 |
| 共面政策 | mesh `DepthBiasState`（constant + slope_scale，参数 plan 记录供 M3 校准）；线严格 Less | 扫描数据贴面场景为等深典型；线图元不受 polygon offset 影响——需在 P2 校准参数 |
| 列表 | `egui::SidePanel` | egui_dock 推迟（首个面板强定义 TabViewer/状态面、无持久化非目标） |
| 取景 | 首对象（空场景）并集取景；后续不动；Fit 按钮 | 避免连续添加跳视角（spec §6 裁定） |
| 对象渲染 | 单一 `Vec` 顺序 + 按管线类型分组绘制（每帧一个回调画全部可见对象） | 列表/显隐/绘制顺序三处天然一致；避免逐对象切管线 |
| 资源释放 | 删除即 drop（延迟销毁语义）；台账计数 | A6 判据可执行（UT+MAN） |

## 5. 实施顺序（P0–P5，回归最小化）

| 阶段 | 内容 | 回归证据 |
|---|---|---|
| P0 | 语义收口（已完成）：spec、A9 守卫扩展正则（`.obj`/`.csv`/`.xyz`+菜单 filter）；docs 约定 | CI 绿 |
| P1 | core 容器化：Scene 多对象 + id（先只装 PointCloud） | 首功能 A1–A9 全量回归 |
| P2 | 共享深度：eframe depth_buffer=24 + 全部管线统一（先只点云） | A5（颜色链路不受影响——仅深度可见性）+ A11 重跑 |
| P3 | 类型逐个：OBJ（网格+深度偏置，最重）→ 路径 → 坐标轴 → 标签/箭头（覆盖层） | 每类型：解析 UT + naga UT + MAN 单类显示 |
| P4 | 侧栏列表 + Fit + 显隐/删除 | A6（台账）/A8（显隐性能） |
| P5 | 场景 C 组装 + 协议 P 执行 + M5 性能 + 首功能回归复核 | A1–A12（除累计迭代）+ A11 |

## 6. 风险与对策

| 风险 | 对策 |
|---|---|
| egui-wgpu 的 pass 深度/采样数与管线不一致 → 等值校验失败（已核验 check_compatible） | P2 先点云验证；管线创建统一从 Renderer 构造参数下发（单一来源） |
| mesh 深度偏置参数不当 → 贴面点云仍被剔除（M3 flaky） | 参数进 plan 常量表 + M3 协议校准轮（P5）；A2 按 P 重测 |
| 全量重传遗漏新类型（格式变化重建路径） | `sync_renderer` 推断全对象上传循环 + 类型齐备性单测（空桶断言） |
| OBJ/CSV 无 magic → 错误路径弱化双校验的边界 | 首行冒烟校验（OBJ: 行1 "v"或"vn"/f 出现；CSV: 数值行）+ A10 错误语义绑定 |
| 首功能回归面：容器化 + 深度化改动其 A5/A11 | P1/P2 各自带独立回归证据；回归范围见 spec M6/A11 |
| 场景 C 样本供给 | 验收方/公开标准样本（私有区），A12 模式；100k 面 OBJ 提及生成脚本（验收辅助、不入库） |
| 标签投影精度（DPI/比例） | 纯函数 + UT（比例不敏感，沿首功能 rect 口径） |

## 7. 产出物与流程

`spec.md`（Approved）→ 本方案（Draft → 人评审 → In Review → Approved）→ `tasks.md`（P0–P5 原子任务，映射 spec A1–A12/M1–M6，标注渠道）→ 实现 → Validate（协议 P + 场景 C + 首功能回归 A11）。
方案批准前不得开始实现（§6.1 / ADR 004 规则 3）。
