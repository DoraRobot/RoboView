# UI 功能路线图 —— 阶段 004–009（登记）

状态：Approved（已批准）

日期：2026-09-03

相关：CONSTITUTION §1.9、§4.1；ADR 004、ADR 006；`docs/specs/001-point-cloud-viewport/` … `docs/specs/007-interaction-polish/`

## 范围

登记已完成基础设施阶段（001 point-cloud-viewport、002 display-types、
003 i18n-system-fonts）之后的 UI 功能序列。各阶段 spec 位于
`docs/specs/NNN-name/`；功能细节、评审结果与验收声明以各 spec 为准——
本文档只记录序列与依赖边。

## 序列与依赖

| 阶段 | 功能 | 状态 | 依赖 |
|---|---|---|---|
| 004 | ui-blueprint —— 四区骨架（树/视口/属性/状态栏）、主菜单栏（macOS 原生 muda + 窗口内兜底）、per-object 外观 uniform、视口辅助层（地面网格、方位指示器）、相机数学 | Draft（已审议；D1–D5 已裁定） | 002、003 |
| 005 | picking-selection —— CPU 拾取、三区选中镜像、F/Delete（焦点门控） | Draft（已审议；裁定完成） | 004（选中语义 + 高亮 uniform 通道） |
| 006 | dock-layout —— 可停靠面板、布局持久化（eframe storage）、3 预设、面板注册表 | Draft（已审议；裁定完成） | 004（固定骨架）、ADR 006 |
| 007 | interaction-polish —— 快捷键（双路径）、右键菜单、DragValue 规范、图标（egui-phosphor 0.10.0）、消息中心（取代 003 错误窗口）、HUD 扩展 | Draft（已审议；裁定完成） | 004、005、006 |
| **008** | **object-transform** —— 移动/旋转/缩放 Gizmo 与拖拽变换 | **已登记（2026-09-03）** | **005 选中语义**（选中=树/视口/属性同一主语；变换命令作用于选中对象）；**004 per-object 外观 uniform 通道**（Gizmo 手柄与受影响对象状态同源渲染） |
| **009** | **timeline** —— scrub/回放面板 | **已登记（2026-09-03）** | **006 可停靠/预留面板机制**（面板注册表、多 surface 浮动、布局持久化）以承载时间线面板 |

实施顺序：004 → 005 → 006 → 007 → 008 → 009。

## 远景（未排期）

- 多场景/视图层（v2）
- 命令面板/插件化 UI 扩展（未来考察）

## 说明

- 008/009 于 2026-09-03 由负责人确认：基于对成熟 3D 工具约定的差距分析
  （008 登记于 005 非目标"路线图序号 008"；009 登记于 004 非目标"路线图序号 009"）。
- 各阶段遵循 SDD 流程：spec → plan → tasks；批准前四视角评审；按最小功能点分波并行实现。
