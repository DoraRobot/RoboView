# RoboView 宪法（中文版）

**版本：** 0.3.2 · **批准日期：** 2026-09-02 · **修订日期：** 2026-09-02 · **状态：** 规范性文件

**语言：** [English](CONSTITUTION.md) · 中文

本文档是 RoboView 项目具有约束力的开发标准。所有提交、PR、代码改动与文档**必须**遵照执行。
当本文档与其他任何约定冲突时，以本文档为准（安全与法律要求除外）。

> 本文件是英文版 [`CONSTITUTION.md`](CONSTITUTION.md) 的中文镜像。英文版始终是具约束力的文本；
> 中英文冲突时**以英文为准**（参见 §1.7）。

---

## 0. 项目身份

| 项目 | 值 |
|---|---|
| 名称 | RoboView（crate：`roboview`） |
| 愿景 | 用 Rust 构建的跨平台 3D 数据可视化工具，服务机器人工程与 AI 数据领域 |
| 平台 | 跨平台桌面：macOS / Windows / Linux |
| 语言 | 英语（准则语言）+ 中文（次要，镜像文件） |
| 版本管理 | 语义化版本（SemVer） |
| 仓库 | 单一 git 仓库，`main` 分支保持线性历史 |
| 许可 | MIT OR Apache-2.0 双许可（ADR 005） |

---

## 1. 语言政策

1.1 **英语是本项目的准则语言。** RoboView 是国际化项目：各种语言的贡献者与使用者都必须能读懂全部内容。

1.2 简体中文是**次要语言**。它只出现在服务中文贡献者的场景中，且必须作为英文原文的翻译存在。

1.3 **仅限英文、不可协商：**

- 代码标识符、类型、函数名
- 代码注释与 `rustdoc` 文档
- 提交信息（见 §3）
- Issue、PR 及讨论的标题与正文
- `docs/` 下所有技术文档（`docs/specs/` 除外，见 §1.9）
- 构建脚本、CI 配置注释及工具链配置

1.4 **允许出现中文：**

- 镜像文档（`README.zh-CN.md`、`CONSTITUTION.zh-CN.md` 等）
- 中文贡献者之间的非正式讨论

1.5 **双语布局——两个区域、按设计划分**（操作细节见 ADR 001）：

- **根级**（`README.md`、`CONSTITUTION.md`）：镜像文件——`foo.md` 为英文准则版本，
  `foo.zh-CN.md` 为其 1:1 译文，旁边并排。根文件受工具链约束固定在根目录，只能采用此布局。
- **`docs/` 及其他计划多语言化的文档树：** 按语言目录树组织。英文是树顶默认层
  （`docs/plans/...`）；其余每种语言一个目录，镜像同一结构（`docs/zh-CN/plans/...`）。
  `.zh-CN.md` 后缀留给根级文件，`docs/` 内不得使用。

1.6 禁止在单个文件中混排中英文段落。中文文本中的术语、路径与代码片段用反引号包裹、保持原样——
它们是引用，不是翻译。

1.7 中英文内容冲突时，**以英文文本为准**。翻译必须完整，并与英文原文保持同步；
即使翻译滞后，英文文本仍具有优先效力。在 `docs/` 中，语言树允许稀疏——未翻译的文档
简单地不存在——但存在的任何文件必须是完整、最新的 1:1 翻译。

1.8 双语索引文件中的语言切换行应如实列出可用语言，且不含自链接：当前语言以纯文本呈现，
只有其他语言是链接。示例：`**语言：** [English](README.md) · 中文`。

1.9 例外——**`docs/specs/` 是中文工作区**（ADR 004）。功能规格是团队内部工作文档，不是对外材料：
用中文撰写；无需英文版，无需镜像；§1.7 的 1:1 要求不适用于本树。`docs/` 其余内容仍按 §1.3 使用英文。

---

## 2. Rust 开发标准

### 2.1 工具链

- 使用**最新稳定版** Rust 工具链；库采用 **edition 2024**。
- 在 `Cargo.toml` 中声明 `rust-version` 并慎重更新——当前 MSRV 为 1.85+。
- `rustc`、`rustfmt`、`clippy` 为基线工具；每次改动三者必须全部通过。
- 未经 PR 说明理由，不得使用 nightly 专属特性。

### 2.2 代码格式

2.2.1 每次改动运行 `cargo fmt`；CI 以 `cargo fmt --check` 为硬性门禁。

2.2.2 保持 rustfmt 默认配置——除经文档化决策外，不得添加自定义 `rustfmt.toml`。100 列宽、4 空格缩进、尾随逗号、标准导入分组。

2.2.3 禁止提交与本次改动无关的重格式化。

### 2.3 静态检查

2.3.1 CI 必须通过 `cargo clippy --workspace --all-targets -- -D warnings`。

2.3.2 禁止在全项目范围静默 lint。若为误报，在最小作用域添加 `#[allow(...)]`，并附一行原因说明。

2.3.3 优先使用编译器 lint 与 Clippy 默认集合；启用额外 lint 集合属于模块级决策，须在提交信息中说明。

### 2.4 代码组织

2.4.1 分层架构：从一开始就分离**核心层**（渲染、场景图、数学、IO——不依赖 GUI）与**应用层**（GUI、平台外壳）。

2.4.2 Cargo workspace，自 2026-09-02 起采纳（方案见 `docs/plans/2026-09-02-workspace-split.md`）。仓库根目录是虚拟 workspace 清单，成员如下：

| crate | 类型 | 职责 |
|---|---|---|
| `roboview-core` | 库 | 渲染核心、场景图、数学、IO、显示类型 trait。不依赖 GUI。 |
| `roboview` | 可执行 | 桌面应用：GUI 外壳、UI 面板、平台集成。 |

- 成员目录位于仓库根目录的 `crates/` 下（`crates/roboview/`、`crates/roboview-core/`）。
- workspace 根没有 `[package]`；`default-members` 为 `roboview`，在根目录运行 `cargo run` 即启动应用。
- 依赖方向单向：`roboview` 依赖 `roboview-core`；核心 crate 绝不依赖应用层，也绝不引入 GUI 相关 crate。
- 新 crate（如显示类型插件 `roboview-displays-*`）加入 workspace，各自携带自己的 `assets/`（ADR 003）。

2.4.3 模块按功能而非类型组织（如 `scene/`、`render/`、`io/`、`ui/`），每个模块拥有自己的主类型与错误类型。

2.4.4 公共 API 面要小、稳定、经过测试；核心层必须是可脱离 GUI 特性单独编译的库。

### 2.5 错误处理

2.5.1 库代码：使用 `thiserror` 定义类型化错误（`enum` + `#[derive(Error)]`）。

2.5.2 可执行/入口代码：使用 `anyhow` 传播带上下文的错误；GUI 层将库错误转为用户可读消息。

2.5.3 禁止静默吞掉错误。确实有意忽略的错误须附原因注释，或优先用 `?` 加 `.context(...)`。

2.5.4 公共 API 可达的库代码中避免 `unwrap()`/`expect()`。程序不变量类失败应使用带说明信息的 `unreachable!`。若确实无法避免 `unwrap`，须注释说明使其安全的不变量。

### 2.6 不安全代码

2.6.1 仅在必要时编写 `unsafe`——先寻找安全替代方案。

2.6.2 每个 `unsafe` 块上方必须有 `// SAFETY:` 注释，精确说明代码维持了哪些不变量。

2.6.3 启用 `#![deny(unsafe_op_in_unsafe_fn)]`，`unsafe` 块保持最小化；优先用安全封装（新类型）收紧不变量。

### 2.7 命名与风格

2.7.1 遵循 Rust API 指南：函数/变量用 `snake_case`，类型与枚举变体用 `CamelCase`，常量、静态项与导入别名用 `SCREAMING_SNAKE_CASE`。清晰优先于简写；公共 API 使用完整单词（`num_vertices`）而非缩写（`n_verts`）。

2.7.2 模块与文件名：`snake_case.rs`。

2.7.3 禁止 `dbg!()` 与可被发布路径触达的 `todo!()` 占位——未完成事项改为登记在册的 issue。

### 2.8 依赖管理

2.8.1 保持依赖图最小化；新增依赖时必须在提交信息中说明用途。

2.8.2 本项目需提交 `Cargo.lock`（可执行程序与将随附的库内文件）。

2.8.3 依赖必须得到维护、有广泛使用且许可兼容。CI 运行 `cargo audit` 与 `cargo deny check`（许可检查）；任何漏洞都会阻止合并。

2.8.4 依赖更新使用独立提交，不得混入功能提交。

### 2.9 测试

2.9.1 单元测试放同模块内（`#[cfg(test)] mod tests`）；集成测试放 `tests/`；rustdoc 中的公共示例成为可运行的 doctest。

2.9.2 新的非平凡行为必须与测试同一提交——无测试的改动须在提交信息中说明理由。

2.9.3 测试必须确定性：禁止墙钟睡眠、网络访问与端口冲突（设计应提供可注入的时间与接口）。

2.9.4 CI 以 `cargo test --workspace --all-targets` 为门禁。

### 2.10 性能与日志

2.10.1 渲染与帧处理路径是性能关键路径：避免每帧分配，先测量再优化，无数据支撑时不得以清晰性为代价做优化。

2.10.2 诊断信息使用 `tracing`（或 `log`），库代码中禁止 `println!`/`eprintln!`；结构化字段承载上下文；冗长 span 仅在 debug 下启用。

### 2.11 资源（Assets）

2.11.1 资源放在所属 crate 旁：`<crate>/assets/`（ADR 003）。引擎资产（shader、核心数据）随 `roboview-core`；应用资产（图标、字体、语言目录）随 `roboview`；每个插件 crate 自带 `assets/`。

2.11.2 必须始终伴随二进制的资源在构建期嵌入（`include_str!`/`include_bytes!`）；大体积或用户可替换的数据在运行时从磁盘加载。操作细节见 ADR 003。

---

## 3. Git 与提交规范

### 3.1 提交信息格式

3.1.1 遵循 **Conventional Commits 1.0.0**：

```
<type>(<scope>): <subject>
<空行>
<body>
<空行>
<footer>
```

3.1.2 **所有提交信息一律使用英文。** 小写、无中文、无表情符号。

3.1.3 允许的 `type`：

| 类型 | 含义 |
|---|---|
| `feat` | 新功能 |
| `fix` | 缺陷修复 |
| `docs` | 仅文档 |
| `style` | 格式化，无行为变化 |
| `refactor` | 重构，无 bug/功能变化 |
| `perf` | 性能改进 |
| `test` | 仅测试 |
| `build` | 构建系统/依赖 |
| `ci` | CI 配置 |
| `chore` | 杂项维护，不改变正式代码 |
| `revert` | 回退提交 |

3.1.4 `scope`：影响的模块或区域，kebab-case 小写（如 `renderer`、`scene`、`view`、`io`、`ci`、`docs`）。无法归入模块时省略。

3.1.5 `subject`：祈使句现在时（用 `add`、`fix`、`remove`，不用 `added`、`fixes`），首字母小写，结尾无句号，**不超过 72 字符**。

3.1.6 `body`：可选；说明变更的**原因**，每行一句，72 字符折行，完整句子。

3.1.7 `footer`：用于 `BREAKING CHANGE: <描述>`（破坏性变更 MUST 标注）与引用（`Fixes #12`）。

### 3.2 提交纪律

3.2.1 **一次提交只做一件逻辑变更。** 每个提交必须可编译且保持测试通过；不得把无关改动混入同一提交（使用暂存区）。

3.2.2 禁止 WIP 提交、向共享分支推送 `fixup`、出现 `Merge branch`——历史保持线性可读。

3.2.3 及时、频繁、细粒度地提交——但任何提交都不得破坏构建。

3.2.4 禁止提交生成产物（`target/`、日志、截图、本地配置）；`git status` 中不应出现此类文件。

### 3.3 示例

正确：

```text
feat(renderer): add point cloud rendering pipeline

Add GPU mesh handling for point cloud entities, including buffer
management and color strategies. Initial support for RGB colors.

CI: renderer tests cover 16M-point workload smoke cases.
```

```text
fix(view): correct camera projection matrix

The near/far depth range was signed when the projection used an
OpenGL-style depth clip; remap to normalized depth range so the
shapes near the far plane do not z-fight.

BREAKING CHANGE: projection ordering now matches the z-prepass pass.
```

禁止：

```text
add point cloud stuff          # 缺 type，含糊
FIXED camera bug!!!            # 大写、非祈使句、表情符号
feat: implement many things    # subject 笼统、无 scope
```

### 3.4 分支与工作流

3.4.1 分支命名：`<type>/<简短-kebab-描述>`——`feat/point-cloud`、`fix/camera-projection`、`docs/design-proposals`、`refactor/error-model`。

3.4.2 使用从 `main` 拉出的短期特性分支；`main` 始终可构建、可发布。

3.4.3 通过 **squash merge** 合并，标题使用 Conventional Commits 规范；评审/合并前先将分支 rebase 到 `main`。合并前须通过全部 CI。

3.4.4 Issue 与 PR 使用英文书写与讨论。

3.4.5 标签遵循 SemVer：`v0.1.0`、`v1.4.2`——只打在 `main` 顶端提交上。

---

## 4. 文档规范

### 4.1 位置与结构

4.1.1 `README.md` 是项目门面——英文准则版本——完整中文镜像位于 `README.zh-CN.md`（§1.5）。

4.1.2 **所有设计方案与技术提案均由 `docs/` 承载**——不得散落在根目录或聊天记录中。宪法是唯一例外，保留在根目录。`docs/` 只属于工程文档；面向用户的文档站在引入后放仓库根目录 `site/`（ADR 002）。`docs/` 分两个层级：

- `docs/plans/` —— **项目级**计划：治理变更、里程碑、架构方向（英文，带 `zh-CN` 镜像）。
- `docs/specs/<feature-id>/` —— **功能级** SDD 工作区，服务于单个小工作：`spec.md`（WHAT）、`plan.md`（HOW）、`tasks.md`（原子任务），中文撰写、无镜像（§1.9、ADR 004）。

4.1.3 目录约定：

```
docs/
  README.md        # 本目录索引与约定
  plans/           # 项目级计划（治理、里程碑、方向）
  specs/           # 功能级 SDD 工作区（中文，ADR 004）
  design/          # 架构与详细设计文档
  decisions/       # ADR：NNN-title.md，编号，批准后不可变
```

4.1.4 文件名 kebab-case（`point-cloud-rendering.md`）。ADR 用补零编号（`001-layered-architecture.md`）。中文镜像放在 `docs/zh-CN/` 树中、与英文相同的相对路径（`docs/zh-CN/decisions/002-gpu-backend.md`）——`.zh-CN.md` 后缀只适用于根级文件（§1.5）。

### 4.2 内容规则

4.2.1 每份方案/设计文档开头包含元信息——标题、状态（`Draft | In Review | Approved | Superseded | Rejected`）、日期。Approved 即生效；被取代的文档保留在历史中。

4.2.2 按读者一无所知的标准写作：说明问题、候选方案、决策与理由（为何放弃其他方案）。

4.2.3 文档按 §1.3 使用英文；中文镜像按 §1.5 使用中文。设计层面的代码改动未同步到 `docs/` 时不完整。

4.2.4 **文档只描述 RoboView 本身。** 不得出现关于外部项目、工具或平台的来源、灵感或对比声明，
也不得记录内部讨论、设计对话或写作过程。文档承载的是已确认的决策及其理由——绝不记录决策的达成过程。
特别是：项目聊天、提示词与协商过程属于私密内容，绝不进入仓库，存放它们只能使用项目所有者
用于此用途的私有目录（位于 git 忽略范围内）。

---

## 5. 版本与发布

5.1 语义化版本（SemVer）：`MAJOR.MINOR.PATCH`。库 API 尚不稳定时（`0.x`），minor 版本即可能破坏兼容；重要破坏仍须显著标注。

5.2 `CHANGELOG.md` 随首个发布引入，此后维护在根目录，按版本分组、按 Conventional Commits 类型分节。首个发布之前，该文件不必存在。

5.3 发布流程：升版本 → 更新 `CHANGELOG` → 在 `main` 顶端打 `vX.Y.Z` 标签 → CI 产出发布包。仅从 `main` 发布。

---

## 6. 工作流与评审

6.1 **先设计后编码：** 任何非平凡改动先在 `docs/plans/` 提交方案（§4.1.2），走 `Draft → In Review → Approved`；批准后才开始实现（或有文档化的例外）。具体功能按 `docs/specs/<feature-id>/` 的 SDD 工作流执行——规格 → 方案 → 任务 → 实现 → 验证（ADR 004）；触及共享架构的功能仍需独立的项目级方案或 ADR。

6.2 每个改动都是小 PR：标题清晰，描述写明改了什么、为什么，并链接相关方案/ADR。

6.3 合并前至少一人批准；评审意见以后续提交落实（不得强推共享历史）。

6.4 CI 门禁——全部强制：`cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace --all-targets`、`cargo deny check`、`cargo audit`。

6.5 面向用户的产出必须遵守语言政策（§1）——UI 文案以英文为先，国际化（i18n）结构从第一天就位（本项目为国际化项目）。

---

## 7. 执行

7.1 宪法在每次里程碑评审时复审，修订走与普通方案相同的 `docs/plans/` 流程；每次修订都提升版本号，并在 `CHANGELOG.md` 存在后记录到其中。首个发布之前，修订记录在承载该修订的方案文档中。

7.2 评审者应当在评审意见中引用宪法条款指出违规（如 `CONSTITUTION §2.3`）。

7.3 任何规范都不允许无记录的例外——例外须写入 PR 注释，并最终回收到本文档。
