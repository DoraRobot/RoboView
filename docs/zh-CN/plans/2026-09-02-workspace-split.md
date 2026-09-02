# Workspace 拆分——采纳 Cargo workspace crate 结构

状态：Approved（已批准）

日期：2026-09-02

相关：CONSTITUTION §2.4.1–2.4.4（已修订，0.2.0）；ADR 003

> 本文件是英文版 [`docs/plans/2026-09-02-workspace-split.md`](../../plans/2026-09-02-workspace-split.md) 的中文镜像；如有冲突以英文版为准。

## 背景

RoboView 致力于分层架构：无 GUI 依赖的核心层（渲染、场景图、数学、IO）与应用层（GUI、平台外壳）
相分离（CONSTITUTION §2.4.1）。§2.4.2 确定了目标 crate（`roboview-core` 库 + `roboview` 可执行），
并要求在职责分离要求它时立刻拆成 workspace。仓库今天没有任何 GUI 栈、几乎没有代码：
此刻拆分只花费一个占位入口的移动，而推迟拆分则意味着未来在两个方向（GPU 渲染 vs GUI 外壳）
全速增长的代码上补边界。

## 决策

现在采纳 Cargo workspace。

- 成员：`roboview-core`（库）与 `roboview`（可执行）。
- workspace 根 `Cargo.toml` 是虚拟清单（无 `[package]`）；`default-members` 为 `roboview`，
  在根目录运行 `cargo run` 即启动应用。
- 成员目录平铺在仓库根目录，以各自 crate 名命名（`roboview/`、`roboview-core/`），
  而非嵌套在 `crates/` 下。
- 依赖方向单向：`roboview` → `roboview-core`；核心 crate 绝不依赖应用层或 GUI crate。
- 核心 crate 从第一天起暴露模块树：`scene/`、`render/`、`io/`、`displays/`。
  模块按功能组织（§2.4.3），并拥有各自的主类型与错误类型。
- 新 crate（如显示类型插件 `roboview-displays-*`）加入 workspace，各自携带自己的 `assets/`（ADR 003）。

## 候选方案

- **推迟拆分，暂时保留单 crate。** 只有在 crate 几乎为空时迁移才便宜；分层架构是项目第一天
  就成立的既定不变量（§2.4.1），拆分只是"何时"而非"是否"。否决：现在做只额外花一次占位移动，
  以后做则要迁移真实代码。
- **嵌套 `crates/` 目录。** 把 crate 集中一处，也是本生态的常见做法。否决：平铺目录路径更短
  （`roboview/src/main.rs` 而非 `crates/roboview/src/main.rs`），ADR 003 的"资产随 crate"
  规则保持字面成立，且嵌套层以后加也不是破坏性变更。
- **现在就拆三个 crate**（`roboview-core` + 独立 schema/IO crate + `roboview`）。
  schema crate 今天没有第二个消费者：数据与协议类型留在 `roboview-core` 内，
  直到出现无头消费者（CLI 转换器、离屏渲染器）。
- **现在用 feature 门控的单 crate**（`#[cfg(feature = "ui")]` 让核心保持无 GUI）。
  否决：门控是对同一边界的手工模仿，日后仍须移除；workspace 才是同一规则的机器级实现。

## 宪法修订

- §2.4.2 改写为已采纳的 workspace；宪法版本 0.1.0 → 0.2.0（§7.1）。
- 后续修订，0.2.1：§5.2 与 §7.1 明确 `CHANGELOG.md` 自首个发布起存在；在此之前，
  修订记录存于承载该修订的方案文档。
