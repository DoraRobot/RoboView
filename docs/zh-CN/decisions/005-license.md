# 005 — 许可：MIT OR Apache-2.0

状态：Approved（已批准）

日期：2026-09-02

取代：无

相关：CONSTITUTION §0、§2.8.3

> 本文件是英文版 [`docs/decisions/005-license.md`](../../decisions/005-license.md)
> 的中文镜像；如有冲突以英文版为准。

## 背景

仓库是公开的，但 README 仍写着"许可待定"。没有许可的公开代码库，任何人都无法确定如何使用它，
贡献者也缺乏再分发的明确性；而下游依赖面（渲染栈、GUI 组件集）以宽松许可为主——
宽松的 MIT/Apache-2.0 依赖与宽松的项目许可能干净组合，而单一的限制性选择
（如 copyleft，或与生态宽松核心不一致的项目许可）会给采用者与本项目自己的
依赖政策（§2.8.3）制造摩擦。

MIT + Apache-2.0 双许可模式是周边生态的通用形态：两种许可都简短、公认、宽松；
组合让采用者按自身约束选择，也让代码库之间保持互操作。

## 候选

| 候选 | 评估 |
|---|---|
| **MIT OR Apache-2.0 双许可** | 首选。生态标准、宽松，与预期依赖集事实兼容；贡献者可按其一授权。 |
| 仅 MIT | 可行，但 Apache-2.0 明确的专利授予在软件重度领域更有价值；双许可是既定形态。 |
| 仅 Apache-2.0 | 可接受；在生态中略不如双许可形态常见。 |
| Copyleft（GPL 系） | 否决：与预期的宽松依赖集冲突，且在无法律需求的情况下收窄采用面。 |

## 决策

- RoboView **采用 MIT OR Apache-2.0 双许可**（SPDX：`MIT OR Apache-2.0`）。
- 许可文本位于仓库根目录：`LICENSE-MIT` 与 `LICENSE-APACHE`；
  README 的许可证一节写明双许可并指向这两个文件。
- 所有 Cargo 清单声明 `license = "MIT OR Apache-2.0"`（通过 `[workspace.package]`），
  发布的 crate 因此携带 SPDX 标识。
- 项目文本的版权归属：*RoboView contributors*（2026 年）。
- 依赖政策（§2.8.3）依此确认：宽松依赖为基线；copyleft 依赖须在提交信息中
  写明理由，预期仅属例外。

## 规则

1. 任何新增依赖必须与 `MIT OR Apache-2.0` 许可兼容（§2.8.3）；不兼容则阻止合并。
2. 许可文本一经批准不得改动；变更许可须走新的 ADR。

## 影响

- README 的许可证一节以双许可替代"待定"。
- CONSTITUTION §0 增加 License 一行；版本 0.3.0 → 0.3.1。
- Cargo 清单声明 SPDX 标识；可执行 crate 标记 `publish = false`，
  直至另行决定。
