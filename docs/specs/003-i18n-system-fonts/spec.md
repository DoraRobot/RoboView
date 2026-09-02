# i18n-system-fonts — 规格

状态：Approved（已批准）

日期：2026-09-02

修订：2026-09-02（四视角评审后全面修订；负责人裁定 4 项决策已落位正文：D1 自研 fontdb、locale 显式注入、CI 预装 CJK 字体、display-types 功能完成后追溯改名为 002-display-types；2026-09-02 负责人定稿批准：In Review → Approved，§7 已裁定节删除并落位正文）

相关：ADR 004（SDD 工作区）、ADR 006（技术栈）、`docs/specs/002-display-types/`（已实现：`texts.rs` 文案集中，§6.5）、`docs/specs/001-point-cloud-viewport/`（性能协议 A11 基线）

> 跨 workspace 引用全限定（docs/README 约定，本文件从之）：`001-point-cloud-viewport spec.md A9`、
> `docs/specs/002-display-types/spec.md A1–A12`；裸编号仅本 workspace 有效。

## 1. 问题陈述

两个已被实际触发的缺陷：

1. **字形覆盖**：egui 默认字体链（拉丁/emoji/图标子集）不含 `→`（U+2192）——display-types 的
   提示文案已因此出现豆腐块（已用 ASCII 临时规避）；中文等非拉丁文案将系统性遇到同类缺失
   （CJK 缺口是符号的百倍规模：002-display-types spec 的 marker 标签是用户数据，任何 locale 下都可能含中文）。
2. **文案硬编码英文**：`texts.rs` 集中但无 locale 概念与切换机制。

本功能把"文案层 i18n 骨架"与"字体层系统字体"一次落地：**应用文案支持 en/zh-CN
（OS 自动检测 + 运行期切换），字体经系统字体加载（零打包体积，自研 fontdb 胶水），
符号/CJK 缺口由系统字体族补齐**。

## 2. 成功指标（可测试）

| # | 指标 | 判定 |
|---|---|---|
| M1 | 无豆腐块 | 回退链建立后，探测字符串（`→ … X/Y/Z 中文测试`，不含替换字形所在码位）在应用内渲染无 `.notdef`；UT 用 epaint `Fonts::has_glyph(s)`（0.32.3 公开 API、无头可用）逐字断言 + MAN 目视；链上存在能覆盖探测字符的字体族为断言前置 |
| M2 | 双语可用 | en / 中文（zh-CN）两套文案全部切齐：菜单/面板/对话框/错误消息**模板**同步切换（≤1 帧）；机器诊断细节（core `reason` 载荷与 OS 错误文本）保持原语言，不属本指标 |
| M3 | 语言行为 | 启动按 OS 语言自动选择（缺省英文）；运行期菜单切换即时生效（≤1 帧）；切换后对象名/数据类型/顺序不变（数据不翻译） |
| M4 | 文案完整性 | 每条可翻译 key 在两 locale 均有值：zh 缺失 → 英文回退 + 按 (locale, key) 去重警告一次（tracing），不崩溃 |
| M5 | 性能 | 固定测量协议（同 `001-point-cloud-viewport spec.md` A11）：release、参考机（型号/OS/分辨率）记录、计时起点=进程启动至字体链就绪（扫描+FontDefinitions 构造完成），样本 ≥3 取中位，判据 < 300ms；语言切换帧无 >33ms 尖峰（切换后 N 帧窗口采样，暖帧声明除外） |
| M6 | 回归 | display-types 全部验收不受影响（本功能不取代任何 display-types 验收行，见 §6 影响声明）；`001-point-cloud-viewport spec.md` A9 守卫（check_data_paths.sh）仍绿；五门禁绿 |

## 3. 用户故事

- **US1**：系统语言为中文时启动 RoboView → 界面自动全中文。
- **US2**：运行中经语言菜单切到 English → 全部文案即时切换，无需重启（含已打开的错误窗口）。
- **US3**：任意平台渲染含 `→` 与中文的界面文案/用户数据 → 无豆腐块。
- **US4**：某条文案缺翻译 → 显示英文 + 日志警告一次，应用不崩。

## 4. 验收标准（渠道：CI=自动，UT=单测，MAN=手工）

| # | 验收 | 渠道 |
|---|---|---|
| A1 | 回退链包含 ≥1 个系统字体族（非空且键均在 font_data 中，0.32 对悬空键只降级）：UT 断言（fonts 单元）| UT+CI |
| A2 | 探测字符串（`→ … X/Y/Z 中文测试`）经 `Fonts::has_glyph`（Proportional + Monospace 各一次）逐字渲染无 .notdef；CI 三平台 runner **预装** `fonts-noto-cjk`，ubuntu/windows/macos 确定性通过 | UT+CI |
| A3 | OS 语言 = zh（mac/win/linux 至少一台 zh 本机）→ 启动中文；非 zh → 英文（其余平台 En + CI 覆盖，沿用 001 A12 弹性模式） | MAN |
| A4 | 运行期切换：语言菜单选择 → 全部文案 ≤1 帧切换；来回切换 10 次无闪烁/错位；**已打开的错误窗口同步切换**；**zh 稳态布局**：默认 1280×800 与最小 480×360 下全 UI 无截断/遮挡（DragValue/侧栏 230pt/按钮/错误消息完整可读） | MAN |
| A5 | 模拟缺失（测试删 zh 键）→ 英文回退 + 恰好一次警告（按 (locale,key) 去重） | UT |
| A6 | 对象列表/场景数据切换语言后不变 | MAN |
| A7 | 性能按 M5 协议执行并记录：启动字体链就绪 <300ms（中位）；切换帧无 >33ms 尖峰（暖帧声明除外） | MAN |
| A8 | 回归：display-types A1–A12（全量，无取代项——本功能零语义改动，字面英文验收行按 locale 等价语义执行）；`001-point-cloud-viewport spec.md` A9 守卫 + 五门禁 + 三平台矩阵 | CI+MAN |

## 5. 非目标（本功能不做）

- 不做第三种语言（仅 en/zh-CN；语言树骨架预留）；不做 RTL。
- 不打字库：不打包任何字体文件（纯系统字体加载）。
- 不做语言选择持久化（重启按 OS 语言；会话内记忆）。
- 不做排版质量保证（断行/换行/宽度微调）——但 **导致控件不可用/文案被截断不可读的布局问题不属豁免**（A4 zh 稳态验收覆盖）。
- 不做日期/数字/单位本地化格式；不做翻译质量治理；不做字体选择 UI。
- 不修改 core：i18n/字体全部在 app 层（core 零文案层、零字体；既有不变式保持）。
- 不翻机器诊断细节（core `reason`、`std::io::Error` 文本恒定原语言——见 §6 分层）。
- **范围锁定**：任何调整（如改打包字体、新增 locale、引入第三语言）须修订本规格并重新批准（同
  `docs/specs/002-display-types/spec.md` 范围锁定先例）。

## 6. 约束

### 6.1 字体方案（已裁定：自研 fontdb 胶水）

- 依赖：`fontdb 0.23`（MIT、MSRV 1.60；许可/维护已核验，deny 白名单内）+ `sys-locale 0.3`
  （MIT OR Apache-2.0、零依赖、MSRV 1.56）。均只进 `crates/roboview`（app 层，§2.8.1 说明用途、独立提交）。
- **单一 locale 无关回退链**（启动构造一次，进程级缓存，OnceLock）：
  `egui 内建默认（拉丁）→ 系统拉丁/符号候选族 → 平台 CJK 候选族（链尾，无条件附加）`。
  候选族清单（按系统存在性全查全追加）：macOS：Hiragino Sans GB / STHeiti / Apple Symbols /
  PingFang SC（部分系统不存在，作为候选而非假设）；Windows：Microsoft YaHei / SimHei /
  Segoe UI Symbol；Linux：Noto Sans CJK SC / DejaVu Sans。
- **切语言零字体操作**：运行期切换只换文案 getter，不重建 FontDefinitions——A4 ≤1 帧、A7 无
  33ms 尖峰由此平凡成立；EN 界面中的中文数据（marker 标签/文件 stem）由链尾 CJK 覆盖（US3）。
- fontdb 扫描/查询一次性（启动，`RoboViewApp::new` 内完成、首帧前，`tracing` 计时）；
  `FaceInfo.index` 透传 `FontData{index}`（.ttc 集合字体）；只增补不替换内建（链序保持）。
- 扫描失败/无系统字体 → 静默回退内建字体（tracing 一次警告），M1 判定相应降级为"参考记录"。
- **CI 平台政策**：ubuntu runner 预装 `fonts-noto-cjk`（A2 确定性）；A1 恒常断言链非空。
- 已核验的反面参照（实现时规避，不引入）：现成字体加载 crate 无一配对 egui 0.32.3（版本或
  crates.io 许可声明缺口），且存在 MSRV 超 1.85、.ttc 索引丢失、族首插入反转链序等缺陷——
  自研胶水避免全部四点。

### 6.2 Locale 机制（已裁定：显式注入，无全局可变）

- `ui/texts/`：`Locale { En, ZhCn }` + 纯函数 `Locale::from_tag(&str) -> Locale`
  （未知/解析失败 → `En` + tracing 警告；`zh` 前缀（含 `zh-Hant/-TW/-HK`）→ `ZhCn`，一行政策）。
- 状态流：`RoboViewApp.locale: Locale`，**显式传入**全部文案消费点
  （`show_viewport`/`show_objects_panel`/对话框/错误窗口等约 8 处签名 + 调用点机械加参，
  编译器穷尽保证无漏）；**无进程级可变状态**。
- 语言菜单：`File → Language → English / 中文（简体）`；菜单项以各语言**自称**（不随 locale 切换）。

### 6.3 文案层（key 表；不变量恒 const）

- `TextKey` 枚举（约 46 键）+ `resolve(locale, key) -> &'static str`（zh 缺失 → En + 去重 warn once）
  + snake_case 零参 getter 包装（调用点机械改造约 44 层）。
- **不变量集合**（恒为英文 const，不翻译）：`WINDOW_TITLE`（RoboView）、`AXIS_X/Y/Z`
  （002-display-types spec 锁定）、`🗑` 图标、**生成名模板**（`Frame {N}`/`Marker {N}`——生成即
  数据，切换不翻译既有名）；`ViewportState` 零 locale 依赖。
- D4 翻译清单（以现有英文为准逐条中文）：可翻译 const（约 41）+ 错误模板（约 11 句，
  整句作为翻译单元、占位符原位替换）+ 语言菜单（3-4 条）≈ 55-60 项。

### 6.4 错误分层与错误窗口（已裁定）

- 分层：**错误模板 = 可翻译文案**（进 locale 表）；**`reason` 载荷与 `std::io::Error` 文本 =
  机器诊断细节**，英文/OS 原文恒定，不翻译、不入完整性清单（M2 边界）。
- 错误状态改**结构化事件**（`{ file: String, error: LoadError }`），错误窗口每帧按当前
  locale 组装——已打开的错误窗口随切换更新（A4/ M2 架构保障）。
- core 错误文本为英文代码字符串（§1.3 允许、经 app 映射层转用户可见消息 §2.5.2）；"core 零文案"
  精确表述为：core 无 UI 文案层、不感知 locale（其错误文本经 app 映射后可见）。

### 6.5 语言边界与回归

- zh-CN 文案仅存在于运行时译本表（locale/文案 key 的字符串值）中；仓库文档、代码注释、commit、
  键名与标识符不因本功能改变，仍为英文（§1.3）；UI 文案英文为先、zh 为译本（§6.5，缺失回退英文）。
- 回归执行：display-types 各 MAN 验收以 **En locale** 执行（判据按行为、不按文案字面）。

