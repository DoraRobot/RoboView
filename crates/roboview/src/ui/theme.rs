//! Semantic palette of the RoboView UI: every token that carries a meaning
//! across panels and the viewport — selection highlight, ground grid,
//! world-origin axes — is defined here once (004 spec §6 "语义色板", M8)
//! instead of being picked ad hoc per panel.
//!
//! # A9 assertion subset
//!
//! The spec pins exactly three tokens by unit test (004 spec §4 A9;
//! channel UT + MAN): [`SELECT_HIGHLIGHT`], [`GRID_LINE`], and
//! [`ORIGIN_AXIS`]. The three share one rationale — they must keep a
//! defined relation to the scene semantics: the selection highlight is a
//! *new* orange that can never be mistaken for an axis color (the 002
//! semantic colors, display-types spec A4: X red / Y green / Z blue), the
//! grid is a neutral gray darker than every semantic color, and the origin
//! axes are exactly the frame-axis colors the core line pipeline draws —
//! *referencing* the published constants of `roboview_core::render::line`
//! instead of copying their literals, so the app palette and the core
//! frame colors cannot drift apart. The remaining tokens below (HUD text,
//! panel background, indicator base, viewport floor) are style values;
//! 004 spec §6 keeps them out of the assertion set.
//!
//! # Two color representations
//!
//! The A9 tokens are [`roboview_core::io::Color`] sRGB byte triples — the
//! same representation the GPU pipelines consume as per-vertex colors,
//! which is what makes the grid and axis tokens directly comparable with
//! core. The chrome tokens are `egui::Color32`, the painter's own type.
//! [`to_color32`] bridges a byte token into egui at the paint edge, so no
//! token ever exists in two hand-maintained representations.

//! # Dead-code note (delete as tokens get wired)
//!
//! The app crate is a binary, so rustc's `dead_code` analysis has no
//! external-interface notion for this module: until the later 004 tasks
//! consume these tokens (viewport grid/axis layer wiring, selection
//! highlight, HUD chrome, panel fills), every public item here is
//! unreachable from `main` and would warn on every build. The module-wide
//! allow below is the single point to remove as each token finds its
//! consumer.
#![allow(dead_code)]

use eframe::egui::Color32;

use roboview_core::io::Color;
use roboview_core::render::line::{AXIS_X_COLOR_SRGB, AXIS_Y_COLOR_SRGB, AXIS_Z_COLOR_SRGB};

/// Builder for the palette's sRGB byte tokens: `io::Color` fields are
/// spelled out once here instead of at every token.
const fn srgb(r: u8, g: u8, b: u8) -> Color {
    Color { r, g, b }
}

/// Selection highlight (orange family) of the tree rows and — from picking
/// onward — of the selected object in the viewport (004 spec §6 选中高亮;
/// A9 assertion token).
///
/// A **new** token that must stay distinguishable from the 002 semantic
/// colors (display-types spec A4): `(255, 128, 0)` sits at hue ≈ 30° —
/// midway between the X-axis red hue (0°) and yellow — so it is
/// unmistakably orange while the closest axis hue (the red X axis) is
/// still ≈ 30° away. The path/arrow amber of the core line pipeline
/// (`LINE_AMBER_SRGB`, hue ≈ 37°) is a paler and less saturated
/// orange-yellow; the selection orange is fully saturated and darker in
/// its green channel, and A9 asserts the separation from the axes.
pub const SELECT_HIGHLIGHT: Color = srgb(255, 128, 0);

/// Ground-grid line color of the viewport helper layer (004 spec §6 地面
/// 网格/网格线; A9 assertion token).
///
/// A neutral gray, darker than the semantic colors that float above it —
/// the origin axis rows (red/green) and the selection highlight (A9
/// asserts the ordering); the Z blue is no longer painted (origin trio
/// removed), so it no longer constrains the gray. `(110, 110, 110)` reads
/// as a proper grid from the default pose yet still recedes behind the
/// saturated colors; `(70, 70, 70)` from the original calibration
/// vanished under video capture and dense low-angle line fusion — the
/// recorded "ghost" feedback (2026-09-05).
pub const GRID_LINE: Color = srgb(110, 110, 110);

/// World-origin axis trio of the viewport helper layer (004 spec §6 世界
/// 原点三轴; A9 assertion token): the X / Y / Z axes of the world origin,
/// X red / Y green / Z blue.
///
/// The tuple *references* the core constants of
/// `roboview_core::render::line` rather than restating their literals, so
/// the app palette is the same color the core frame pipeline draws by
/// construction — A9 asserts both that identity and the 002 semantics the
/// core constants pin (display-types spec §7 F3, A4).
pub const ORIGIN_AXIS: (Color, Color, Color) =
    (AXIS_X_COLOR_SRGB, AXIS_Y_COLOR_SRGB, AXIS_Z_COLOR_SRGB);

/// HUD text color of the viewport overlay layer (004 spec §6 HUD 文字).
/// Style value, outside the A9 assertion set. Near-white — the same white
/// the existing overlay labels paint over their dark backing
/// (ui/viewport.rs), which reads best against the dark floor.
pub const HUD_TEXT: Color32 = Color32::WHITE;

/// Panel background of the four-region layout (004 spec §6 面板背景).
/// Style value, outside the A9 assertion set. Carries the app's dark
/// theme: gray 27 is exactly the `panel_fill` of egui's dark visuals
/// (`Theme::Dark`, set in main.rs), which every panel fill currently
/// derives from — a token wired in place of that derivation changes
/// nothing visually.
pub const PANEL_BACKGROUND: Color32 = Color32::from_gray(27);

/// Neutral base behind the orientation indicator and other viewport
/// corner chrome (004 spec §6 指示器中性底). Style value, outside the A9
/// assertion set. Translucent black at alpha 150 — the same neutral
/// backing the existing overlay labels already use over the 3D content
/// (ui/viewport.rs), independent of what the scene shows behind it.
pub const INDICATOR_BACKGROUND: Color32 = Color32::from_black_alpha(150);

/// Viewport floor: the neutral gray backdrop the 3D content draws over
/// (004 spec §6 视口底中性灰). Style value, outside the A9 assertion set.
/// Today the floor is exactly the panel background — the central panel's
/// fill shows through wherever the scene draws nothing — so the token
/// starts at the same gray 27 and keeps its own name for when the floor
/// tone evolves separately from the panels.
pub const VIEWPORT_FLOOR: Color32 = Color32::from_gray(27);

/// Bridge a palette sRGB byte token into egui's painter color space:
/// opaque, with the same byte values. The scene-level tokens live as
/// [`Color`] (sRGB bytes, the core convention), so egui-side consumers
/// convert once at the paint edge instead of keeping a parallel constant.
pub const fn to_color32(color: Color) -> Color32 {
    Color32::from_rgb(color.r, color.g, color.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Perceptual hue of an sRGB byte color in degrees on the standard RGB
    /// hexagon (`[0°, 360°)`, max-channel sectors; the sector formula is
    /// the textbook HSV hue). `None` for achromatic colors, whose hue is
    /// undefined.
    fn hue(color: Color) -> Option<f64> {
        let r = f64::from(color.r) / 255.0;
        let g = f64::from(color.g) / 255.0;
        let b = f64::from(color.b) / 255.0;
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let delta = max - min;
        if delta == 0.0 {
            return None;
        }
        let sector = if max == r {
            ((g - b) / delta).rem_euclid(6.0)
        } else if max == g {
            (b - r) / delta + 2.0
        } else {
            (r - g) / delta + 4.0
        };
        Some(60.0 * sector)
    }

    /// Relative luminance of an sRGB byte color in linear light (sRGB
    /// decode, rec. 709 coefficients), in `[0, 1]` — the standard measure
    /// a "darker than" claim refers to.
    fn relative_luminance(color: Color) -> f64 {
        fn linear(channel: u8) -> f64 {
            let c = f64::from(channel) / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
    }

    /// The three origin-axis colors, as the A9 tests keep referring to
    /// them.
    fn axis_colors() -> [Color; 3] {
        [AXIS_X_COLOR_SRGB, AXIS_Y_COLOR_SRGB, AXIS_Z_COLOR_SRGB]
    }

    #[test]
    fn select_highlight_is_distinct_in_hue_from_every_axis_color() {
        // A9 (004 spec §4): the selection highlight is its own orange
        // token, hue-distinct from the 002 semantic axis colors
        // (display-types spec A4). The orange (255, 128, 0) sits at hue
        // ≈ 30°; the nearest axis hue — the X-axis red at 0° — is still
        // ≈ 30° away. The 25° floor keeps half the red-to-yellow sector
        // between the highlight and any warm axis drift while leaving the
        // chosen orange its margin.
        for axis in axis_colors() {
            assert_ne!(
                SELECT_HIGHLIGHT, axis,
                "the selection highlight must never equal an axis color {axis:?}"
            );
        }
        let highlight = hue(SELECT_HIGHLIGHT).expect("the orange is saturated");
        assert!(
            (15.0..=45.0).contains(&highlight),
            "the highlight must read as orange, got hue {highlight:.1}°"
        );
        for axis in axis_colors() {
            let axis_hue = hue(axis).expect("axis colors are saturated");
            let delta = (highlight - axis_hue).abs();
            let distance = delta.min(360.0 - delta);
            assert!(
                distance >= 25.0,
                "highlight hue {highlight:.1}° must stay ≥ 25° from the axis at {axis_hue:.1}° \
                 (nearest is the red X axis, ≈ 30° away), got {distance:.1}°"
            );
        }
    }

    #[test]
    fn grid_line_is_a_neutral_gray_darker_than_every_semantic_color() {
        // A9 (004 spec §4): the grid token is a neutral gray (equal RGB
        // channels) and darker than every semantic color it must never
        // compete with — the selection orange and the still-painted axis
        // colors (X red, Y green; the Z blue is no longer drawn since the
        // origin trio was removed, so it does not constrain the gray).
        assert_eq!(GRID_LINE.r, GRID_LINE.g, "grid token must be neutral gray");
        assert_eq!(GRID_LINE.g, GRID_LINE.b, "grid token must be neutral gray");
        let grid_luminance = relative_luminance(GRID_LINE);
        for semantic in [SELECT_HIGHLIGHT]
            .into_iter()
            .chain(axis_colors().into_iter().take(2))
        {
            let luminance = relative_luminance(semantic);
            assert!(
                grid_luminance < luminance,
                "grid line (luminance {grid_luminance:.3}) must be darker than the semantic \
                 color {semantic:?} (luminance {luminance:.3})"
            );
        }
    }

    #[test]
    fn origin_axis_token_references_core_constants_with_002_semantics() {
        // A9 (004 spec §6): the origin-axes token is the frame-axis color
        // of the core line pipeline — referenced, not copied. The identity
        // checks below pin the reference link itself (the tuple is built
        // from the core constants, and replacing it with restated literals
        // fails here); the literal checks pin the 002 semantics the core
        // constants stand for — X red / Y green / Z blue (display-types
        // spec A4, §7 F3) — mirroring the core-side pinning test in
        // render/line.rs.
        assert_eq!(
            ORIGIN_AXIS.0, AXIS_X_COLOR_SRGB,
            "X must be the core X constant"
        );
        assert_eq!(
            ORIGIN_AXIS.1, AXIS_Y_COLOR_SRGB,
            "Y must be the core Y constant"
        );
        assert_eq!(
            ORIGIN_AXIS.2, AXIS_Z_COLOR_SRGB,
            "Z must be the core Z constant"
        );

        let (x, y, z) = ORIGIN_AXIS;
        assert_eq!(x, Color { r: 255, g: 0, b: 0 }, "X red (002 A4)");
        assert_eq!(y, Color { r: 0, g: 255, b: 0 }, "Y green (002 A4)");
        assert_eq!(z, Color { r: 0, g: 0, b: 255 }, "Z blue (002 A4)");
    }
}
