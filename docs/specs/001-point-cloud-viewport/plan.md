# point-cloud-viewport — 方案

状态：Approved（已批准）

日期：2026-09-02（2026-09-02 评审修订；修订说明：渲染契约、接口与数据流、格式矩阵、相机防护、依赖账本——对应 spec.md 2026-09-02 修订）

修订记录：2026-09-02 修改——4 视角评审后全面修订；经负责人评审通过，由 Draft 转 In Review；2026-09-02 负责人批准，转 Approved。

相关：`spec.md`（Approved）；ADR 004、ADR 006

## 1. 概述

本方案确定 HOW：系统原生对话框、自研 PLY/PCD 解析器、core 内点云显示类型与最小渲染管线、
egui 视口与轨道相机输入适配。全部改动沿 ADR 006 分层：

- `roboview-core`：io（解析/格式）、displays（点云显示类型）、render（管线/上传）、scene（相机状态/数学、包围盒）。
- `roboview`：eframe 壳、egui 菜单/视口/错误通知、输入事件→相机增量映射。

## 2. 依赖清单

> 范围注：`eframe`/`egui`/`wgpu`/`glam`/`bytemuck` 已由 ADR 006 定案（含理由），本表列出
> **实现期新增**的 crate；每个 crate 进入 Cargo.toml 时在提交信息写明理由（§2.8.1）。

| crate | 归属层 | 用途 | 许可预期 |
|---|---|---|---|
| `egui-wgpu` | app | egui 视口的 GPU 集成（paint callback 渲染场景） | MIT OR Apache-2.0 |
| `thiserror` | core | `PointCloudError` 类型化错误（§2.5.1） | MIT OR Apache-2.0 |
| `rfd` | app | 系统原生文件对话框（macOS/Windows/Linux 三平台） | MIT（单一）——采纳时以 deny 校验为准 |

许可红线：**rfd 不得开启 `gtk3` feature**（LGPL）。egui 族/wgpu 族的传递依赖（winit=Apache-2.0、
objc2=MIT、zbus/ashpd=MIT、windows-sys=MIT OR Apache-2.0、unicode-ident 组合等）已在 deny.toml
白名单覆盖（评审已实证）；glam 为 MIT OR Apache-2.0（修正此前笔误）。

## 3. 跨层契约（接口与数据流）

### 3.1 core 公共 API 面（小、稳定、可测，§2.4.4）

```text
io::load_point_cloud(path: &Path) -> Result<PointCloudData, PointCloudError>
io::PLY/PCD 子模块私有；PointCloudData { positions, colors: Option<Vec<RgbaColor>>,
                                           point_count, bounds: Option<Aabb>, source: Format }
displays::PointCloud { data: PointCloudData }        # 纯数据 + 元信息；持久化到替换前
scene::OrbitCamera { target(yaw) ... }               # 状态 + view/proj 数学（含钳制）
scene::Scene { displays, camera }                    # 单显示实例（本版）
render::Renderer::new(device: Arc<Device>, queue: Arc<Queue>, target_format: TextureFormat)
render::Renderer::prepare(...) / paint(...)          # 分阶段 API，见 §3.2
```

### 3.2 渲染契约（与 egui-wgpu 的对齐约束，最高风险项）

- **设备所有权**：`Instance/Adapter/Device/Queue/Surface/交换链` 全部由 **eframe 创建并持有**
  （经 `egui_wgpu::RenderState` 暴露）；core `Renderer` **不得**自建设备/交换链，只通过
  `Renderer::new(Arc<Device>, Arc<Queue>, target_format)` 注入并拥有管线/缓冲/bind group/WGSL。
- **单帧单提交**：egui-wgpu 每帧仅提交一次；绘制必须发生在**外部已开启的 `RenderPass`** 上
  （`CallbackTrait::paint` 阶段），禁止自建 encoder/pass/submit。
- **上传时机**：数据（静态点云）在 `CallbackTrait::prepare` 阶段一次性 `write_buffer`（每帧零上传，
  满足 §2.10.1）；替换文件时旧缓冲随 `drop` 进入 wgpu 延迟销毁，覆盖 A7 安全释放。
- **同 wgpu 实例约束**：core 与 app 依赖的 `wgpu` 版本必须一致（同一 semver 主版本），
  否则跨 crate 类型不兼容——写入 §9 风险表。
- **管线生命周期**：管线在首次拿到 `RenderState`（窗口创建后）惰性创建；`target_format` 改变
  （罕见，跨屏移动）时校验并按需重建；视口回调每个 egui 帧重新注册，不跨帧持有。
- **视口尺寸**：每帧经 `PaintCallbackInfo` 取物理像素尺寸，用于投影宽高比与帧资源更新。

### 3.3 所有权链与替换语义

```text
对话框（app/rfd）→ 后台线程读取+解析（core/io 纯函数，无 GUI 依赖）→ 结果回主线程：
  Ok(PointCloudData) → Scene 单实例替换：旧 displays/drop；新数据上传 GPU（prepare）；
                       错误通知清空；失败时（A7/US2）：保留旧点云 + 可读错误。
  解析后 CPU 点数组是否保留：保留（供后续拾取/统计）；占用以 100 万点 × 16B ≈ 16MB 为上限
  意识，不做内存优化（非目标）。
```

## 4. 关键实现决策（含候选与放弃理由，§4.2.2）

| 决策 | 选择 | 候选与放弃理由 |
|---|---|---|
| 文件对话框 | rfd（原生、三平台） | 备选：另一原生实现（平台 FFI）——保持 spec"原生对话框"约束；egui 自绘对话框**非原生**，须修订 spec 才可用，不作合规备选 |
| 解析器 | 自研字节级 | 放弃第三方 loader：3rd party 引入额外依赖/许可面/格式子集不可控（§2.8.1）；本版子集小（§7），手写可控且构成 io 模块自有资产 |
| 点图元 | PointList（1px） | 放弃实例化四方图（`vertex_index` 展开，约 4M 顶点）：本版验证链路，观感改进（>1px、防抖动）延迟立项；1px 限制与无深度测试（egui pass 无 depth attachment，单云场景无碍）记入验收记录已知项 |
| 相机 | 球坐标轨道（core） | 放弃 6-DOF/look-at 工具相机：本领域常用轨道交互；球坐标=纯函数，可单测（A6） |
| 颜色布局 | `u32`（Rgba8Unorm）+ 顶点着色器 sRGB→linear | 文件为 3 字节：WebGPU 要求 stride 为 4 的倍数，3 字节布局非法；egui 表面 `Srgb` 目标，直接输出会偏色——顶点色统一转 linear（片元输出经硬件 sRGB 编码） |
| 线程模型 | 读取+解析后台线程（`std::thread`，无 async 栈依赖）；完成后经 egui 事件回主线程 | 防止 1M 点解析阻塞 UI 帧（M3 2s 量级）；core 解析纯函数天然可线程化 |
| unsafe | 预期零 unsafe | 全部 wgpu/bytemuck 安全 API；不可避免时按 §2.6 `// SAFETY:` 最小块 |

## 5. 字节级格式矩阵与拒绝规则（plan 新增，对应 spec §7）

- **PLY**：头部解析到 `end_header`（精确偏移，含 CRLF）；每顶点 stride 按**全部已声明属性**
  推导（list 属性显式拒绝）；元素可乱序，找 `vertex` 元素（不保证第一个）；ASCII 用 token 化
  （多空格、CRLF、末行无换行、科学计数）。
- **PCD**：`FIELDS/TYPE/SIZE/COUNT` 并行数组校验长度一致；允许组合枚举：
  `x y z` 全 `F 4`（v0.7 + `FIELDS x y z`）；`rgb` = `F 4`（PCL 惯例位序：
  LE u32 读取后 `r=(v>>16)&0xff, g=(v>>8)&0xff, b=v&0xff`）或 `U 4`（直接四字节 r/g/b/a? 裁定为 U4 = R,G,B,? 顺序=文件顺序）。`COUNT>1`、f64 坐标、`DATA ascii/binary` 之外的声明：拒绝。
- **无效点（G1）**：`nan/inf` 保留在数据数组；渲染 shader 跳过（`!is_finite`）；包围盒计算排除
  非有限值；`bounds=None`（全无效）时相机用默认距离（见 §6）。
- **计数防护**：声明 count 与实际文件字节按 stride 上限预校验；超出 → A8 错误路径，防 OOM。
- **大端**：不支持子集；PCD 无端序标记，小端解析器遇大端文件为已知限制（记录，不强制防御）。
- **fixture 策略**：每格式每子集至少一组内存构造样例（合法/截断/坏 magic/超大 count/字段错位/
  含 nan），按 §7 G1 预写测试字节构造函数——不带路径字符串，确定性（§2.9.3）。

## 6. 相机与退化防护（核心数学，A6 可单测）

- `OrbitCamera`（core/scene）：目标点、yaw、pitch、distance → view/proj 纯函数；
- 钳制：pitch ∈ [−π/2+ε, π/2−ε]；distance ≥ ε_min（如 0.01×包围盒对角）；滚轮缩放上下限；
- 零尺寸/退化包围盒（单点云、全同点、全无效点）→ `bounds=None` → 取景使用默认距离与原点目标；
- 输入防御：egui 事件增量计算在 app 输入适配层，NaN/Inf 事件丢弃。
- app `ui/camera.rs` 仅为"egui 事件 → 轨道增量"的薄适配；不持有相机状态。

## 7. 测试策略与验收支持

- **core 单测**（无 GUI/无 GPU）：PLY/PCD 解析（fixture 矩阵）、sRGB 转换、相机钳制与退化、
  bbox 计算、A8 错误路径——全部确定性自动化。
- **WGSL 编译**：naga 对 shader 做 headless 编译单测（CI 无 GPU 也通过）。
- **GPU/适配器**：需要 adapter 的测试在无适配器环境显式 skip 并记录原因；渲染正确性留三平台手工验收。
- **手工验收清单**：按 spec 表 A 行的 MAN 条目逐项执行并记录（A6 交互、A11 性能协议、
  A12 样本——样本由验收方提供/公开标准样本，存放于私有区，**不进入仓库**）。
- **A9 检查口径**：CI 脚本 grep 生产路径（`crates/roboview*/src` 的非 `#[cfg(test)]` 部分）不含文件路径；
  测试夹具明确豁免。

## 8. 风险与对策

| 风险 | 对策 |
|---|---|
| core 与 egui-wgpu 的 wgpu 版本不一致 → 跨 crate 类型不兼容（最大风险） | Cargo 依赖版本对齐（同主版本）；CI/审阅时核对；必要时借 `wgpu` feature 约束 |
| callback API 版本敏感性（egui 各版本签名变动） | 锁定 egui/eframe 版本于 Cargo.lock（已提交）；升级走专用 commit（§2.8.4） |
| wgpu 依赖树过宽，deny 白名单缺条目 | CI deny 反馈后逐条补充（宽松许可）并记录 |
| 某传递依赖 MSRV 高于 1.85 | msrv job 实测；若冲突，评估升级 MSRV（需明确记录）或降级版本 |
| rfd 平台/许可问题 | spec 合规的备选原生方案；gtk3 feature 禁用 |
| 解析器边界遗漏（stride/NaN/截断） | §5 矩阵先行；fixture 覆盖；拒绝规则表驱动 |

## 9. 产出物与流程

`spec.md`（Approved，评审修订后）→ 本方案（Draft → 人评审 → In Review → Approved）→
`tasks.md`（原子任务，与 spec 表 §4 一一对应，标注验证渠道）→ 实现 → Validate（自动 + 手工）。
方案批准前不得开始实现（§6.1 / ADR 004 规则 3）。
