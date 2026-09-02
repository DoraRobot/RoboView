# 003 — 资源文件位置

状态：Approved（已批准）

日期：2026-09-02（2026-09-02 修订）

取代：无

相关：CONSTITUTION.md §2.4.2（workspace、分层架构）、§2.8（依赖）、§2.11（资源）

> 本文件是英文版 [`docs/decisions/003-asset-location.md`](../../decisions/003-asset-location.md)
> 的中文镜像；如有冲突以英文版为准。

## 背景

RoboView 需要随附若干资源：图标与图片、字体、shader 文件、UI 语言包（i18n）、演示/示例数据。
问题在于这些文件放仓库哪里——项目现在已是 Cargo workspace
（`roboview-core` 库 + `roboview` 可执行，§2.4.2），未来还可能加插件。

## 候选

| 候选 | 评估 |
|---|---|
| **`assets/` 跟随所属 crate** | 首选。资源位于 crate 自己的树内，规则在新增 crate 或插件出现时都不变。 |
| 永远的共享根目录 `assets/` | 归属含糊：core 的 shader、app 的图标、插件的资源全挤一个目录。插件必须自带资源，无法共享池。 |
| 资源放进 `src/` | 非代码产物混入源码树；打包与嵌入路径不清；shader、语言包不是 Rust 源码。 |

## 决策

**资源跟随所属 crate：** `<crate>/assets/`。

- 现在（workspace）：引擎资产（shader、核心数据）在 `crates/roboview-core/assets/`；
  应用资产（图标、字体、语言包）在 `crates/roboview/assets/`；每个插件 crate 自带 `assets/`。

命名：用 `assets/`，不用 `resources/`——图形学中 "resource" 已被 GPU 资源语义占用
（如 wgpu），Qt 式 "resources" 命名会混淆运行时语义。

## 规则

1. `assets/` 下按类别分子目录：`icons/`、`fonts/`、`shaders/`、`locales/`、`demo/`——
   出现新类别时再扩。
2. **嵌入策略。** 必须伴随二进制的资源（图标、内置字体、小 shader、语言包）在构建期嵌入
   （`include_str!` / `include_bytes!`，目录规模变大后上 `rust-embed`）。
   大体积或用户可替换的数据（演示/示例数据集）运行时从磁盘加载，绝不嵌入。
3. **单一来源。** 代码引用的资源只有一处；禁止复制资产复用——复制会破坏打包。
4. Rust 示例程序遵循标准 `examples/` crate 惯例；其数据放 `assets/demo/`，不放 `examples/`。
5. 语言包目录命名与仓库其他部分一致，遵循 BCP-47（ADR 001）。
6. 保持入库二进制体积小。若演示数据集必须超过几 MB，应作为刻意决策评估 git-lfs，
   而非静默提交大文件。

## 影响

- 某 crate 的第一个资源实体出现时，在该 crate 下创建 `assets/` 目录（此前不放 `.gitkeep`）。
- 宪法 §2.11（资源）已引用本 ADR（2026-09-02 加入）。
