# 004 — SDD 规格工作区（中文工作文档）

状态：Approved（已批准）

日期：2026-09-02

取代：无

相关：CONSTITUTION §1.9、§4.1.2–4.1.3、§6.1；ADR 001；CONSTITUTION §4.2.4

> 本文件是英文版 [`docs/decisions/004-sdd-specs-workspace.md`](../../decisions/004-sdd-specs-workspace.md)
> 的中文镜像；如有冲突以英文版为准。

## 背景

RoboView 将采纳规格驱动开发（SDD）作为默认实现工作流（方案：`docs/plans/2026-09-02-sdd-workflow.md`）。
SDD 在每个功能专属工作区产出三份文档：规格（WHAT）、方案（HOW）与原子任务清单。
仓库已有 `docs/plans/`，但这些文档是项目级的：治理变更、里程碑、架构方向。
单个功能的规格/方案/任务不属于那里。

SDD 工作文档是为团队编写的，使用中文。这是对 §1.3/§1.5（英文准则、语言树）的刻意例外：
这些是内部工作产物，此处团队工作语言是中文。它们不是对外材料。

SDD 还会产生执行痕迹（`debug/`、prompts、日志），那些是过程记录，绝不进入仓库（§4.2.4）。

## 决策

- **`docs/specs/<feature-id>/`** 是功能级 SDD 工作区，包含 `spec.md`
  （WHAT：问题、成功指标、用户故事、验收标准、非目标、约束）、`plan.md`
  （HOW：架构、模块、接口）、`tasks.md`（可独立验证的原子任务）。
- **中文撰写，无镜像。** 没有英文版，没有 `docs/zh-CN/specs/` 树——
  这是双语规则唯一的例外（§1.9）；§1.7 的 1:1 要求不适用于该树。
- **项目级计划留在 `docs/plans/`**——英文，带 `zh-CN` 镜像，与以往一致。
  两个层级彼此区分，不合并。
- **执行痕迹留在私有区：** prompts、`agent.log`、`debug/` 内容保留在
  私有档案（`.leon/`，git-ignore），绝不进入 `docs/specs/` 或仓库其他位置（§4.2.4）。
- **升级路径：** 功能的设计一旦升级为架构级的公开设计，作为英文文档（带镜像）
  迁往 `docs/design/`，适用时伴随 ADR。

## 规则

1. `docs/specs/<feature-id>/` 精确包含 `spec.md`、`plan.md`、`tasks.md`；
   id 使用 kebab-case（如 `point-cloud-renderer`）。
2. 规格质量门槛：六要素齐全、成功指标可测试、非目标明确、"粒度检验"通过
   （换技术栈实现后规格仍成立）。实现开始前规格须经评审（§6.1）。
3. `tasks.md` 中任务原子且可独立验证；完成状态记录在此，执行痕迹留在
   `.leon/`（见上述规则）。
4. 本工作区豁免于语言树（§1.5、ADR 001）及其完整性检查。

## 影响

- `docs/plans/` 保持项目级角色；`docs/specs/` 是功能级。现有文档不动。
- CONSTITUTION §1.9、§4.1.2、§4.1.3、§6.1 引用本 ADR。
