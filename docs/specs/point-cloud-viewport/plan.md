# point-cloud-viewport — 方案

状态：Draft（草案）

日期：2026-09-02

相关：`spec.md`（Approved）；ADR 004、ADR 006

## 1. 概述

本方案确定 HOW：系统原生对话框入口（`rfd` 或等效许可兼容实现）、自研 PLY/PCD 解析器、
core 内的点云显示类型与最小渲染管线、app 内的 egui 视口与轨道相机。全部改动沿 ADR 006
的分层：core（wgpu/glam/bytemuck + 解析/显示/渲染）和 app（eframe/egui/rfd + 相机交互）。

## 2. 依赖清单（新增，逐个理由，§2.8.1）

| crate | 归属层 | 用途 | 许可预期 |
|---|---|---|---|
| `wgpu` | core | GPU 抽象，渲染管线 | MIT OR Apache-2.0 |
| `glam` | core | 数学（Vec3/Mat/弧度） | MIT |
| `bytemuck` | core | GPU 数据字节布局 | MIT OR Apache-2.0 |
| `rfd` | app | 系统原生文件对话框（macOS/Windows/Linux） | MIT OR Apache-2.0（**采纳时以 deny 验证为准**） |

规则：上述每个 crate 进入 `Cargo.toml` 时在提交信息写明理由；rfd 若许可不兼容白名单，
立即退回另选（见 §4 备选）。wgpu 的传递依赖许可由 deny 检查统一兜底，缺白名单条目时
补入 `deny.toml`（保持宽松许可一致，§2.8.3）。

## 3. 模块设计

```
roboview-core/
├── io/
│   ├── point_cloud.rs      # 格式分发：扩展名+文件头双校验 → PLY/PCD 解析
│   ├── ply.rs              # PLY：ascii + binary_little_endian
│   └── pcd.rs              # PCD 0.7：ascii + binary_little_endian
│   └── data/               # PointCloudData（positions/colors/包围盒）+ 错误类型（thiserror）
├── displays/point_cloud.rs # 显示类型：数据 → GPU 顶点缓冲句柄；标记包围盒
├── render/                 # Renderer：device/queue、点云管线（WGSL）、上传/绘制
└── scene/                  # Scene（显示实例 + 相机状态）、包围盒与变换工具
```

```
roboview/
├── main.rs                 # eframe App：菜单（打开点云文件）、错误通知、事件分发
└── ui/
    ├── viewport.rs         # egui 视口组件 + egui_wgpu paint callback 渲染场景
    └── camera.rs           # 轨道相机（左键环绕/滚轮缩放/中键平移，目标=包围盒中心）
```

## 4. 关键实现决策

- **对话框**：首选 `rfd`（原生对话框、覆盖三平台、轻依赖）。备选：egui 自行实现的对话框
  （无新依赖，但非原生）。以许可验证与 CI 结果在实施时定案；spec 只要求"原生对话框"。
- **点渲染**：PointList 图元 + 共享着色器（WGSL）；不做大于 1px 的点精灵（非目标，保持最小）。
  坐标/颜色两个顶点缓冲（bytemuck 布局，无每帧分配）。
- **相机**：球坐标轨道（yaw/pitch/distance）+ 目标点锁定包围盒中心；输入为 egui 鼠标事件。
- **解析器**：字节级手写（不引第三方 loader）；测试用**内存构造的字节数组**做样例
  （不依赖磁盘测试文件、确定性、§2.9.3）；A9 指生产代码无硬编码路径，不影响测试内联数据。
- **错误路径**：`PointCloudError`（thiserror）向上传播，app 层转为用户可见的通知；不 panic。
- **分层测试**：core 全部逻辑可在无 GUI 环境单测；egui 交互留给验收步骤手工验证。

## 5. 风险与对策

| 风险 | 对策 |
|---|---|
| wgpu 依赖树过宽，deny 白名单缺条目 | CI deny 反馈后逐条补充（宽松许可）并记录 |
| 某依赖（如 wgpu 传递链）MSRV 高于 1.85 | msrv job 实测；若冲突，评估升级 MSRV（需明确记录）或降级依赖版本 |
| rfd 许可/平台问题 | 按 §4 备选立即切换 |
| 1px 点在大屏幕上观感一般 | 明确记录为非目标；后续功能再做实例化四方图 |

## 6. 产出物

`spec.md`（Approved）+ 本方案（评审后转 Approved）+ `tasks.md`（原子任务清单）。
