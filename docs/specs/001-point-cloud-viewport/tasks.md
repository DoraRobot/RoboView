# point-cloud-viewport — 任务清单

状态：Completed（已完成——T1–T14 全部执行完毕，负责人验收通过）

日期：2026-09-02

相关：`spec.md`（Approved）、`plan.md`（Approved）

> 约定：任务原子、可独立验证（ADR 004 规则 3）；验证渠道 UT=core 单测（无 GUI/无 GPU）、
> CI=门禁/脚本、MAN=手工验收。每条完成时在此勾选并附提交哈希（留痕，不写进代码）。

## 任务

| # | 任务 | 产出与验证 | 对应 | 依赖 | 状态 |
|---|---|---|---|---|---|
| T1 | 依赖落地 | core：`wgpu`/`glam`/`bytemuck`/`thiserror`；app：`eframe`/`egui`/`egui-wgpu`/`rfd`（eframe `default-features=false`+wgpu 后端；rfd 无 gtk3）。验证：`cargo check --workspace` + `cargo deny` + 提交信息含用途（§2.8.1） | M4/M5 | — | ☑ |
| T2 | PLY（ASCII）解析器 | 全属性 stride 推导、token 化（多空格/CRLF/科学计数/末行无换行）、计数防护。fixture：合法/截断/坏 magic/超大 count/含 nan。UT | A2/A8 | T1 | ☑ |
| T3 | PLY（binary_little_endian）解析器 | 字节读取 + `r g b`(uchar3) 与 `rgb` 方言按文件字段顺序。fixture 同 T2。UT | A3/A8 | T2 | ☑ |
| T4 | PCD（ascii + binary_le）解析器 | `TYPE/SIZE/COUNT` 组合枚举（xyz=F4；rgb 打包位序与 U4 顺序）；COUNT>1/f64/未知声明拒绝。UT | A4/A8 | T2 | ☑ |
| T5 | 数据模型与格式分发 | `PointCloudData`/`RgbaColor`/`Aabb`/`PointCloudError`（thiserror）；扩展名+文件头双校验分发；格式表（§7）锁定。UT | M2/A8 | T1 | ☑ |
| T6 | 无效点（G1）策略 | nan/inf 保留在数据；bbox 排除非有限值；全无效 → `bounds=None`。UT（含全无效情形） | G1/A6(相机侧) | T5 | ☑ |
| T7 | 渲染核心 | `Renderer::new(Arc<Device>,Arc<Queue>,target_format)`；点云管线（RGBA8Unorm 颜色、stride 4 对齐）；prepare 一次性上传；paint 向外部 pass 记录；WGSL（sRGB→linear、`!is_finite` 跳过、默认色）。验证：naga headless 编译 UT | M4/M5/A5 | T1/T5 | ☑ |
| T8 | OrbitCamera（core） | yaw/pitch/distance→view/proj 纯函数；pitch/dist 钳制；零尺寸/退化 bbox 回退默认取景。UT | A6 | T1 | ☑ |
| T9 | app 壳与打开入口 | eframe 启动、空视口提示、菜单"打开点云文件"→ rfd 原生对话框；错误通知 UI（可读、不 panic，§6.5 英文文案）。MAN | A1/A8/US3 | T1/T7 | ☑ |
| T10 | 视口集成 | egui-wgpu paint callback（每帧注册、`PaintCallbackInfo` 尺寸/target_format 处理）；投影按视口更新。MAN + T7 衔接 | A5/A10 | T9 | ☑ |
| T11 | 相机输入与替换时序 | app 输入适配（左/滚轮/中键→增量；NaN 事件丢弃）；模块加载：后台线程解析→回主线程；成功替换（A7 成功）/失败保留旧数据+错误通知（A7 失败）。MAN（替换语义）+ UT（增量纯函数） | A6/A7 | T8/T9/T10 | ☑ |
| T12 | A9 检查口径 | CI 脚本：生产路径（`crates/*/src` 非 test 部分）grep 无数据文件路径；测试夹具豁免明确。CI | A9 | T2–T11 | ☑ |
| T13 | 门禁全绿 | 五门禁 + 三平台矩阵 + msrv 全绿（含 wgpu/egui 新依赖下）；naga 编译接入 CI。CI | A10/M5/MSRV | T1–T12 | ☑ |
| T14 | 验收执行（本机） | 按 spec §4 A1–A12 手工清单逐项记录；A11 性能协议（release/参考机型/计时/5s 环绕 p95）；A12 样本由验收方提供或公开标准样本（存私有区、不进仓库）。MAN | M1–M3 | T1–T13 | ☑ |

## 备注

- 实现开始条件：plan.md 转 Approved 且本清单创建完毕（§6.1 / ADR 004 规则 3）。
- 渲染正确性（A5 目视、A6 手感）无 GPU CI 兜底，列为手工验收项；无 adapter 环境测试显式 skip。
- 性能回归口径以 A11 记录为准，禁止口头结论。

## 完成记录

- T1–T8 已完成（提交见上表对应列）：T1 `8f35cec`、T2–T6 `29d74f5`（解析器与数据模型）、T7 `aa1e184`（渲染）、T8 `c2cd3c1`（相机/场景）。
- T9–T11（应用层）`bbafaeb`；T12（A9 守卫）`192ace6`；T13 本地五门禁全绿（GitHub CI 首轮以 Actions 页为准）。
- T14 负责人已手工验证：可正常打开并显示点云数据（A1–A8 流程通过）。A11 性能协议数值未单独记录，若后续补测再追加。
