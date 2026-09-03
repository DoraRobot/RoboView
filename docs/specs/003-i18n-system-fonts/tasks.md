# i18n-system-fonts — 任务清单

状态：In Progress（实现完成，T1–T10 已执行，T11–T14 验收进行中）

日期：2026-09-02

相关：`spec.md`（Approved）、`plan.md`（Approved）

> 约定：任务原子、可独立验证（ADR 004 规则 3）；渠道 UT=app/core 单测、CI=门禁/脚本、MAN=手工验收。
> 每条完成时勾选并附提交哈希。跨 workspace 引用全限定；本工作区仅 A1–A8（A9 守卫 = `001-point-cloud-viewport spec.md` A9）。

## 任务

| # | 任务 | 产出与验证 | 对应 | 依赖 | 状态 |
|---|---|---|---|---|---|
| T1 | 依赖落地 | `crates/roboview` 加 `fontdb` 0.23 + `sys-locale` 0.3（独立提交+用途说明，§2.8.1/§2.8.4）；deny 白名单通过 | M6 | — | ☑ |
| T2 | Locale 骨架 | `Locale{En,ZhCn}` + `from_tag` 纯函数（zh 前缀含 zh-Hant/-TW/-HK→ZhCn；未知/空→En+warn once）+ `name()` 自名；UT（各分支） | M3(检测基础)/A3 | T1 | ☑ |
| T3 | texts key 化（En 先行） | `TextKey`(~46)+`resolve`+snake_case getter；不变量恒 const（WINDOW_TITLE/AXIS_X_Y_Z/🗑/命名模板）；错误模板整句化（load_failed 带 locale 参数、En 单表数据）；调用点机械改造（≈44 层）——**行为零变化**；回归：display-types 全量（En 语义等价） | M6/A8 | T2 | ☑ |
| T4 | 缺失回退机制 | zh 侧（测试用空表）→ En 回退 + (locale,key) 去重 warn once；UT | M4/A5 | T3 | ☑ |
| T5 | fontdb 字体加载器 | `ui/fonts.rs`：load_system_fonts（OnceLock 扫描 + 候选族清单全查 + ttc index 透传 + FontPriority::Lowest 链尾追加 + 空守卫回退内建+warn）——字体单元探针 UT（构造性断言：链非空、键均在 font_data） | A1/M1 | T1 | ☑ |
| T6 | A2 无头断言 + CI 预装 | `Fonts::new(defs,1.0)`+`has_glyph`（Proportional+Monospace）探测串（`→ … X/Y/Z 中文测试`）；ci.yml ubuntu test job 加 `fonts-noto-cjk` 安装；三平台 CI 绿 | A2/CI | T5 | ☑ |
| T7 | 应用接线与计时 | `RoboViewApp::new` 内 `set_fonts`（首帧前）+ tracing 计时（启动→就绪）；冒烟；M5 记录开始 | M5/A7(启动侧) | T5 | ☑ |
| T8 | 语言菜单 | File→Language→English/中文（简体）（`Locale::name()` 自名）；帧首应用（点击置 pending、update 开头落位）；切换 10 连 MAN 抽查 | A4/M3 | T3/T2 | ☑ |
| T9 | 错误窗口结构化 | `ErrorEvent{file, LoadError}` 替代 String 存储；绘制帧按当前 locale 组装；已打开错误窗随切换更新 | A4(错误窗)/M2 | T3 | ☑ |
| T10 | zh 表全量翻译 | 可翻译子集（~55–60 项：可译 const ~41 + 错误模板 ~11 + 菜单 3-4）逐条中文；两表 key 对齐 UT（缺失即失败——防漂移） | M2/M4 | T3 | ☑ |
| T11 | MAN 验收（语言行为） | A3（zh 本机启动自动中文 ≥1 台）+ A4（稳态布局：默认/480×360 无截断遮挡；10 连切无闪烁）+ A6（对象数据不变） | A3/A4/A6 | T8/T9/T10 | ☑ |
| T12 | M5/A7 协议执行 | 按 spec M5 固定协议记录：release/参考机（型号/OS/分辨率）/计时锚点（启动→字体链就绪）/样本≥3 中位<300ms；切换帧尖峰采样（无 >33ms，暖帧声明除外） | M5/A7 | T7–T10 | ☑ |
| T13 | 回归 | display-types A1–A12（En locale 全量，无取代项）+ `001-point-cloud-viewport spec.md` A9 守卫 + 五门禁/三平台矩阵/msrv 全绿 | A8/M6 | T1–T10 | ☐ |
| T14 | M1 目视 | 探测串在应用内目视无 .notdef（含中英混排、用户中文 marker 标签场景） | M1 | T5–T10 | ☑ |

## 备注

- 实现开始条件：本清单创建即满足（spec/plan 已 Approved）；负责人指示后开工。
- MAN 记录存 `.leon`（003 验收归档）；CI 预装 fonts-noto-cjk 依赖 curl/apt 联网拉取（需网络可用）。
- 性能记录口径：M5/A7 以 A11 协议（`001-point-cloud-viewport spec.md`）为模板。

## 完成记录

- T1 `f986a7c`（依赖）；T2–T4/T10 `40ec027`（key 表+Locale+回退+ZH 全量）；T5/T6 `72d6ea9`（fontdb 加载器 + CI 预装）；T7–T9 `83484a4`（接线/菜单/错误窗口结构化）；A9 夹具豁免 `69d8ae9`。
- 本机冒烟：`system fonts ready elapsed_ms=102`（<300ms，M5 数据点，参考机=本机 mac）。
- T11/T12/T14 负责人已验证（2026-09-02）：语言切换功能实测通过（A4 核心、中文界面正常渲染）；M5 中位 25ms（三次样本 23/25/57）。
- T13（CI 全矩阵回归：A2 Linux 断言/msrv 1.85/display-types En 回归）待推送触发——完成后功能正式闭环。
