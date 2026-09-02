# i18n-system-fonts — 方案

状态：Approved（已批准）

日期：2026-09-02

相关：`spec.md`（Approved）；ADR 004、ADR 006；`docs/specs/002-display-types/`（texts.rs 现结构）

修订记录：2026-09-02 起草；经负责人评审通过（In Review → Approved）。

## 1. 概述

本方案确定 HOW：自研 fontdb 胶水（扫描/查询/注入 egui 字体链）、locale 骨架与显式注入、
texts.rs 的 key 表化（不变量恒 const）、错误窗口结构化、语言菜单与完整 zh 翻译表、
CI 预装 CJK 字体与 A1/A2 无头断言、M5/A7 测量协议落地。全部改动在 app 层（§6 约束）；
core 零涉。新增依赖：`fontdb` 0.23、`sys-locale` 0.3（均仅进 `crates/roboview`）。

## 2. 依赖清单（新增，逐个理由，§2.8.1）

| crate | 归属层 | 用途 | 许可（已核验） | MSRV |
|---|---|---|---|---|
| `fontdb` | app | 系统字体扫描/查询（自研胶水的底座，承担全部字体系统工作） | MIT（0.23，crates 内外双确认；依赖 log/slotmap/tinyvec/ttf-parser 全宽松） | 1.60 |
| `sys-locale` | app | OS 语言检测（返回语言标签） | MIT OR Apache-2.0 | 1.56 |

红线：均入 deny 白名单；独立依赖提交（§2.8.4）；提交信息写明用途；不启用任何额外 feature。
**不引入** `egui-system-fonts`/`egui_zhcn_fonts`/`egui-chinese-font` 等（spec §6.1 已记录核验结论）。

## 3. 模块设计

### 3.1 locale（`ui/texts.rs` 内，文件保持单文件、随规模增长）

```text
pub enum Locale { En, ZhCn }
impl Locale { pub fn from_tag(tag: &str) -> Locale;  // 纯函数：zh 前缀(含 zh-Hant/-TW/-HK)→ZhCn；
                                                      // 未知/解析失败→En + tracing warn once
              pub fn name(self) -> &'static str; }   // "English"/"中文（简体）"——菜单自名（不随切换）
```

### 3.2 文案 key 表（`ui/texts.rs`）

```text
pub enum TextKey { MenuFile, MenuOpenPointCloud, …, ErrorWindowTitle, … }   // ~46 键
pub fn resolve(locale: Locale, key: TextKey) -> &'static str;
    // zh 缺失 → En + 按 (locale,key) 去重 warn once（OnceLock<Mutex<HashSet>>）
// 每键一个 snake_case getter：pub fn menu_file(locale: Locale) -> &'static str { resolve(locale, TextKey::MenuFile) }
// 不变量（恒 const、不迁表）：WINDOW_TITLE/AXIS_X/Y/Z/OBJECTS_REMOVE(🗑)
// 命名模板（恒英文、生成即数据）：default_frame_name/marker_name(sequence)
// 错误模板（约 11 句，整句为翻译单元、占位符原位替换）：load_failed(locale, file, &LoadError) -> String
//     —— 变体选句走 locale 表；reason/std::io::Error 载荷原文透传（机器语言，不译）
```

### 3.3 字体加载器（新 `ui/fonts.rs`）

```text
pub fn load_system_fonts() -> egui::FontDefinitions;
// 1) fontdb::Database::load_system_fonts()（OnceLock 缓存，进程一次）
// 2) 候选族清单（逐项 query，存在即取）：
//    macOS: Hiragino Sans GB / STHeiti / Apple Symbols / PingFang SC
//    Windows: Microsoft YaHei / SimHei / Segoe UI Symbol
//    Linux: Noto Sans CJK SC / DejaVu Sans
// 3) FontData::from_owned(bytes) + FontData{ index: face_info.index }（.ttc 透传）
// 4) insert_font/FontDefinitions：内建默认（拉丁/emoji）不动；系统族以
//    FontPriority::Lowest（链尾追加）——只增补、不替换、不插族首
// 5) 空守卫：全系统无字体可注入 → 返回默认 FontDefinitions + tracing warn（M1 降级参考记录）
pub fn probe_has_glyphs(defs: &FontDefinitions, probe: &str) -> Vec<(char, bool)>;  // 测试用
```

### 3.4 app 接线（main.rs / ui/*）

- `RoboViewApp { locale: Locale, error: Option<ErrorEvent>, … }`，
  `ErrorEvent { file: String, error: io::LoadError }`（结构化存储）。
- `RoboViewApp::new`：`let defs = fonts::load_system_fonts(); cc.egui_ctx.set_fonts(...)`；
  `tracing` 计时（开始→set_fonts 完成），供 M5/A7 记录。
- 显式注入：`show_viewport(ctx, state, locale)`、`show_objects_panel(ui, state, locale, requests)`、
  对话框 `show(ctx, locale)`、错误窗口 `error_window(ctx, locale, &error_event)`——签名加
  `locale` 参数（≈8 处）；调用点机械加参（≈44 层，编译器穷尽）。
- 语言菜单：`File → Language →` 两项（`Locale::name()` 自名）；点击置 `app.locale`，
  **帧首应用**（菜单点击只 pending，update 开头落位——消除混帧）。
- `poll_background_load`：失败分支改存 `ErrorEvent`（不再拼 String）。
- ci.yml：ubuntu test job 加一步 `sudo apt-get install -y fonts-noto-cjk`。

## 4. 关键实现决策

| 决策 | 选择 | 理由/候选放弃 |
|---|---|---|
| fonts 注入语义 | `FontPriority::Lowest` 链尾追加、不插族首 | 保持内建拉丁字形优先（度量/等宽性）、只补缺口；族首插入会反转 spec §6.1 链序且抢 emoji/等宽字形 |
| 探测断言 | `epaint::text::Fonts::new(defs, 1.0)` 无头 + `has_glyph/has_glyphs`（0.32.3 公开 API） | 无需 GPU/窗口；FontFamily::Proportional 与 Monospace 各一遍 |
| 探测串 | `→ … X/Y/Z 中文测试` | 不含替换字形码位（◻/?），规避 font.rs 已知 false-negative |
| ttc | `fontdb::FaceInfo.index` 透传 `FontData.index` | 否则 mac/win 集合字体错面（字重/字形偏差） |
| 菜单自名 | `Locale::name()` 恒自名（"English"不随切换变"英文"） | 切换器本身作为标识，字面稳定 |
| zh 简体单表 | `from_tag` 把 `zh*` 全映射 `ZhCn` | 本期无繁体字形承诺（降级记录机制覆盖） |
| 单一文件 | `texts.rs` 单文件承载 Locale/TextKey/表/helper（约 600 行） | 匹配现状单文件风格；子模块拆分收益低（无外部消费者） |

## 5. 实施顺序（P0–P4，回归最小化）

| 阶段 | 内容 | 回归证据 |
|---|---|---|
| P0 | 依赖（fontdb/sys-locale 独立提交）+ `Locale::from_tag` 骨架（UT：zh*/未知/空串） | 五门禁 |
| P1 | texts key 化 + 调用点机械改造（En 单表先行，行为零变化）+ A5 缺失回退 UT（zh 表空表模拟） | display-types A1–A12 全量（En 语义等价） |
| P2 | fonts 加载器 + A1/A2（无头断言）+ ci.yml 预装 + `RoboViewApp::new` 接线与计时 | A2 UT+CI 三平台；M5 记录开跑 |
| P3 | 语言菜单 + 错误窗口结构化 + zh 表全量翻译（~55–60 项） | A3/A4/A6 MAN（含 zh 稳态布局、错误窗切换） |
| P4 | M5/A7 协议记录 + 回归（A8：display-types En 全量 + 001 A9 守卫 + 五门禁）+ M1 目视 | A7/A8 |

## 6. 风险与对策

| 风险 | 对策 |
|---|---|
| 本机/系统缺 CJK 字体（mac 无 PingFang 实测） | 候选清单按"存在性全查"（本机 STHeiti/Hiragino 有）；全缺 → A2 降级 + M1 参考记录（spec 已定） |
| `set_fonts` 时机过晚（首帧后）→ 首帧英文/旧字体闪烁 | `RoboViewApp::new` 内完成（首帧前）；计时记录验证 |
| 切换帧字体 relayout 风险 | 已解耦（零字体操作）；P3 实测 10 连切 |
| texts 改造破坏既有点 | En 单表先行（P1 行为零变化）+ display-types 全量回归；编译器穷尽（枚举 getter） |
| zh 文案过长截断（230pt 侧栏） | A4 稳态布局 MAN（480×360 最小窗口）；措辞按短句校验 |
| 字体字节常驻内存（零打包代价） | 记录；CJK ~20MB 级在桌面工具可接受（§5 非目标已声明） |

## 7. 产出物与流程

`spec.md`（Approved）→ 本方案（Draft → 人评审 → In Review → Approved）→ `tasks.md`
（P0–P4 原子任务，映射 spec A1–A8/M1–M6，标注渠道）→ 实现 → Validate（A2 三平台 + A4/A6/A7 MAN + A8 回归）。
