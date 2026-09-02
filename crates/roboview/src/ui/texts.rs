//! All user-facing copy, centralized in one module so the entire UI surface
//! can be reviewed — and localized — from a single place (display-types spec
//! §6.5: the strings are the i18n boundary, the widgets never inline their
//! own copy).
//!
//! i18n layout (003 spec §6.2/§6.3): the copy is keyed — every translatable
//! string is a [`TextKey`] with an English and a simplified-Chinese value in
//! the `EN`/`ZH` tables below — and resolved per locale by pure
//! functions. There is no global locale: `Locale` flows down from the app
//! into every copy consumer (panel/dialog/viewport signatures), so a switch
//! takes effect on the next frame. A zh value missing from the table falls
//! back to English and warns once per (locale, key) pair (spec M4/A5).
//!
//! The few never-translated invariants stay `const` here: [`WINDOW_TITLE`],
//! [`AXIS_X`]/[`AXIS_Y`]/[`AXIS_Z`], the [`OBJECTS_REMOVE`] icon, and the
//! generated-name templates of [`default_frame_name`]/[`default_marker_name`]
//! (a generated name is data once it exists; switching languages never
//! renames scene objects — spec M3/A6).
//!
//! Error copy follows spec §6.4's layering: the message *templates* are
//! translatable rows of the tables, while the payloads — core `reason`
//! strings and OS-level error text — pass through verbatim in their original
//! language (machine diagnostics are never translated). Panels, dialogs, and
//! error messages must use the getters below instead of formatting their own
//! text.

use std::collections::HashSet;
use std::fmt::Display;
use std::sync::{Mutex, OnceLock};

use roboview_core::displays::DisplayKind;
use roboview_core::io::{LoadError, ObjError, PathError, PointCloudError};

/// Native window title (also the `eframe::run_native` application name).
/// Invariant: never translated.
pub const WINDOW_TITLE: &str = "RoboView";

/// Coordinate axis letters: the per-axis labels painted at a frame's axis
/// tips (display-types spec §7 F3 / A4) and the prefixes of the XYZ drag
/// values. Invariants: never translated (display-types spec locked the
/// letters into its own acceptance).
pub const AXIS_X: &str = "X";
pub const AXIS_Y: &str = "Y";
pub const AXIS_Z: &str = "Z";

/// Icon of the remove button of one object list row. Invariant: an icon,
/// never translated.
pub const OBJECTS_REMOVE: &str = "🗑";

/// Generated display names of UI-added objects; `sequence` is the per-kind
/// add counter, so every name is unique within a session (the file-backed
/// objects are named by their file stem instead). Invariants: generated
/// names are data from the moment of creation and are never translated or
/// renamed by a locale switch (spec M3/A6).
pub fn default_frame_name(sequence: u64) -> String {
    format!("Frame {sequence}")
}

/// Generated display name of a UI-added marker; see [`default_frame_name`].
pub fn default_marker_name(sequence: u64) -> String {
    format!("Marker {sequence}")
}

/// User-facing language of the UI copy (spec §6.2). The enum is the only
/// locale representation; the value is passed down explicitly and there is
/// no process-wide mutable state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Locale {
    /// English — the source language of the tables (En first, zh is a
    /// translation, display-types spec §6.5); also the fallback for any
    /// missing zh value.
    En,
    /// Simplified Chinese (`zh-CN`). One table serves every zh tag —
    /// `zh-Hant`/`zh-TW`/`zh-HK` included — per the single-table policy of
    /// plan §4 (no traditional-glyph promise this feature).
    ZhCn,
}

impl Locale {
    /// Parse an OS/Browser-style language tag (e.g. `sys-locale` output).
    /// Pure function, one-line policy: any tag starting with `zh` maps to
    /// [`Locale::ZhCn`] (so `zh`, `zh-CN`, `zh-Hans`, `zh-Hant`, `zh-TW`,
    /// `zh-HK`, `zh_CN` all land on the simplified table); any tag starting
    /// with `en` maps to [`Locale::En`]; anything else — including the empty
    /// string — defaults to [`Locale::En`] and records a one-time warning
    /// per unrecognized tag (spec §6.2).
    pub fn from_tag(tag: &str) -> Locale {
        let tag = tag.trim().to_ascii_lowercase();
        if tag.starts_with("zh") {
            Locale::ZhCn
        } else if tag.starts_with("en") {
            Locale::En
        } else {
            warn_unknown_tag(&tag);
            Locale::En
        }
    }

    /// The locale's self-name, shown in the language menu. Always the
    /// language's own name — it does not switch with the active locale
    /// ("English" stays "English" in a zh UI, plan §4: the switcher is its
    /// own stable identifier).
    pub fn name(self) -> &'static str {
        match self {
            Locale::En => "English",
            Locale::ZhCn => "中文（简体）",
        }
    }

    /// BCP-47-ish tag of this locale, for logs and tests.
    pub fn as_str(self) -> &'static str {
        match self {
            Locale::En => "en",
            Locale::ZhCn => "zh-CN",
        }
    }
}

impl Display for Locale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One runtime-translatable string of the UI. The variants are the key
/// space; their values live in the `EN` and `ZH` tables. Invariant copy
/// (see the module docs) is not keyed.
///
/// Grouped the way the pre-locale constants they replace were:
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextKey {
    // File menu (main.rs) and the native open-file dialog specs.
    /// Label of the top menu bar's first menu.
    MenuFile,
    /// File menu item opening the point cloud picker (PLY/PCD).
    MenuOpenPointCloud,
    /// File menu item opening the mesh picker (OBJ).
    MenuOpenMesh,
    /// File menu item opening the path picker (CSV/XYZ).
    MenuOpenPath,
    /// Language submenu label (`File → Language`).
    LanguageMenu,
    /// Titles of the native open-file dialogs, one per file family.
    FileDialogTitlePointCloud,
    FileDialogTitleMesh,
    FileDialogTitlePath,
    /// Filter labels of the native open-file dialogs. The dotless extension
    /// lists stay in the caller (main.rs, next to `OpenKind`): the
    /// repository check script `scripts/check_data_paths.sh` (display-types
    /// spec A12) treats a quoted `.<ext>` in production code as a hardcoded
    /// data path.
    FileDialogFilterPointCloud,
    FileDialogFilterMesh,
    FileDialogFilterPath,
    // Viewport (ui/viewport.rs).
    /// Hint shown in the center of the viewport while the scene holds no
    /// object.
    ViewportEmptyHint,
    /// Shown in the viewport while a file loads in the background.
    ViewportLoading,
    // Objects panel (ui/objects_panel.rs).
    /// Heading of the fixed objects panel (the left sidebar list).
    ObjectsPanelTitle,
    /// Fit button: reframes the camera to the union of the visible objects
    /// (display-types spec §6).
    ObjectsFit,
    /// Tooltip of the Fit button (enabled state).
    ObjectsFitTooltip,
    /// Tooltip of the disabled Fit button: nothing measurable is visible
    /// (frames and markers never participate in the bounds union).
    ObjectsFitTooltipDisabled,
    /// Objects panel entry opening the Add frame dialog (display-types spec §7 F3).
    ObjectsAddFrame,
    /// Objects panel entry opening the Add marker dialog (display-types spec §7 F4).
    ObjectsAddMarker,
    /// Hint of an empty objects list.
    ObjectsEmptyHint,
    /// Tooltip of the remove button of one object row.
    ObjectsRemoveTooltip,
    /// Kind column labels, one per display kind (display-types spec §4 type column). The
    /// core's `DisplayKind::as_str` is a handle-ledger key, not UI text;
    /// this mapping is the only place a kind becomes copy.
    KindPointCloud,
    KindMesh,
    KindPath,
    KindFrame,
    KindMarker,
    // Add dialogs (ui/objects_panel.rs).
    /// Title of the non-modal Add frame window.
    AddFrameWindowTitle,
    /// Title of the non-modal Add marker window.
    AddMarkerWindowTitle,
    /// Confirm button of both add-object windows.
    AddObjectButton,
    /// Label of the frame dialog's origin row (three XYZ drag values).
    AddFrameOrigin,
    /// Label of the frame dialog's axis-length row.
    AddFrameLength,
    /// Marker shape radio choices of the Add marker window (display-types spec §7 F4).
    MarkerShapeText,
    MarkerShapeArrow,
    /// Label of the text marker's anchor row (three XYZ drag values).
    MarkerAnchor,
    /// Label of the text marker's label-text field.
    MarkerText,
    /// Placeholder of the label-text field (the text itself is user data).
    MarkerTextHint,
    /// Labels of the arrow marker's endpoint rows.
    MarkerStart,
    MarkerEnd,
    // Error window (main.rs) and background loader.
    /// Title of the non-modal error notification window.
    ErrorWindowTitle,
    /// Notification when the loader thread ended without reporting an
    /// outcome (defensive; the loader always sends its result before
    /// exiting).
    LoaderAborted,
    // Error message templates (spec §6.4: the *template* is translatable
    // copy; the payloads it interpolates — core `reason` strings, line
    // numbers, OS error text — pass through verbatim). Placeholders use
    // `{name}` tokens that [`interpolate`] substitutes at runtime.
    /// Load-failure window body: `{file}` is the chosen file's name,
    /// `{detail}` the per-variant detail line below.
    LoadFailed,
    /// Loader spawn failure: `{error}` is the OS error text.
    LoaderStartFailed,
    /// Shared unsupported-extension leaf: `{extension}`.
    UnsupportedFormat,
    /// Malformed point cloud: `{reason}`.
    PointCloudMalformed,
    /// Malformed OBJ line: `{line}` (1-based) and `{reason}`.
    ObjMalformed,
    /// OBJ over the supported load limits: `{reason}`.
    ObjLimit,
    /// Malformed path-file line: `{line}` (1-based) and `{reason}`.
    PathMalformed,
    /// Path file with too few points: `{points}`.
    PathTooFewPoints,
    /// Path file over the supported load limits: `{reason}`.
    PathLimit,
    /// OS-level file read failure leaf: `{error}`.
    ReadError,
}

impl TextKey {
    /// Every key, in declaration order — the canonical key space the
    /// coverage tests pin both tables to (a key added here must land in the
    /// `EN`/`ZH` tables and in a getter). Test-only enumeration: production
    /// code never iterates the key space.
    #[cfg(test)]
    pub const ALL: &'static [TextKey] = &[
        TextKey::MenuFile,
        TextKey::MenuOpenPointCloud,
        TextKey::MenuOpenMesh,
        TextKey::MenuOpenPath,
        TextKey::LanguageMenu,
        TextKey::FileDialogTitlePointCloud,
        TextKey::FileDialogTitleMesh,
        TextKey::FileDialogTitlePath,
        TextKey::FileDialogFilterPointCloud,
        TextKey::FileDialogFilterMesh,
        TextKey::FileDialogFilterPath,
        TextKey::ViewportEmptyHint,
        TextKey::ViewportLoading,
        TextKey::ObjectsPanelTitle,
        TextKey::ObjectsFit,
        TextKey::ObjectsFitTooltip,
        TextKey::ObjectsFitTooltipDisabled,
        TextKey::ObjectsAddFrame,
        TextKey::ObjectsAddMarker,
        TextKey::ObjectsEmptyHint,
        TextKey::ObjectsRemoveTooltip,
        TextKey::KindPointCloud,
        TextKey::KindMesh,
        TextKey::KindPath,
        TextKey::KindFrame,
        TextKey::KindMarker,
        TextKey::AddFrameWindowTitle,
        TextKey::AddMarkerWindowTitle,
        TextKey::AddObjectButton,
        TextKey::AddFrameOrigin,
        TextKey::AddFrameLength,
        TextKey::MarkerShapeText,
        TextKey::MarkerShapeArrow,
        TextKey::MarkerAnchor,
        TextKey::MarkerText,
        TextKey::MarkerTextHint,
        TextKey::MarkerStart,
        TextKey::MarkerEnd,
        TextKey::ErrorWindowTitle,
        TextKey::LoaderAborted,
        TextKey::LoadFailed,
        TextKey::LoaderStartFailed,
        TextKey::UnsupportedFormat,
        TextKey::PointCloudMalformed,
        TextKey::ObjMalformed,
        TextKey::ObjLimit,
        TextKey::PathMalformed,
        TextKey::PathTooFewPoints,
        TextKey::PathLimit,
        TextKey::ReadError,
    ];
}

/// English copy — the source language (display-types spec §6.5: English
/// first, zh is the translation; a missing zh row falls back here). Must
/// hold every key of `TextKey::ALL`; the coverage test pins the table to
/// the key space so a new key can never go missing silently.
static EN: &[(TextKey, &str)] = &[
    // File menu and native dialog specs.
    (TextKey::MenuFile, "File"),
    (TextKey::MenuOpenPointCloud, "Open point cloud…"),
    (TextKey::MenuOpenMesh, "Open mesh (OBJ)…"),
    (TextKey::MenuOpenPath, "Open path (CSV/XYZ)…"),
    (TextKey::LanguageMenu, "Language"),
    (TextKey::FileDialogTitlePointCloud, "Open point cloud file"),
    (TextKey::FileDialogTitleMesh, "Open mesh file"),
    (TextKey::FileDialogTitlePath, "Open path file"),
    (TextKey::FileDialogFilterPointCloud, "Point cloud"),
    (TextKey::FileDialogFilterMesh, "Mesh (OBJ)"),
    (TextKey::FileDialogFilterPath, "Path (CSV/XYZ)"),
    // Viewport.
    (
        TextKey::ViewportEmptyHint,
        "The scene is empty. Open a file, or add a frame or marker to begin.",
    ),
    (TextKey::ViewportLoading, "Loading…"),
    // Objects panel.
    (TextKey::ObjectsPanelTitle, "Objects"),
    (TextKey::ObjectsFit, "Fit"),
    (
        TextKey::ObjectsFitTooltip,
        "Frame the camera to the visible objects",
    ),
    (
        TextKey::ObjectsFitTooltipDisabled,
        "Nothing measurable is visible yet — open a file first",
    ),
    (TextKey::ObjectsAddFrame, "Add frame…"),
    (TextKey::ObjectsAddMarker, "Add marker…"),
    (
        TextKey::ObjectsEmptyHint,
        "No objects yet. Open a file, or add a frame or marker above.",
    ),
    (
        TextKey::ObjectsRemoveTooltip,
        "Remove this object from the scene",
    ),
    // Kind labels.
    (TextKey::KindPointCloud, "Point cloud"),
    (TextKey::KindMesh, "Mesh"),
    (TextKey::KindPath, "Path"),
    (TextKey::KindFrame, "Frame"),
    (TextKey::KindMarker, "Marker"),
    // Add dialogs.
    (TextKey::AddFrameWindowTitle, "Add frame"),
    (TextKey::AddMarkerWindowTitle, "Add marker"),
    (TextKey::AddObjectButton, "Add"),
    (TextKey::AddFrameOrigin, "Origin"),
    (TextKey::AddFrameLength, "Axis length"),
    (TextKey::MarkerShapeText, "Text label"),
    (TextKey::MarkerShapeArrow, "Arrow"),
    (TextKey::MarkerAnchor, "Anchor"),
    (TextKey::MarkerText, "Text"),
    (TextKey::MarkerTextHint, "Label text"),
    (TextKey::MarkerStart, "Start"),
    (TextKey::MarkerEnd, "End"),
    // Error window and background loader.
    (TextKey::ErrorWindowTitle, "Error"),
    (
        TextKey::LoaderAborted,
        "The background loader ended unexpectedly. The object was not added.",
    ),
    // Error templates ({name} tokens are interpolated at runtime).
    (TextKey::LoadFailed, "Could not open \"{file}\":\n{detail}"),
    (
        TextKey::LoaderStartFailed,
        "Could not start the background loader: {error}",
    ),
    (
        TextKey::UnsupportedFormat,
        "Unsupported file format: \"{extension}\"",
    ),
    (
        TextKey::PointCloudMalformed,
        "Malformed point cloud: {reason}",
    ),
    (
        TextKey::ObjMalformed,
        "Malformed OBJ file at line {line}: {reason}",
    ),
    (
        TextKey::ObjLimit,
        "The OBJ file exceeds the supported load limits: {reason}",
    ),
    (
        TextKey::PathMalformed,
        "Malformed path file at line {line}: {reason}",
    ),
    (
        TextKey::PathTooFewPoints,
        "Path file has too few points ({points}): a polyline needs at least 2 points",
    ),
    (
        TextKey::PathLimit,
        "The path file exceeds the supported load limits: {reason}",
    ),
    (TextKey::ReadError, "Could not read the file: {error}"),
];

/// Simplified-Chinese copy (`zh-CN`), the translation table. Kept key-aligned
/// with `EN`: the coverage test asserts both tables hold exactly
/// `TextKey::ALL`, so a zh row can never be forgotten (漏译) or orphaned
/// (多键) without failing. zh copy exists only here, as runtime string
/// values (spec §6.5); a missing row resolves to the English value with a
/// one-time warning instead of a crash (spec M4).
static ZH: &[(TextKey, &str)] = &[
    // File menu and native dialog specs.
    (TextKey::MenuFile, "文件"),
    (TextKey::MenuOpenPointCloud, "打开点云…"),
    (TextKey::MenuOpenMesh, "打开网格（OBJ）…"),
    (TextKey::MenuOpenPath, "打开路径（CSV/XYZ）…"),
    (TextKey::LanguageMenu, "语言"),
    (TextKey::FileDialogTitlePointCloud, "打开点云文件"),
    (TextKey::FileDialogTitleMesh, "打开网格文件"),
    (TextKey::FileDialogTitlePath, "打开路径文件"),
    (TextKey::FileDialogFilterPointCloud, "点云"),
    (TextKey::FileDialogFilterMesh, "网格（OBJ）"),
    (TextKey::FileDialogFilterPath, "路径（CSV/XYZ）"),
    // Viewport.
    (
        TextKey::ViewportEmptyHint,
        "场景为空。打开文件，或添加坐标架或标记以开始。",
    ),
    (TextKey::ViewportLoading, "正在加载…"),
    // Objects panel.
    (TextKey::ObjectsPanelTitle, "对象"),
    (TextKey::ObjectsFit, "适配"),
    (TextKey::ObjectsFitTooltip, "将相机对准可见对象"),
    (
        TextKey::ObjectsFitTooltipDisabled,
        "暂无可测量的可见对象——请先打开文件",
    ),
    (TextKey::ObjectsAddFrame, "添加坐标架…"),
    (TextKey::ObjectsAddMarker, "添加标记…"),
    (
        TextKey::ObjectsEmptyHint,
        "暂无对象。打开文件，或在上方添加坐标架或标记。",
    ),
    (TextKey::ObjectsRemoveTooltip, "从场景中移除该对象"),
    // Kind labels.
    (TextKey::KindPointCloud, "点云"),
    (TextKey::KindMesh, "网格"),
    (TextKey::KindPath, "路径"),
    (TextKey::KindFrame, "坐标架"),
    (TextKey::KindMarker, "标记"),
    // Add dialogs.
    (TextKey::AddFrameWindowTitle, "添加坐标架"),
    (TextKey::AddMarkerWindowTitle, "添加标记"),
    (TextKey::AddObjectButton, "添加"),
    (TextKey::AddFrameOrigin, "原点"),
    (TextKey::AddFrameLength, "轴长度"),
    (TextKey::MarkerShapeText, "文本标签"),
    (TextKey::MarkerShapeArrow, "箭头"),
    (TextKey::MarkerAnchor, "锚点"),
    (TextKey::MarkerText, "文本"),
    (TextKey::MarkerTextHint, "标签文本"),
    (TextKey::MarkerStart, "起点"),
    (TextKey::MarkerEnd, "终点"),
    // Error window and background loader.
    (TextKey::ErrorWindowTitle, "错误"),
    (TextKey::LoaderAborted, "后台加载器意外结束，对象未添加。"),
    // Error templates.
    (TextKey::LoadFailed, "无法打开“{file}”：\n{detail}"),
    (TextKey::LoaderStartFailed, "无法启动后台加载器：{error}"),
    (
        TextKey::UnsupportedFormat,
        "不支持的文件格式：“{extension}”",
    ),
    (TextKey::PointCloudMalformed, "点云格式错误：{reason}"),
    (
        TextKey::ObjMalformed,
        "OBJ 文件第 {line} 行格式错误：{reason}",
    ),
    (TextKey::ObjLimit, "OBJ 文件超出支持的加载上限：{reason}"),
    (
        TextKey::PathMalformed,
        "路径文件第 {line} 行格式错误：{reason}",
    ),
    (
        TextKey::PathTooFewPoints,
        "路径文件点数过少（{points}）：折线至少需要 2 个点",
    ),
    (TextKey::PathLimit, "路径文件超出支持的加载上限：{reason}"),
    (TextKey::ReadError, "无法读取文件：{error}"),
];

/// Resolve `key` to the copy of `locale` (spec §6.3). A zh value missing
/// from the table falls back to the English one and warns once per
/// (locale, key) pair (spec M4); resolution never panics.
pub fn resolve(locale: Locale, key: TextKey) -> &'static str {
    resolve_from(ZH, locale, key)
}

/// Resolution core over an injected zh table: `zh` replaces the real table
/// so tests can simulate a missing-zh-key gap (A5) with an empty slice; the
/// English side is always the real `EN` table.
fn resolve_from(zh: &[(TextKey, &'static str)], locale: Locale, key: TextKey) -> &'static str {
    match locale {
        Locale::En => en_copy(key),
        Locale::ZhCn => match lookup(zh, key) {
            Some(text) => text,
            None => {
                warn_missing_zh(key);
                en_copy(key)
            }
        },
    }
}

/// Row lookup of one key in `tables`; pure and order-independent.
fn lookup(tables: &[(TextKey, &'static str)], key: TextKey) -> Option<&'static str> {
    tables
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, text)| *text)
}

/// English value of `key`. Every key must be in `EN` — the coverage test
/// pins the table to `TextKey::ALL`, so the panic is a developer error
/// that tests catch, not a runtime failure.
fn en_copy(key: TextKey) -> &'static str {
    lookup(EN, key).expect("every text key must have an English value")
}

/// (locale, key) pairs whose missing zh value was already reported.
fn missing_zh_warned() -> &'static Mutex<HashSet<(Locale, TextKey)>> {
    static WARNED: OnceLock<Mutex<HashSet<(Locale, TextKey)>>> = OnceLock::new();
    WARNED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Unrecognized language tags already reported by [`Locale::from_tag`].
fn unknown_tags_warned() -> &'static Mutex<HashSet<String>> {
    static WARNED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    WARNED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Record a zh gap for `key` and warn on its first occurrence only (spec
/// M4's dedupe: exactly one warning per (locale, key) pair).
fn warn_missing_zh(key: TextKey) {
    let mut warned = missing_zh_warned()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if warned.insert((Locale::ZhCn, key)) {
        tracing::warn!(
            locale = %Locale::ZhCn,
            ?key,
            "zh-CN copy is missing for {key:?}; showing English"
        );
        #[cfg(test)]
        warn_state::count_missing_zh();
    }
}

/// Warn once per unrecognized tag value (first sighting only, so a single
/// odd OS tag cannot spam the log).
fn warn_unknown_tag(tag: &str) {
    let mut warned = unknown_tags_warned()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if warned.insert(tag.to_owned()) {
        tracing::warn!(tag, "unrecognized locale tag; defaulting to English");
        #[cfg(test)]
        warn_state::count_unknown_tag();
    }
}

/// Substitute the `{name}` tokens of a runtime copy template. Templates
/// live in the tables (each whole sentence is one translation unit with its
/// placeholders in place), so they cannot be handed to `format!` — which
/// requires literal format strings — and are filled here instead. Payload
/// text is appended verbatim and never rescanned, so a payload containing
/// `{…}` (or even a token name) cannot corrupt the message. Unknown tokens
/// pass through untouched.
fn interpolate(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let Some(relative_close) = rest[open..].find('}') else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..open]);
        let name = &rest[open + 1..open + relative_close];
        match args.iter().find(|(key, _)| *key == name) {
            Some((_, value)) => out.push_str(value),
            None => out.push_str(&rest[open..=open + relative_close]),
        }
        rest = &rest[open + relative_close + 1..];
    }
    out.push_str(rest);
    out
}

/// Label of the top menu bar's first menu.
pub fn menu_file(locale: Locale) -> &'static str {
    resolve(locale, TextKey::MenuFile)
}

/// File menu item opening the point cloud picker (PLY/PCD).
pub fn menu_open_point_cloud(locale: Locale) -> &'static str {
    resolve(locale, TextKey::MenuOpenPointCloud)
}

/// File menu item opening the mesh picker (OBJ).
pub fn menu_open_mesh(locale: Locale) -> &'static str {
    resolve(locale, TextKey::MenuOpenMesh)
}

/// File menu item opening the path picker (CSV/XYZ).
pub fn menu_open_path(locale: Locale) -> &'static str {
    resolve(locale, TextKey::MenuOpenPath)
}

/// Label of the `File → Language` submenu.
pub fn language_menu(locale: Locale) -> &'static str {
    resolve(locale, TextKey::LanguageMenu)
}

/// Titles of the native open-file dialogs, one per file family.
pub fn file_dialog_title_point_cloud(locale: Locale) -> &'static str {
    resolve(locale, TextKey::FileDialogTitlePointCloud)
}
pub fn file_dialog_title_mesh(locale: Locale) -> &'static str {
    resolve(locale, TextKey::FileDialogTitleMesh)
}
pub fn file_dialog_title_path(locale: Locale) -> &'static str {
    resolve(locale, TextKey::FileDialogTitlePath)
}

/// Filter labels of the native open-file dialogs. The dotless extension
/// lists stay in the caller (main.rs, next to `OpenKind`) on purpose: the
/// repository check script `scripts/check_data_paths.sh` (display-types
/// spec A12) treats a quoted `.<ext>` in production code as a hardcoded
/// data path.
pub fn file_dialog_filter_point_cloud(locale: Locale) -> &'static str {
    resolve(locale, TextKey::FileDialogFilterPointCloud)
}
pub fn file_dialog_filter_mesh(locale: Locale) -> &'static str {
    resolve(locale, TextKey::FileDialogFilterMesh)
}
pub fn file_dialog_filter_path(locale: Locale) -> &'static str {
    resolve(locale, TextKey::FileDialogFilterPath)
}

/// Hint shown in the center of the viewport while the scene holds no object.
pub fn viewport_empty_hint(locale: Locale) -> &'static str {
    resolve(locale, TextKey::ViewportEmptyHint)
}

/// Shown in the viewport while a file loads in the background.
pub fn viewport_loading(locale: Locale) -> &'static str {
    resolve(locale, TextKey::ViewportLoading)
}

/// Heading of the fixed objects panel (the left sidebar list).
pub fn objects_panel_title(locale: Locale) -> &'static str {
    resolve(locale, TextKey::ObjectsPanelTitle)
}

/// Fit button of the objects panel: reframes the camera to the union of the
/// visible objects (display-types spec §6).
pub fn objects_fit(locale: Locale) -> &'static str {
    resolve(locale, TextKey::ObjectsFit)
}

/// Tooltip of the Fit button (enabled state).
pub fn objects_fit_tooltip(locale: Locale) -> &'static str {
    resolve(locale, TextKey::ObjectsFitTooltip)
}

/// Tooltip of the disabled Fit button: nothing measurable is visible
/// (frames and markers never participate in the bounds union).
pub fn objects_fit_tooltip_disabled(locale: Locale) -> &'static str {
    resolve(locale, TextKey::ObjectsFitTooltipDisabled)
}

/// Objects panel entry opening the Add frame dialog (display-types spec §7 F3, A4).
pub fn objects_add_frame(locale: Locale) -> &'static str {
    resolve(locale, TextKey::ObjectsAddFrame)
}

/// Objects panel entry opening the Add marker dialog (display-types spec §7 F4, A5).
pub fn objects_add_marker(locale: Locale) -> &'static str {
    resolve(locale, TextKey::ObjectsAddMarker)
}

/// Hint of an empty objects list (the scene holds no object yet).
pub fn objects_empty_hint(locale: Locale) -> &'static str {
    resolve(locale, TextKey::ObjectsEmptyHint)
}

/// Tooltip of the remove button.
pub fn objects_remove_tooltip(locale: Locale) -> &'static str {
    resolve(locale, TextKey::ObjectsRemoveTooltip)
}

/// The object-list label of `kind`. The core's own `DisplayKind::as_str` is
/// a handle-ledger key, not UI text; this mapping is the only place a kind
/// becomes copy.
pub fn object_kind_label(locale: Locale, kind: DisplayKind) -> &'static str {
    let key = match kind {
        DisplayKind::PointCloud => TextKey::KindPointCloud,
        DisplayKind::Mesh => TextKey::KindMesh,
        DisplayKind::Path => TextKey::KindPath,
        DisplayKind::Frame => TextKey::KindFrame,
        DisplayKind::Marker => TextKey::KindMarker,
    };
    resolve(locale, key)
}

/// Title of the non-modal Add frame window.
pub fn add_frame_window_title(locale: Locale) -> &'static str {
    resolve(locale, TextKey::AddFrameWindowTitle)
}

/// Title of the non-modal Add marker window.
pub fn add_marker_window_title(locale: Locale) -> &'static str {
    resolve(locale, TextKey::AddMarkerWindowTitle)
}

/// Confirm button of both add-object windows.
pub fn add_object_button(locale: Locale) -> &'static str {
    resolve(locale, TextKey::AddObjectButton)
}

/// Label of the frame dialog's origin row (three XYZ drag values).
pub fn add_frame_origin(locale: Locale) -> &'static str {
    resolve(locale, TextKey::AddFrameOrigin)
}

/// Label of the frame dialog's axis-length row.
pub fn add_frame_length(locale: Locale) -> &'static str {
    resolve(locale, TextKey::AddFrameLength)
}

/// Marker shape radio choices of the Add marker window (display-types spec §7 F4).
pub fn marker_shape_text(locale: Locale) -> &'static str {
    resolve(locale, TextKey::MarkerShapeText)
}
pub fn marker_shape_arrow(locale: Locale) -> &'static str {
    resolve(locale, TextKey::MarkerShapeArrow)
}

/// Label of the text marker's anchor row (three XYZ drag values).
pub fn marker_anchor(locale: Locale) -> &'static str {
    resolve(locale, TextKey::MarkerAnchor)
}

/// Label of the text marker's label-text field.
pub fn marker_text(locale: Locale) -> &'static str {
    resolve(locale, TextKey::MarkerText)
}

/// Placeholder of the label-text field (the text itself is user data).
pub fn marker_text_hint(locale: Locale) -> &'static str {
    resolve(locale, TextKey::MarkerTextHint)
}

/// Labels of the arrow marker's endpoint rows.
pub fn marker_start(locale: Locale) -> &'static str {
    resolve(locale, TextKey::MarkerStart)
}
pub fn marker_end(locale: Locale) -> &'static str {
    resolve(locale, TextKey::MarkerEnd)
}

/// Title of the non-modal error notification window.
pub fn error_window_title(locale: Locale) -> &'static str {
    resolve(locale, TextKey::ErrorWindowTitle)
}

/// Notification when the loader thread ended without reporting an outcome
/// (defensive: the loader always sends its result before exiting).
pub fn loader_aborted(locale: Locale) -> &'static str {
    resolve(locale, TextKey::LoaderAborted)
}

/// Notification when a chosen file cannot be loaded. Spec A10 keeps every
/// previously loaded object on screen — the scene is untouched on failure,
/// this message is the only reaction. The detail line is a per-variant
/// mapping of the typed `io::LoadError` tree (spec §6.4): the templates are
/// locale copy, the `reason`/OS payloads pass through in their original
/// language.
pub fn load_failed(locale: Locale, file: &str, error: &LoadError) -> String {
    let detail = describe_load_error(locale, error);
    interpolate(
        resolve(locale, TextKey::LoadFailed),
        &[("file", file), ("detail", detail.as_str())],
    )
}

/// One readable line per `io::LoadError` variant family.
fn describe_load_error(locale: Locale, error: &LoadError) -> String {
    match error {
        LoadError::UnsupportedFormat { extension } => unsupported_format(locale, extension),
        LoadError::PointCloud(error) => describe_point_cloud_error(locale, error),
        LoadError::Obj(error) => describe_obj_error(locale, error),
        LoadError::Path(error) => describe_path_error(locale, error),
    }
}

fn describe_point_cloud_error(locale: Locale, error: &PointCloudError) -> String {
    match error {
        PointCloudError::UnsupportedFormat { extension } => unsupported_format(locale, extension),
        PointCloudError::Malformed { reason } => interpolate(
            resolve(locale, TextKey::PointCloudMalformed),
            &[("reason", reason)],
        ),
        PointCloudError::Io(error) => read_error(locale, error),
    }
}

fn describe_obj_error(locale: Locale, error: &ObjError) -> String {
    match error {
        ObjError::Malformed { line, reason } => {
            let line = line.to_string();
            interpolate(
                resolve(locale, TextKey::ObjMalformed),
                &[("line", &line), ("reason", reason)],
            )
        }
        ObjError::Limit { reason } => {
            interpolate(resolve(locale, TextKey::ObjLimit), &[("reason", reason)])
        }
        ObjError::Io(error) => read_error(locale, error),
    }
}

fn describe_path_error(locale: Locale, error: &PathError) -> String {
    match error {
        PathError::Malformed { line, reason } => {
            let line = line.to_string();
            interpolate(
                resolve(locale, TextKey::PathMalformed),
                &[("line", &line), ("reason", reason)],
            )
        }
        PathError::TooFewPoints { points } => {
            let points = points.to_string();
            interpolate(
                resolve(locale, TextKey::PathTooFewPoints),
                &[("points", &points)],
            )
        }
        PathError::Limit { reason } => {
            interpolate(resolve(locale, TextKey::PathLimit), &[("reason", reason)])
        }
        PathError::Io(error) => read_error(locale, error),
    }
}

/// Shared copy of the three formats' unsupported-extension leaf.
fn unsupported_format(locale: Locale, extension: &str) -> String {
    interpolate(
        resolve(locale, TextKey::UnsupportedFormat),
        &[("extension", extension)],
    )
}

/// Shared copy of the leaf that reports an OS-level file error.
fn read_error(locale: Locale, error: &std::io::Error) -> String {
    let error = error.to_string();
    interpolate(resolve(locale, TextKey::ReadError), &[("error", &error)])
}

/// Notification when the background loader could not be started at all
/// (thread spawn failure; the scene is left untouched). The OS error text
/// passes through untranslated (spec §6.4).
pub fn loader_start_failed(locale: Locale, error: &impl Display) -> String {
    let error = error.to_string();
    interpolate(
        resolve(locale, TextKey::LoaderStartFailed),
        &[("error", &error)],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_tag_maps_every_zh_tag_to_simplified_chinese() {
        for tag in [
            "zh", "zh-CN", "zh-Hans", "zh-Hant", "zh-TW", "zh-HK", "zh_CN", "ZH", "Zh-cn",
            " zh-CN ",
        ] {
            assert_eq!(Locale::from_tag(tag), Locale::ZhCn, "tag {tag:?}");
        }
    }

    #[test]
    fn from_tag_maps_english_family_to_en_without_warning() {
        let before = warn_state::unknown_tag_warnings();
        for tag in ["en", "en-US", "en-GB", "EN-us"] {
            assert_eq!(Locale::from_tag(tag), Locale::En, "tag {tag:?}");
        }
        assert_eq!(warn_state::unknown_tag_warnings() - before, 0);
    }

    #[test]
    fn from_tag_defaults_to_en_and_warns_once_per_unknown_tag() {
        let before = warn_state::unknown_tag_warnings();
        // An unknown tag maps to En…
        assert_eq!(Locale::from_tag("fr"), Locale::En);
        // …and the empty string is unknown too, not English.
        assert_eq!(Locale::from_tag(""), Locale::En);
        assert_eq!(Locale::from_tag("de-DE"), Locale::En);
        assert_eq!(Locale::from_tag("garbage tag"), Locale::En);
        // Repeat the first tag: same result, no second warning.
        assert_eq!(Locale::from_tag("fr"), Locale::En);
        // One warning per distinct unknown tag value.
        assert_eq!(warn_state::unknown_tag_warnings() - before, 4);
    }

    #[test]
    fn locale_names_are_self_names_and_display_tags_are_bcp47() {
        assert_eq!(Locale::En.name(), "English");
        assert_eq!(Locale::ZhCn.name(), "中文（简体）");
        assert_eq!(Locale::En.to_string(), "en");
        assert_eq!(Locale::ZhCn.to_string(), "zh-CN");
        assert_eq!(Locale::En.as_str(), "en");
        assert_eq!(Locale::ZhCn.as_str(), "zh-CN");
    }

    #[test]
    fn tables_cover_exactly_the_key_space_once_with_nonempty_values() {
        let all: HashSet<TextKey> = TextKey::ALL.iter().copied().collect();
        assert_eq!(all.len(), TextKey::ALL.len(), "TextKey::ALL has duplicates");

        for (table, name) in [(EN, "EN"), (ZH, "ZH")] {
            let keys: HashSet<TextKey> = table.iter().map(|(key, _)| *key).collect();
            assert_eq!(keys.len(), table.len(), "{name} table has duplicate keys");
            let missing: Vec<TextKey> = TextKey::ALL
                .iter()
                .copied()
                .filter(|key| !keys.contains(key))
                .collect();
            let extra: Vec<TextKey> = keys
                .iter()
                .copied()
                .filter(|key| !all.contains(key))
                .collect();
            assert!(missing.is_empty(), "{name} misses keys: {missing:?}");
            assert!(extra.is_empty(), "{name} has unknown keys: {extra:?}");
            for (key, value) in table {
                assert!(!value.is_empty(), "{name} value of {key:?} is empty");
            }
        }

        // The two tables are key-aligned: no 漏译 (zh misses an en key) and
        // no 多键 (zh carries a key en dropped).
        let en_keys: HashSet<TextKey> = EN.iter().map(|(key, _)| *key).collect();
        let zh_keys: HashSet<TextKey> = ZH.iter().map(|(key, _)| *key).collect();
        assert_eq!(zh_keys, en_keys);
    }

    #[test]
    fn resolve_returns_copy_for_every_key_in_both_locales() {
        for key in TextKey::ALL {
            let en = resolve(Locale::En, *key);
            let zh = resolve(Locale::ZhCn, *key);
            assert!(!en.is_empty(), "En value of {key:?} is empty");
            assert!(!zh.is_empty(), "Zh value of {key:?} is empty");
            assert_ne!(
                en, zh,
                "{key:?} has identical En and Zh values — untranslated row?"
            );
        }
    }

    #[test]
    fn lookup_finds_rows_and_reports_none_on_empty_table() {
        assert_eq!(lookup(EN, TextKey::MenuFile), Some("File"));
        assert_eq!(lookup(ZH, TextKey::MenuFile), Some("文件"));
        assert_eq!(lookup(&[], TextKey::MenuFile), None);
    }

    /// A5-style scenario: a zh gap (simulated by an injected empty zh
    /// table) resolves to English and warns exactly once per (locale, key)
    /// pair, without panicking.
    #[test]
    fn missing_zh_copy_falls_back_to_english_with_one_warning_per_pair() {
        let before = warn_state::missing_zh_warnings();
        // English never consults the zh side, so no gap and no warning.
        assert_eq!(resolve_from(&[], Locale::En, TextKey::MenuFile), "File");
        assert_eq!(warn_state::missing_zh_warnings() - before, 0);
        // A zh gap falls back to the English value…
        assert_eq!(resolve_from(&[], Locale::ZhCn, TextKey::MenuFile), "File");
        // …and the repeated same pair does not warn twice.
        assert_eq!(resolve_from(&[], Locale::ZhCn, TextKey::MenuFile), "File");
        assert_eq!(warn_state::missing_zh_warnings() - before, 1);
        // A different key is a different pair: warns again.
        assert_eq!(resolve_from(&[], Locale::ZhCn, TextKey::ObjectsFit), "Fit");
        assert_eq!(warn_state::missing_zh_warnings() - before, 2);
    }

    #[test]
    fn interpolate_substitutes_named_tokens_verbatim() {
        assert_eq!(
            interpolate(
                "open \"{file}\" now: {detail}",
                &[("file", "a.ply"), ("detail", "bad")] // A9: test-fixture file name
            ),
            "open \"a.ply\" now: bad"
        );
        // Payloads are appended verbatim, never rescanned.
        assert_eq!(interpolate("{a}", &[("a", "{b}")]), "{b}");
        // Unknown tokens pass through untouched.
        assert_eq!(interpolate("x {unknown} y", &[("a", "1")]), "x {unknown} y");
        // No token, no braces: passthrough.
        assert_eq!(interpolate("plain", &[("a", "1")]), "plain");
    }

    #[test]
    fn kind_labels_cover_every_kind_in_both_locales() {
        for (kind, en, zh) in [
            (DisplayKind::PointCloud, "Point cloud", "点云"),
            (DisplayKind::Mesh, "Mesh", "网格"),
            (DisplayKind::Path, "Path", "路径"),
            (DisplayKind::Frame, "Frame", "坐标架"),
            (DisplayKind::Marker, "Marker", "标记"),
        ] {
            assert_eq!(object_kind_label(Locale::En, kind), en);
            assert_eq!(object_kind_label(Locale::ZhCn, kind), zh);
        }
    }

    /// En copy must stay byte-identical to the pre-locale messages (the
    /// P1 "behavior zero change" guarantee, regression-checked here).
    #[test]
    fn load_failed_en_messages_keep_the_legacy_wording() {
        let unsupported = LoadError::UnsupportedFormat {
            extension: "ply".into(),
        };
        assert_eq!(
            load_failed(Locale::En, "scan.ply", &unsupported), // A9: test-fixture file name
            "Could not open \"scan.ply\":\nUnsupported file format: \"ply\""
        );
        let cloud = LoadError::PointCloud(PointCloudError::Malformed {
            reason: "missing header".into(),
        });
        assert_eq!(
            load_failed(Locale::En, "scene.ply", &cloud), // A9: test-fixture file name
            "Could not open \"scene.ply\":\nMalformed point cloud: missing header"
        );
        let obj = LoadError::Obj(ObjError::Malformed {
            line: 12,
            reason: "unexpected token".into(),
        });
        assert_eq!(
            load_failed(Locale::En, "mesh.obj", &obj), // A9: test-fixture file name
            "Could not open \"mesh.obj\":\nMalformed OBJ file at line 12: unexpected token"
        );
        let obj_limit = LoadError::Obj(ObjError::Limit {
            reason: "too big".into(),
        });
        assert_eq!(
            load_failed(Locale::En, "mesh.obj", &obj_limit), // A9: test-fixture file name
            "Could not open \"mesh.obj\":\nThe OBJ file exceeds the supported load limits: too big"
        );
        let path = LoadError::Path(PathError::Malformed {
            line: 3,
            reason: "bad column count".into(),
        });
        assert_eq!(
            load_failed(Locale::En, "route.csv", &path), // A9: test-fixture file name
            "Could not open \"route.csv\":\nMalformed path file at line 3: bad column count"
        );
        let few = LoadError::Path(PathError::TooFewPoints { points: 1 });
        assert_eq!(
            load_failed(Locale::En, "route.csv", &few), // A9: test-fixture file name
            "Could not open \"route.csv\":\nPath file has too few points (1): a polyline needs \
             at least 2 points"
        );
        let io_error = std::io::Error::other("I/O bomb");
        let io = LoadError::PointCloud(PointCloudError::Io(io_error));
        assert_eq!(
            load_failed(Locale::En, "scene.ply", &io), // A9: test-fixture file name
            "Could not open \"scene.ply\":\nCould not read the file: I/O bomb"
        );
        // Unsupported extension nested under a family behaves like the
        // top-level variant of the same leaf.
        let nested = LoadError::Path(PathError::Io(std::io::Error::other("disk full")));
        assert_eq!(
            load_failed(Locale::En, "route.csv", &nested), // A9: test-fixture file name
            "Could not open \"route.csv\":\nCould not read the file: disk full"
        );
    }

    /// Spec §6.4 boundary: the zh template translates, the machine
    /// payloads (core `reason`, OS error text) pass through in their
    /// original language.
    #[test]
    fn zh_templates_translate_and_machine_payloads_pass_through() {
        let malformed = LoadError::PointCloud(PointCloudError::Malformed {
            reason: "missing header".into(),
        });
        assert_eq!(
            load_failed(Locale::ZhCn, "scene.ply", &malformed), // A9: test-fixture file name
            "无法打开“scene.ply”：\n点云格式错误：missing header"
        );
        let obj = LoadError::Obj(ObjError::Malformed {
            line: 7,
            reason: "duplicate vertex".into(),
        });
        assert_eq!(
            load_failed(Locale::ZhCn, "mesh.obj", &obj), // A9: test-fixture file name
            "无法打开“mesh.obj”：\nOBJ 文件第 7 行格式错误：duplicate vertex"
        );
        let few = LoadError::Path(PathError::TooFewPoints { points: 1 });
        assert_eq!(
            load_failed(Locale::ZhCn, "route.csv", &few), // A9: test-fixture file name
            "无法打开“route.csv”：\n路径文件点数过少（1）：折线至少需要 2 个点"
        );
        let io = LoadError::Path(PathError::Io(std::io::Error::other("permission denied")));
        assert_eq!(
            load_failed(Locale::ZhCn, "route.csv", &io), // A9: test-fixture file name
            "无法打开“route.csv”：\n无法读取文件：permission denied"
        );
    }

    #[test]
    fn loader_messages_interpolate_the_os_error_verbatim() {
        let error = std::io::Error::other("resource busy");
        assert_eq!(
            loader_start_failed(Locale::En, &error),
            "Could not start the background loader: resource busy"
        );
        assert_eq!(
            loader_start_failed(Locale::ZhCn, &error),
            "无法启动后台加载器：resource busy"
        );
        assert_eq!(
            loader_aborted(Locale::En),
            "The background loader ended unexpectedly. The object was not added."
        );
        assert_eq!(
            loader_aborted(Locale::ZhCn),
            "后台加载器意外结束，对象未添加。"
        );
    }

    /// The invariant set never drifts into the tables: window title, axis
    /// letters, the remove icon, and the generated-name templates stay
    /// constant and locale-independent (spec §6.3).
    #[test]
    fn invariant_copy_stays_constant() {
        assert_eq!(WINDOW_TITLE, "RoboView");
        assert_eq!((AXIS_X, AXIS_Y, AXIS_Z), ("X", "Y", "Z"));
        assert_eq!(OBJECTS_REMOVE, "🗑");
        assert_eq!(default_frame_name(3), "Frame 3");
        assert_eq!(default_marker_name(4), "Marker 4");
    }
}

/// Test-visible bookkeeping of the one-time warnings. Kept out of the
/// production code paths (the counters only exist under `cfg(test)`, where
/// the tests measure deltas; the production side effects are the `warn!`
/// calls themselves).
#[cfg(test)]
mod warn_state {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static MISSING_ZH_WARNINGS: AtomicUsize = AtomicUsize::new(0);
    static UNKNOWN_TAG_WARNINGS: AtomicUsize = AtomicUsize::new(0);

    pub(super) fn count_missing_zh() {
        MISSING_ZH_WARNINGS.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn count_unknown_tag() {
        UNKNOWN_TAG_WARNINGS.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn missing_zh_warnings() -> usize {
        MISSING_ZH_WARNINGS.load(Ordering::Relaxed)
    }

    pub(super) fn unknown_tag_warnings() -> usize {
        UNKNOWN_TAG_WARNINGS.load(Ordering::Relaxed)
    }
}
