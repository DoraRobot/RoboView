//! System font loading for the i18n work (spec 003 §6.1, plan §3.3).
//!
//! egui's builtin fonts cover Latin, emoji, and icon glyphs only: arrows
//! such as `→` (U+2192) and every CJK character render as the replacement
//! box (`◻`). RoboView ships no font files (spec §5 non-goal), so the
//! missing glyphs come from the fonts installed on the host, discovered
//! through `fontdb` and appended to egui's fallback chains.
//!
//! The final chain therefore is `egui builtins (Latin/emoji/icons),
//! untouched and first, followed by the system candidates at the tails of
//! both the Proportional and the Monospace chain`. Per spec §6.1 the
//! appended tail holds each platform's symbol/Latin and CJK candidates in
//! the spec's candidate order, and the CJK group ends up in the tail
//! region — the coverage guarantee for user data such as marker labels
//! and file stems under any locale (spec US3). Builtin fonts are never
//! replaced and nothing is ever inserted ahead of them: glyphs the
//! builtins already cover keep their metrics and designs.
//!
//! The font database scan runs once per process (the system font set
//! cannot change while the app runs) and the `FontDefinitions` assembly
//! happens at startup, before the first frame, so locale switching at
//! runtime never touches fonts (spec A4/A7 hold trivially; M5 timing is
//! anchored by the caller in `RoboViewApp::new`).
//!
//! Failure mode (spec §6.1, M1): when no candidate family exists on the
//! machine the function logs one warning and returns the untouched
//! builtin definitions; the M1 "no tofu" verdict then degrades to a
//! reference note, and the A2 unit test records itself as skipped
//! instead of asserting.
//!
//! The A2 probe string is `→ … X/Y/Z 中文测试`. It deliberately avoids
//! `?` and `◻`: epaint resolves missing glyphs to `◻` (fallback `?`) and
//! `has_glyph` reports a false negative for the replacement character
//! itself (spec plan §4).
//!
//! Module wiring: `RoboViewApp::new` calls [`load_system_fonts`] and
//! installs the result with `Context::set_fonts` before the first frame.

use std::sync::{Arc, Mutex};

use eframe::egui::{FontData, FontDefinitions, FontFamily};
use fontdb::Database;

#[cfg(test)]
use eframe::egui::{
    FontId,
    epaint::{AlphaFromCoverage, text::Fonts},
};

/// Candidate font families by platform group (spec 003 §6.1), in the
/// order they are appended to the chains. Every family is *tried* on
/// every OS — candidates "exist then add" — but the current platform's
/// group is searched first so its preferred design wins for characters
/// that several candidates share.
const PLATFORM_GROUPS: [&[&str]; 3] = [
    // macOS.
    &[
        "Hiragino Sans GB",
        "STHeiti",
        "Apple Symbols",
        "PingFang SC",
    ],
    // Windows.
    &["Microsoft YaHei", "SimHei", "Segoe UI Symbol"],
    // Linux and other Unix-like systems.
    &["Noto Sans CJK SC", "DejaVu Sans"],
];

/// Prefix of the egui `font_data` keys owned by this module
/// (`system-<sanitized family name>`); distinguishes our fonts from the
/// builtin ones in the definitions.
const SYSTEM_KEY_PREFIX: &str = "system-";

/// Probe string of the A2/M1 glyph-coverage assertions (spec 003 A2).
/// Literal CJK is the point of the probe; see the module docs for the
/// `?`/`◻` caveat. Test-only: it exists to drive assertions, and no
/// production build needs it.
#[cfg(test)]
const PROBE_TEXT: &str = "→ … X/Y/Z 中文测试";

/// The subset of the candidate list that carries CJK glyphs. Presence of
/// any of these in the assembled definitions is the A2 pre-condition
/// ("a font able to cover the probe characters is a prerequisite of the
/// assertion", spec §2 M1); the symbol-only candidates (Apple Symbols,
/// Segoe UI Symbol, DejaVu Sans) cannot cover the CJK half of the probe.
/// Test-only, like the probe itself.
#[cfg(test)]
const CJK_CANDIDATE_FAMILIES: [&str; 6] = [
    "Hiragino Sans GB",
    "STHeiti",
    "PingFang SC",
    "Microsoft YaHei",
    "SimHei",
    "Noto Sans CJK SC",
];

/// The process-wide font database: scanning the system font directories
/// is the expensive one-time step (spec §6.1 caches it). A `Mutex`
/// instead of a `OnceLock` so that `#[cfg(test)]` runs can rebuild the
/// database (tests call [`load_system_fonts`] many times and must not
/// share stale state).
static SYSTEM_DATABASE: Mutex<Option<Arc<Database>>> = Mutex::new(None);

/// Returns the process-wide system font database, scanning the system
/// font directories once on first use (fontdb `load_system_fonts` picks
/// the platform-correct entry point; fontdb's default features include
/// the Linux fontconfig support).
fn system_database() -> Arc<Database> {
    let mut slot = SYSTEM_DATABASE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(database) = slot.as_ref() {
        return Arc::clone(database);
    }
    let mut database = Database::new();
    database.load_system_fonts();
    let scanned = database.len();
    let database = Arc::new(database);
    *slot = Some(Arc::clone(&database));
    tracing::debug!(scanned, "system font scan complete");
    database
}

/// Forces the next [`load_system_fonts`] to re-scan the system fonts.
/// Test-only: unit tests share the process-wide cache with the app and
/// with each other, and must be able to rebuild it deterministically.
#[cfg(test)]
pub(crate) fn reset_for_tests() {
    let mut slot = SYSTEM_DATABASE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *slot = None;
}

/// Assembles egui font definitions for the app: the builtin definitions
/// with every installed spec-003 candidate family appended to the tails
/// of both the `Proportional` and the `Monospace` chain.
///
/// Fonts are read into owned bytes and registered under a stable key
/// (`system-<family>`, sanitized to lowercase alphanumerics and
/// hyphens); `.ttc` collection faces keep their face index
/// (`fontdb::FaceInfo::index` → `FontData::index`), so the correct face
/// of a collection file is used on macOS and Windows.
///
/// When no candidate family is installed the builtin definitions are
/// returned untouched, with one warning (spec §6.1 empty guard).
pub fn load_system_fonts() -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    let database = system_database();

    let mut appended: Vec<&'static str> = Vec::new();
    for family in candidate_families() {
        let Some(face_id) = database.query(&fontdb::Query {
            // Defaults pick the normal-style, normal-weight, normal-stretch face.
            families: &[fontdb::Family::Name(family)],
            ..Default::default()
        }) else {
            continue; // Not installed: candidates are tried on existence.
        };
        let Some(face) = database.face(face_id) else {
            continue;
        };
        // Read the file once, into memory owned by the FontData: egui
        // (and the ab_glyph parse inside it) must not depend on the
        // database or on the file being re-readable later.
        let Some(bytes) = face_bytes(&face.source) else {
            // File vanished between the scan and this read (or became
            // unreadable): skip the family, the chain still works.
            tracing::trace!(family, "system font file could not be read");
            continue;
        };

        let key = font_key_for(family);
        let data = FontData {
            index: face.index, // .ttc collection: keep the scanned face.
            ..FontData::from_owned(bytes)
        };
        defs.font_data.insert(key.clone(), Arc::new(data));
        if let Some(chain) = defs.families.get_mut(&FontFamily::Proportional) {
            chain.push(key.clone());
        }
        if let Some(chain) = defs.families.get_mut(&FontFamily::Monospace) {
            chain.push(key);
        }
        appended.push(family);
    }

    if appended.is_empty() {
        // Spec §6.1 empty guard: fall back to the builtin fonts; the M1
        // verdict degrades to a reference note (spec §2).
        tracing::warn!(
            "no spec-003 system font candidates found; using the egui builtin fonts only (M1 degrades to a reference note)"
        );
    } else {
        tracing::debug!(?appended, "system fonts appended to the fallback chains");
    }
    defs
}

/// Headless probe parameters (spec plan §3.3): `has_glyph` needs only
/// the cmap of each face, so one point per pixel and a small atlas are
/// enough — no GPU or window is involved. Test-only.
#[cfg(test)]
const PROBE_PIXELS_PER_POINT: f32 = 1.0;

/// See [`PROBE_PIXELS_PER_POINT`].
#[cfg(test)]
const PROBE_MAX_TEXTURE_SIDE: usize = 4096;

/// Headless glyph probe for the A2/M1 assertions: for every character of
/// `probe` (in order, duplicates kept) reports whether it renders as a
/// real glyph — not the `◻`/`?` replacement — in *both* the
/// `Proportional` and the `Monospace` chain of `defs` (spec A2 queries
/// each family once; a character missing from either chain would show a
/// box there). Test-only, like the probe string it is built for.
#[cfg(test)]
pub(crate) fn probe_has_glyphs(defs: &FontDefinitions, probe: &str) -> Vec<(char, bool)> {
    let fonts = Fonts::new(
        PROBE_PIXELS_PER_POINT,
        PROBE_MAX_TEXTURE_SIDE,
        AlphaFromCoverage::default(),
        defs.clone(),
    );
    let proportional = FontId::proportional(14.0);
    let monospace = FontId::monospace(14.0);
    probe
        .chars()
        .map(|c| {
            let present = fonts.has_glyph(&proportional, c) && fonts.has_glyph(&monospace, c);
            (c, present)
        })
        .collect()
}

/// Stable egui `font_data` key of a system family: `system-` followed by
/// the family name lowered and with every run of non-alphanumeric
/// characters collapsed into one hyphen (`"Hiragino Sans GB"` →
/// `"system-hiragino-sans-gb"`). Deterministic across calls and
/// processes, so the same face always lives under the same key.
fn font_key_for(family: &str) -> String {
    let mut key = String::with_capacity(SYSTEM_KEY_PREFIX.len() + family.len());
    key.push_str(SYSTEM_KEY_PREFIX);
    let mut just_separated = false;
    for c in family.chars() {
        if c.is_alphanumeric() {
            for lowered in c.to_lowercase() {
                key.push(lowered);
            }
            just_separated = false;
        } else if !just_separated {
            key.push('-');
            just_separated = true;
        }
    }
    while key.ends_with('-') {
        key.pop();
    }
    key
}

/// The candidate families in attempt order: the current platform's group
/// first, then every other group (a family that ships on another OS may
/// still be installed here, and is appended all the same when it exists).
fn candidate_families() -> impl Iterator<Item = &'static str> {
    let first = platform_group_index();
    (0..PLATFORM_GROUPS.len())
        .flat_map(move |offset| PLATFORM_GROUPS[(first + offset) % PLATFORM_GROUPS.len()])
        .copied()
}

/// Index (into [`PLATFORM_GROUPS`]) of the platform group tried first on
/// the current OS: the macOS group.
#[cfg(target_os = "macos")]
fn platform_group_index() -> usize {
    0
}

/// Index (into [`PLATFORM_GROUPS`]) of the platform group tried first on
/// the current OS: the Windows group.
#[cfg(target_os = "windows")]
fn platform_group_index() -> usize {
    1
}

/// Index (into [`PLATFORM_GROUPS`]) of the platform group tried first on
/// the current OS: the Linux group and every other Unix-like desktop OS.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn platform_group_index() -> usize {
    2
}

/// Reads the full byte content of a face's source into memory.
fn face_bytes(source: &fontdb::Source) -> Option<Vec<u8>> {
    match source {
        fontdb::Source::File(path) | fontdb::Source::SharedFile(path, _) => {
            std::fs::read(path).ok()
        }
        fontdb::Source::Binary(data) => Some(data.as_ref().as_ref().to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Assertion helper: the spec-003 CJK candidates present in `defs`
    /// (their `system-*` keys exist in `font_data`). The A2 probe is only
    /// meaningful when at least one such family reached the chains.
    fn cjk_candidates_in(defs: &FontDefinitions) -> usize {
        CJK_CANDIDATE_FAMILIES
            .iter()
            .filter(|family| defs.font_data.contains_key(&font_key_for(family)))
            .count()
    }

    #[test]
    fn font_key_sanitization_is_stable() {
        assert_eq!(font_key_for("Hiragino Sans GB"), "system-hiragino-sans-gb");
        assert_eq!(font_key_for("Microsoft YaHei"), "system-microsoft-yahei");
        assert_eq!(font_key_for("Noto Sans CJK SC"), "system-noto-sans-cjk-sc");
        assert_eq!(font_key_for("PingFang SC"), "system-pingfang-sc");
        assert_eq!(font_key_for("Segoe UI Symbol"), "system-segoe-ui-symbol");
    }

    #[test]
    fn candidate_family_keys_are_unique() {
        // One key per candidate, no collisions across groups: the keys
        // become the egui font names, which must stay distinct.
        let mut keys: Vec<String> = candidate_families().map(font_key_for).collect();
        let before = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(
            keys.len(),
            before,
            "candidate families collide after sanitization"
        );
        assert!(!keys.is_empty());
    }

    /// Spec 003 A1 (UT+CI): the fallback chains carry at least one
    /// system font family at the tail, every key a chain references
    /// exists in `font_data` (0.32.3 resolves a dangling key at first use
    /// instead of at install time), and the builtin fonts are untouched
    /// at the head — only appended, never replaced (spec §6.1).
    #[test]
    fn a1_system_fonts_sit_at_chain_tails_without_dangling_keys() {
        reset_for_tests();
        let defs = load_system_fonts();
        let base = FontDefinitions::default();

        let proportional = &defs.families[&FontFamily::Proportional];
        let monospace = &defs.families[&FontFamily::Monospace];
        let base_proportional = &base.families[&FontFamily::Proportional];
        let base_monospace = &base.families[&FontFamily::Monospace];

        if proportional.len() == base_proportional.len() {
            // No candidate family installed (spec §6.1 empty guard
            // already logged the warning): record and skip. CI images
            // always carry fonts, so the assertions below really run
            // there.
            eprintln!("A1 skipped: no spec-003 system font candidates found on this machine");
            return;
        }

        // Heads untouched: the builtin prefix is byte-identical.
        assert_eq!(
            &proportional[..base_proportional.len()],
            &base_proportional[..]
        );
        assert_eq!(&monospace[..base_monospace.len()], &base_monospace[..]);

        // Both chains grow by the same appended tail of system keys.
        let appended_proportional = &proportional[base_proportional.len()..];
        let appended_monospace = &monospace[base_monospace.len()..];
        assert_eq!(appended_proportional, appended_monospace);
        assert!(!appended_proportional.is_empty());
        assert!(
            appended_proportional
                .iter()
                .all(|key| key.starts_with(SYSTEM_KEY_PREFIX)),
            "appended tail must only contain system-* keys"
        );

        // No dangling keys: every referenced font exists in font_data.
        for key in proportional.iter().chain(monospace.iter()) {
            assert!(
                defs.font_data.contains_key(key),
                "chain references {key:?}, but font_data has no such font"
            );
        }

        // Nothing extraneous was registered either.
        for key in defs.font_data.keys() {
            assert!(
                base.font_data.contains_key(key) || key.starts_with(SYSTEM_KEY_PREFIX),
                "unexpected font_data key {key:?}"
            );
        }
    }

    /// Spec 003 A2 (UT+CI): every character of the probe string renders
    /// without the replacement glyph in the `Proportional` and the
    /// `Monospace` chain alike. Pre-condition (spec M1): a CJK-capable
    /// candidate must be present — on machines without one (no Noto CJK,
    /// no STHeiti/PingFang/YaHei/SimHei) the assertion cannot hold and
    /// the test records itself as skipped instead of failing; CI installs
    /// `fonts-noto-cjk` on Linux and the macOS/Windows runners ship CJK
    /// fonts, so the assertion really runs on all three platforms.
    #[test]
    fn a2_probe_string_renders_without_replacement_glyphs() {
        reset_for_tests();
        let defs = load_system_fonts();

        if cjk_candidates_in(&defs) == 0 {
            eprintln!(
                "A2 skipped: no CJK-capable candidate (Noto Sans CJK SC / STHeiti / \
                 PingFang SC / Microsoft YaHei / SimHei) found on this machine"
            );
            return;
        }

        let results = probe_has_glyphs(&defs, PROBE_TEXT);
        assert_eq!(
            results.len(),
            PROBE_TEXT.chars().count(),
            "probe must report every character exactly once per chain pair"
        );
        let missing: Vec<char> = results
            .iter()
            .filter(|(_, present)| !present)
            .map(|(c, _)| *c)
            .collect();
        assert!(
            missing.is_empty(),
            "probe characters missing from the Proportional or Monospace chain \
             (spec 003 A2): {missing:?}"
        );
    }

    /// Probe tool self-check: the report keeps the character list of the
    /// probe intact (order and duplicates).
    #[test]
    fn probe_reports_every_character_in_order() {
        let defs = FontDefinitions::default();
        let results = probe_has_glyphs(&defs, PROBE_TEXT);
        let characters: Vec<char> = results.iter().map(|(c, _)| *c).collect();
        assert_eq!(characters, PROBE_TEXT.chars().collect::<Vec<_>>());
    }

    /// Probe tool self-check: true/false discrimination is correct for
    /// characters whose status is known on every machine — 'A' lives in
    /// the builtin Latin font, U+FFFF is in no font at all.
    #[test]
    fn probe_distinguishes_present_and_missing_glyphs() {
        let defs = FontDefinitions::default();
        let results = probe_has_glyphs(&defs, "A\u{FFFF}");
        assert_eq!(results, vec![('A', true), ('\u{FFFF}', false)]);
    }
}
