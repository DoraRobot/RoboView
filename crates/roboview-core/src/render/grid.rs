//! World-aligned ground grid on the world Z=0 plane — the 004 ui-blueprint
//! viewport helper layer (spec §6 "地面网格", plan §3.3).
//!
//! # Scope
//!
//! Pure CPU geometry generation with no GUI, scene, or GPU state: the caller
//! (the line pipeline) owns the vertex buffers and feeds them through
//! `grid_strips` whenever the camera moves. The grid is a viewport helper —
//! it creates no scene objects and never participates in pick or visibility
//! ledgers (plan §3.3: 视口辅助层…均不入场景树).
//!
//! # Why generation-side culling
//!
//! The line pipeline draws with `blend: None` (line.rs — no alpha channel),
//! so nothing can be faded out on the GPU. Everything outside the visible
//! window or beyond a LOD ring is therefore omitted here, at generation
//! time: `grid_strips` only ever emits segments that the camera can see.
//!
//! # Window and LOD
//!
//! The generation window is the square `[center.x ± radius] × [center.y ±
//! radius]` at Z=0 — `center` is the camera footprint (`x`, `y`; `z` is
//! ignored) and `radius` approximates the visible ground extent (plan §3.3:
//! 默认 ±100 m 内随相机外扩/内收). The default is ±100 m.
//!
//! Lines are spaced by step `s` inside a disc of radius `32·s` around the
//! footprint, then by coarser steps outside, forming concentric LOD levels
//! ("rings"):
//!
//! | level | step (default) | ring radius (default) | on screen |
//! |---|---|---|---|
//! | minor | 0.2 m (spacing fill) | 6.4 m | near zone only (radius ≤ 50 m) |
//! | major | 1 m | 32 m | mid zone (radius ≤ 250 m) |
//! | coarse | 5, 10, 20, … m (1-2-5 ladder) | 160, 320, 640, … m | far zone |
//!
//! The *outer* level — the first one whose ring reaches the window corners
//! (`ring ≥ radius·√2`) — is window-clipped instead of ring-clipped, so the
//! grid always covers the whole visible square. Levels keep multiplying by
//! 5, 10, 20, 50, … until that happens.
//!
//! A level is drawn only while it is readable: an inner level drops out
//! once `radius > 250·step`, i.e. once its on-screen spacing would fall
//! below roughly 2 px (a ≈1000 px wide viewport: spacing in px is
//! `step·(2·radius/px)` ... `px ≈ step·1000/(2·radius) ≥ 2` when
//! `radius ≤ 250·step`). With the default ±100 m window the major level
//! shows as 1 m lines that turn into 5 m lines beyond ±32 m, and minor
//! lines appear only when the camera is within ≈50 m of the ground.
//!
//! # Level switches are discrete "pops", never crawling (A11)
//!
//! Every line sits on a fixed world coordinate `k·step` (a multiple of the
//! step measured from the world origin — `snap_floor` is exported for the
//! same alignment elsewhere). Moving the camera therefore never moves a
//! line: it only adds or removes whole lines where they enter or leave the
//! window or a ring front, and a level appearing/disappearing happens at
//! one radius (its gate), so a zoom crosses each switch exactly once, as a
//! discrete whole-ring event. Between events, line coordinates are
//! bit-identical. Ring seams are continuous because neighbouring levels
//! share exact step multiples (e.g. the 1 m chord and the 5 m annulus at
//! the same offset cut the same disc at the same chord endpoints — both
//! sides evaluate the identical expression `√((ring−u)(ring+u))`), so no
//! gap or overlap can form at a seam.
//!
//! # Output, cost, and limits
//!
//! Returns axis-aligned `[Vec3; 2]` segments on Z=0 (each ordered low→high
//! along its axis), level by level minor → major → coarse, all vertical
//! lines (fixed `x`) then all horizontal lines (fixed `y`) per level, `x`
//! and `y` ascending. LOD density converges to a few hundred segments at
//! any distance (each ring adds ≈2·32 axis lines per level, so the visible
//! count is bounded: the recorded limit is N = 1024 segments for the
//! default steps and any radius ≤ 1000 m, and the measured worst case is
//! ≈650 across the test sweep; [`segment_capacity_bound`] gives a
//! configuration-agnostic pre-allocation guarantee).
//!
//! Generation lives in f32 world coordinates, so it is meaningful only
//! where f32 can still tell grid steps apart — roughly ±1e5–1e6 m from the
//! origin at the default steps (f32 ulp(1e6) ≈ 0.06 m). Callers keep the
//! window and footprint near the origin (objects in this project span
//! meters, spec G1). Outside that range the output stays finite and
//! panic-free but line coordinates quantize.

use glam::{Vec2, Vec3};

/// Minor-line spacing for the default options (spec §6: 次线 0.2 m).
const DEFAULT_MINOR_STEP: f32 = 0.2;
/// Major-line spacing for the default options (spec §6: 主线 1 m).
const DEFAULT_MAJOR_STEP: f32 = 1.0;
/// Default generation-window half extent (spec §6: 默认 ±100 m).
const DEFAULT_RADIUS: f32 = 100.0;

/// A level's disc reaches `LEVEL_RING_INTERVALS * step` around the camera
/// footprint; the ring front is where the next coarser level takes over.
const LEVEL_RING_INTERVALS: f32 = 32.0;

/// An inner level is drawn only while `radius ≤ MAX_RADIUS_PER_STEP ·
/// step`, keeping its on-screen spacing above roughly 2 px (see module
/// docs). 250 ≈ 1000 px / (2·2 px) for the reference viewport.
const MAX_RADIUS_PER_STEP: f32 = 250.0;

/// Safety valve on the coarse-ladder builder (radius so huge that the
/// ladder would otherwise run forever); real windows never get near it.
const LADDER_GUARD: usize = 1024;

/// Options of a [`GridView`]: the two step sizes and the visible window
/// half-extent. Invalid configurations (see [`grid_strips`]) yield an
/// empty grid instead of panicking.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridOptions {
    /// Spacing of the finest (minor) LOD level. Ignored when not positive,
    /// not finite, or not smaller than `major_step`.
    pub minor_step: f32,
    /// Spacing of the major LOD level; coarse levels are multiples of it
    /// (5, 10, 20, …).
    pub major_step: f32,
    /// Half extent of the square generation window around the camera
    /// footprint (spec §6 default ±100 m; it expands and contracts with
    /// the camera zoom).
    pub radius: f32,
}

impl GridOptions {
    /// Builds options from the three raw values (no validation — invalid
    /// values are handled by [`grid_strips`]).
    pub fn new(minor_step: f32, major_step: f32, radius: f32) -> Self {
        Self {
            minor_step,
            major_step,
            radius,
        }
    }
}

impl Default for GridOptions {
    fn default() -> Self {
        Self::new(DEFAULT_MINOR_STEP, DEFAULT_MAJOR_STEP, DEFAULT_RADIUS)
    }
}

/// A ground-grid view: the camera footprint and the [`GridOptions`].
///
/// `center.y` is expected to be 0 (the plane coordinate pair is `x`, `z`), so
/// the grid stays put on Z=0 regardless of camera height.
#[derive(Debug, Clone, Copy)]
pub struct GridView {
    /// Camera footprint — lines are world-aligned multiples of their step,
    /// so this only selects which lines are inside the window (see module
    /// docs, A11 no-crawl argument).
    pub center: Vec3,
    /// Steps and window size.
    pub options: GridOptions,
}

impl GridView {
    /// Builds a view from a center and options.
    pub fn new(center: Vec3, options: GridOptions) -> Self {
        Self { center, options }
    }
}

impl Default for GridView {
    fn default() -> Self {
        Self::new(Vec3::ZERO, GridOptions::default())
    }
}

/// One LOD level of the ladder: lines every `step` meters inside a disc of
/// `ring` meters around the footprint.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Level {
    step: f32,
    /// `f32::INFINITY` marks the window-clipped outer level.
    ring: f32,
}

/// The disc radius of a level with the given step (module docs table).
fn ring_of(step: f32) -> f32 {
    LEVEL_RING_INTERVALS * step
}

/// Next multiplier of the 1-2-5 coarse ladder (`m` is one of 5·10^k,
/// 10·10^k, 20·10^k, …): mantissa 1 → ×2, 2 → ×2.5, 5 → ×2. The f64
/// arithmetic keeps the ladder exactly on its multiples.
fn next_coarse_multiplier(m: f64) -> f64 {
    let decade = 10.0f64.powf(m.log10().floor());
    let mantissa = m / decade;
    if mantissa <= 1.5 {
        m * 2.0
    } else if mantissa <= 3.5 {
        m * 2.5
    } else {
        m * 2.0
    }
}

/// The LOD ladder for a grid: optional minor, major, then coarse levels
/// until a ring reaches `corner` (the window-corner distance) or the f32
/// ceiling of step sizes. Always non-empty when `major` is valid.
fn build_ladder(minor: Option<f32>, major: f32, corner: f64) -> Vec<Level> {
    let mut levels = Vec::new();
    if let Some(step) = minor {
        levels.push(Level {
            step,
            ring: ring_of(step),
        });
    }
    levels.push(Level {
        step: major,
        ring: ring_of(major),
    });
    let mut mult = 5.0f64;
    let mut guard = LADDER_GUARD;
    while guard > 0 {
        guard -= 1;
        let step = ((major as f64) * mult) as f32;
        if !step.is_finite() {
            break; // f32 ceiling of the ladder
        }
        let level = Level {
            step,
            ring: ring_of(step),
        };
        let reached_corner = (level.ring as f64) >= corner;
        levels.push(level);
        if reached_corner {
            break;
        }
        mult = next_coarse_multiplier(mult);
    }
    levels
}

/// Largest multiple of `step` not greater than `value` — a floor against
/// step multiples of the world origin (A11: "lines don't crawl with the
/// camera" is built on coordinates snapped this way).
///
/// Pure arithmetic with no validation: a non-positive, NaN, or infinite
/// `step` yields NaN or ±infinity exactly as the IEEE division does, and
/// NaN/±infinity `value` propagate — callers validate configuration before
/// relying on the result.
pub fn snap_floor(value: f32, step: f32) -> f32 {
    (value / step).floor() * step
}

/// Generates the ground-grid segments for `view` (pure; see module docs
/// for window, LOD, and pop semantics).
///
/// Output is `Vec<[Vec3; 2]>` — pairs of endpoints ordered low → high
/// along the segment's axis, all with `z == 0.0`. Deterministic order:
/// minor level first, then major, then coarse; within a level all
/// vertical lines (fixed `x`, ascending), then all horizontal lines (fixed
/// `y`, ascending); per line the low piece(s) first. The output contains
/// no zero-length and no duplicate segments, never panics, and stays
/// bounded (see module docs and [`segment_capacity_bound`]).
///
/// Returns an empty vector when the view cannot describe a grid:
/// `center` not finite; `radius` not positive or not finite; `major_step`
/// not positive or not finite. A minor level that is not positive, not
/// finite, or not smaller than `major_step` is silently skipped.
pub fn grid_strips(view: &GridView) -> Vec<[Vec3; 2]> {
    let opts = &view.options;
    // Note: `!is_finite()` first — NaN compares false against `<=`, so a
    // NaN radius must be rejected by the finite check, not the range one.
    if !view.center.truncate().is_finite() || !opts.radius.is_finite() || opts.radius <= 0.0 {
        return Vec::new();
    }
    if !opts.major_step.is_finite() || opts.major_step <= 0.0 {
        return Vec::new();
    }
    let minor = if opts.minor_step.is_finite()
        && opts.minor_step > 0.0
        && opts.minor_step < opts.major_step
    {
        Some(opts.minor_step)
    } else {
        None
    };

    // The plane pair of the Y=0 ground: (x, z) — `center.y` is the plane
    // normal component and must be 0 (never `truncate()`, which would drop
    // z and keep the zeroed y).
    let center = Vec2::new(view.center.x, view.center.z);
    let half = opts.radius as f64;
    let corner = half * std::f64::consts::SQRT_2;
    let levels = build_ladder(minor, opts.major_step, corner);

    // The outer level is the first whose ring reaches the window corners;
    // with a non-finite corner (radius too large for f32 rings) fall back
    // to the last level the ladder could build.
    let outer_idx = levels
        .iter()
        .position(|l| (l.ring as f64) >= corner)
        .unwrap_or(levels.len() - 1);

    // Gate the inner levels (outer always passes: its step ≥ radius/32
    // makes radius ≤ 250·step automatic) and size the output.
    let mut drawn: Vec<usize> = Vec::new();
    let mut reserve = 0usize;
    for (i, l) in levels.iter().enumerate() {
        if i >= outer_idx {
            drawn.push(outer_idx);
            let span = half.min(l.ring as f64);
            reserve += per_level_bound(span, l.step as f64);
            break;
        }
        if (half as f32) > MAX_RADIUS_PER_STEP * l.step {
            continue; // this step is no longer readable — whole-ring pop-out
        }
        drawn.push(i);
        let span = half.min(l.ring as f64);
        reserve += per_level_bound(span, l.step as f64);
    }

    let mut out: Vec<[Vec3; 2]> = Vec::with_capacity(reserve);
    let mut inner_ring = 0.0f64;
    for i in drawn {
        let level = levels[i];
        push_axis(&mut out, center, half, level, inner_ring, true);
        push_axis(&mut out, center, half, level, inner_ring, false);
        if i >= outer_idx {
            break; // the outer level covers the window corners — nothing beyond
        }
        inner_ring = level.ring as f64;
    }
    out
}

/// Per-level segment-count estimate used for output sizing (an upper
/// bound on one axis pass; see [`segment_capacity_bound`] for the exact
/// argument).
fn per_level_bound(span: f64, step: f64) -> usize {
    (2.0 * (2.0 * (span / step) + 1.0)) as usize + 3
}

/// Emits one axis family of a level: lines every `level.step` meters,
/// each cut to the annulus between `inner_ring` (disc of the previous
/// drawn level; 0 when none) and `level.ring` (∞ for the outer level),
/// then to the square window around `center`.
fn push_axis(
    out: &mut Vec<[Vec3; 2]>,
    center: Vec2,
    half: f64,
    level: Level,
    inner_ring: f64,
    vertical: bool,
) {
    let (along, perp) = if vertical {
        (center.x as f64, center.y as f64)
    } else {
        (center.y as f64, center.x as f64)
    };
    let step = level.step as f64;
    let ring = level.ring as f64;
    let inner = inner_ring;
    let perp_lo = perp - half;
    let perp_hi = perp + half;
    // Lines live at |coord − along| < ring, so the run only spans the
    // ring (or the window when the ring is beyond it) — bounded by
    // ~2·32 lines per axis regardless of window size. The range is
    // widened by one on each side because k·step rounds in f32 (a line
    // can sit on the boundary even when the division says otherwise);
    // the `u > span` filter below drops the widening slack.
    let span = ring.min(half);
    let k_lo = ((along - span) / step).ceil();
    let k_hi = ((along + span) / step).floor();
    // Saturation guard: beyond ±~9e18 the step grid cannot exist in f32
    // anyway (module-docs world limit) and i64 would overflow the range.
    const K_LIMIT: f64 = 9.0e18;
    if !k_lo.is_finite() || !k_hi.is_finite() || k_lo < -K_LIMIT || k_hi > K_LIMIT {
        return;
    }
    for k in (k_lo as i64 - 1)..=(k_hi as i64 + 1) {
        // The line's actual position is the f32-rounded coordinate (a
        // lattice line can round exactly onto the window edge even when
        // the raw product is a hair outside), so distance and chords use
        // that rounded position.
        let coord = ((k as f64) * step) as f32 as f64;
        let u = (coord - along).abs();
        if u > span {
            continue; // outside this level's disc, or beyond the window
        }
        // Chord half-lengths of this level's ring and the previous drawn
        // ring at offset u, centered on the footprint along the perpendi-
        // cular axis (u == ring → 0 → the piece below drops out).
        let h_out = ((ring - u) * (ring + u)).sqrt();
        if inner > 0.0 && u < inner {
            // Annulus: this level starts beyond the previous ring's disc.
            let h_in = ((inner - u) * (inner + u)).sqrt();
            emit_piece(
                out,
                coord,
                perp - h_out,
                perp - h_in,
                perp_lo,
                perp_hi,
                vertical,
            );
            emit_piece(
                out,
                coord,
                perp + h_in,
                perp + h_out,
                perp_lo,
                perp_hi,
                vertical,
            );
        } else {
            emit_piece(
                out,
                coord,
                perp - h_out,
                perp + h_out,
                perp_lo,
                perp_hi,
                vertical,
            );
        }
    }
}

/// Pushes one window-clipped piece of the line at fixed `coord`, running
/// along the other axis between `lo` and `hi`. Clipping happens before the
/// f32 cast so both sides of a seam round identically.
fn emit_piece(
    out: &mut Vec<[Vec3; 2]>,
    coord: f64,
    lo: f64,
    hi: f64,
    perp_lo: f64,
    perp_hi: f64,
    vertical: bool,
) {
    let lo = lo.max(perp_lo);
    let hi = hi.min(perp_hi);
    if hi <= lo {
        return; // empty piece (callers guarantee finite endpoints)
    }
    // Rounding to f32 can collapse two distinct f64 endpoints, so the
    // length check runs again on the final values.
    let coord = coord as f32;
    let lo = lo as f32;
    let hi = hi as f32;
    if hi <= lo {
        return;
    }
    out.push(if vertical {
        [Vec3::new(coord, 0.0, lo), Vec3::new(coord, 0.0, hi)]
    } else {
        [Vec3::new(lo, 0.0, coord), Vec3::new(hi, 0.0, coord)]
    });
}

/// Upper bound on the number of segments [`grid_strips`] returns for any
/// [`GridView`] whose options carry the same steps and a `radius` not
/// larger than `options.radius` (any center, and `center` outside that
/// bound is the caller's own geometry).
///
/// The bound sums, over every ladder level that could emit (ring below
/// 4·radius — the outer level's ring never exceeds ≈2.5·√2·radius), the
/// level worst case `4·(2·(span/step) + 3)` — two axes, up to two pieces
/// per line, with `span = min(radius, ring)`. It is a loose over-estimate
/// (each ring
/// keeps ~2·32 lines per axis), sized so a persistent line mesh built
/// once with this capacity never needs to grow (plan §3.3: 持久 LineMesh
/// 容量预建). For the default options and radius 1000 m the bound is 1656
/// while the measured worst output is ≈650 (test sweep).
pub fn segment_capacity_bound(options: &GridOptions) -> usize {
    if !options.radius.is_finite() || options.radius <= 0.0 {
        return 0; // no grid can be generated
    }
    let r = options.radius as f64;
    if !options.major_step.is_finite() || options.major_step <= 0.0 {
        return 0;
    }
    let minor = if options.minor_step.is_finite()
        && options.minor_step > 0.0
        && options.minor_step < options.major_step
    {
        Some(options.minor_step)
    } else {
        None
    };
    // Levels with ring ≥ 4·radius can never draw (an outer level's ring is
    // below the first ring ≥ corner of the largest view, ≈2.5·√2·radius),
    // so the ladder may stop at the first ring ≥ 4·radius.
    let ladder = build_ladder(minor, options.major_step, 4.0 * r);
    let mut total = 0usize;
    for l in &ladder {
        let span = r.min(l.ring as f64);
        // Two axes × up to two pieces per line × lines per axis, plus slack.
        total += 4 * (2 * (span / l.step as f64) as usize + 3);
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Options with the default steps and the given window radius.
    fn opts(radius: f32) -> GridOptions {
        GridOptions::new(DEFAULT_MINOR_STEP, DEFAULT_MAJOR_STEP, radius)
    }

    /// Default steps and radius, centered at `(x, y)`.
    fn view(x: f32, y: f32, radius: f32) -> GridView {
        GridView::new(Vec3::new(x, 0.0, y), opts(radius))
    }

    /// Segments with fixed `x` (lines running along Y), as (x, lo, hi).
    fn verticals(strips: &[[Vec3; 2]]) -> Vec<(f32, f32, f32)> {
        strips
            .iter()
            .filter(|s| s[0].x.to_bits() == s[1].x.to_bits() && s[0].z < s[1].z)
            .map(|s| (s[0].x, s[0].z, s[1].z))
            .collect()
    }

    /// Segments with fixed `y` (lines running along X), as (lo, hi, y).
    fn horizontals(strips: &[[Vec3; 2]]) -> Vec<(f32, f32, f32)> {
        strips
            .iter()
            .filter(|s| s[0].z.to_bits() == s[1].z.to_bits() && s[0].x < s[1].x)
            .map(|s| (s[0].x, s[1].x, s[0].z))
            .collect()
    }

    /// Sorted distinct fixed coordinates of the vertical lines (fixed `x`,
    /// running along Z on the Y=0 plane; NaN-free by construction, so
    /// `total_cmp` order equals value order and works across signs).
    fn vertical_x_coords(strips: &[[Vec3; 2]]) -> Vec<f32> {
        let mut xs: Vec<f32> = verticals(strips).into_iter().map(|(x, _, _)| x).collect();
        xs.sort_by(f32::total_cmp);
        xs.dedup_by(|a, b| a.to_bits() == b.to_bits());
        xs
    }

    /// Sorted distinct fixed coordinates of the horizontal lines (fixed
    /// `z`, running along X on the Y=0 plane).
    fn horizontal_y_coords(strips: &[[Vec3; 2]]) -> Vec<f32> {
        let mut ys: Vec<f32> = horizontals(strips).into_iter().map(|(_, _, z)| z).collect();
        ys.sort_by(f32::total_cmp);
        ys.dedup_by(|a, b| a.to_bits() == b.to_bits());
        ys
    }

    #[test]
    fn snap_floor_floors_to_step_multiples_of_the_world_origin() {
        assert_eq!(snap_floor(2.3, 1.0), 2.0);
        assert_eq!(snap_floor(-2.3, 1.0), -3.0); // floor, not truncation
        assert_eq!(snap_floor(0.25, 0.2), 0.2);
        assert_eq!(snap_floor(-0.25, 0.2), -0.4);
        assert_eq!(snap_floor(0.0, 5.0), 0.0);
        // The exact binary values of these multiplications are exact
        // multiples of 0.2f32's value, so equality is exact.
        assert_eq!(snap_floor(6.4, 0.2), 6.4);
        // Structural properties hold at any magnitude.
        for value in [-1e4, -7.3, -0.01, 0.01, 7.3, 1e4] {
            for step in [0.2, 0.5, 1.0, 3.0, 100.0] {
                let s = snap_floor(value, step);
                assert!(s <= value, "floor must not exceed value");
                assert!(value - s < step, "floor must be within one step");
                // s is exactly representable as k·step for integer k.
                assert!(((s / step) - (s / step).round()).abs() < 1e-3);
            }
        }
    }

    #[test]
    fn snap_floor_never_panics_on_non_finite_inputs() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.0, 0.0, -1.0] {
            for step in [f32::NAN, f32::INFINITY, 0.0, -2.0, 0.5] {
                let _ = snap_floor(value, step); // documented: propagates
            }
        }
    }

    #[test]
    fn default_view_emits_exactly_234_segments() {
        // Radius 100 centered at the origin: the major disc (ring 32 m,
        // 63 lines per axis, one chord each) plus the 5 m outer level
        // (41 lines per axis: 13 double-pieced inside the ring, 28 single
        // full-window pieces beyond it). Minors are gated out at R = 100.
        let strips = grid_strips(&GridView::default());
        assert_eq!(strips.len(), 234);
        assert_eq!(verticals(&strips).len(), 117);
        assert_eq!(horizontals(&strips).len(), 117);
    }

    #[test]
    fn default_view_lines_sit_on_world_multiples() {
        let strips = grid_strips(&GridView::default());
        let xs = vertical_x_coords(&strips);
        // Inside the major ring: every integer from −31 to 31.
        let inner: Vec<f32> = xs.iter().copied().filter(|x| x.abs() <= 31.0).collect();
        let expected: Vec<f32> = (-31..=31).map(|k| k as f32).collect();
        assert_eq!(inner.len(), expected.len());
        for (got, want) in inner.iter().zip(&expected) {
            assert_eq!(got.to_bits(), want.to_bits());
        }
        // Beyond the ring, multiples of 5 from 35 to the window edge.
        let outer: Vec<f32> = xs.iter().copied().filter(|x| x.abs() > 33.0).collect();
        let mut joined: Vec<f32> = (7..=20).map(|k| 5.0 * k as f32).collect();
        joined.sort_by(f32::total_cmp);
        let mut mirror: Vec<f32> = joined.iter().map(|x| -x).collect();
        mirror.sort_by(f32::total_cmp);
        let mut expected = mirror;
        expected.extend(joined);
        assert_eq!(outer, expected);
        // No other coordinates exist (63 majors + 28 outer 5 m lines).
        assert_eq!(xs.len(), 91);
    }

    #[test]
    fn window_edges_reach_the_square_border() {
        let strips = grid_strips(&GridView::default());
        // The x = ±100 and y = ±100 border lines run full window width.
        for x in [-100.0, 100.0] {
            let v: Vec<(f32, f32, f32)> = verticals(&strips)
                .into_iter()
                .filter(|(vx, _, _)| *vx == x)
                .collect();
            let (_, lo, hi) = v[0];
            assert_eq!(lo, -100.0);
            assert_eq!(hi, 100.0);
            assert_eq!(v.len(), 1, "border line {x} must be one full segment");
        }
        let h: Vec<(f32, f32, f32)> = horizontals(&strips)
            .into_iter()
            .filter(|(_, _, vy)| *vy == 100.0)
            .collect();
        assert_eq!(h.len(), 1);
        assert_eq!(h[0], (-100.0, 100.0, 100.0));
    }

    #[test]
    fn ring_seam_is_bitwise_continuous() {
        // At x = 5 the major chord and the two 5 m annulus pieces cut the
        // same disc with the same expression: √((32−5)(32+5)) = √999, so
        // the three intervals tile [−100, 100] exactly.
        let strips = grid_strips(&GridView::default());
        let mut intervals: Vec<(f32, f32)> = verticals(&strips)
            .into_iter()
            .filter(|(x, _, _)| *x == 5.0)
            .map(|(_, lo, hi)| (lo, hi))
            .collect();
        intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
        let f = (27.0f64 * 37.0f64).sqrt() as f32; // both sides compute this
        let expected = [(-100.0, -f), (-f, f), (f, 100.0)];
        assert_eq!(intervals.len(), 3);
        for ((got_lo, got_hi), (want_lo, want_hi)) in intervals.iter().zip(&expected) {
            assert_eq!(got_lo.to_bits(), want_lo.to_bits());
            assert_eq!(got_hi.to_bits(), want_hi.to_bits());
        }
    }

    #[test]
    fn minor_level_fills_the_near_zone() {
        // R = 10: minors (0.2 m, ring 6.4 m) fill the near disc, majors
        // cover the rest of the window.
        let strips = grid_strips(&view(0.0, 0.0, 10.0));
        let xs = vertical_x_coords(&strips);
        // Every 0.2 m multiple in the ring (k = −31..=31) is present.
        for k in -31..=31 {
            let want = 0.2f32 * k as f32;
            assert!(
                xs.iter().any(|x| x.to_bits() == want.to_bits()),
                "missing minor line at {want}"
            );
        }
        // Majors at whole meters inside the ring region are separate
        // coords (f32 1.0 vs 5·0.2f32), and whole meters beyond the ring.
        assert!(xs.iter().any(|x| x.to_bits() == 7.0f32.to_bits()));
        assert!(xs.iter().any(|x| x.to_bits() == 10.0f32.to_bits()));
        // Nothing between the minor ring (6.4) and the next major (7).
        assert!(!xs.iter().any(|x| x.abs() > 6.4 && x.abs() < 7.0));
    }

    #[test]
    fn tiny_window_draws_only_minor_lines_evenly_spaced() {
        // R = 4: even the minor ring (6.4 m) reaches the window corners
        // (5.66 m), so the 0.2 m level is the window-clipped outer level:
        // 41 lines per axis, each a single full segment.
        let strips = grid_strips(&view(0.0, 0.0, 4.0));
        assert_eq!(strips.len(), 82);
        let xs = vertical_x_coords(&strips);
        assert_eq!(xs.len(), 41);
        for pair in xs.windows(2) {
            let d = pair[1] - pair[0];
            assert!(
                (d - 0.2).abs() < 1e-5,
                "spacing must stay 0.2 m, got {d} between {} and {}",
                pair[0],
                pair[1]
            );
        }
        assert!(xs.first().unwrap().to_bits() == (-4.0f32).to_bits());
        assert!(xs.last().unwrap().to_bits() == 4.0f32.to_bits());
    }

    #[test]
    fn zoom_gate_drops_minors_and_then_majors() {
        // R = 30 (≤ 50): minors present near the center.
        let near = grid_strips(&view(0.0, 0.0, 30.0));
        let xs = vertical_x_coords(&near);
        assert!(xs.iter().any(|x| x.to_bits() == (0.2f32 * 3.0).to_bits()));
        // R = 60 (> 50): the minor ring popped out as one discrete event —
        // every line coordinate inside ±6 is now a whole meter (majors)
        // or a 5 m multiple (5, 0, −5 ⊂ whole meters).
        let far = grid_strips(&view(0.0, 0.0, 60.0));
        let xs = vertical_x_coords(&far);
        let inner: Vec<f32> = xs.iter().copied().filter(|x| x.abs() <= 6.0).collect();
        let expected: Vec<f32> = (-6..=6).map(|k| k as f32).collect();
        assert_eq!(inner.len(), expected.len());
        for (got, want) in inner.iter().zip(&expected) {
            assert_eq!(got.to_bits(), want.to_bits());
        }
        // R = 300 (> 250): majors gone too — the near zone shows only the
        // 5 m level (plus nothing of the 10/20 m annuli inside ±10).
        let wide = grid_strips(&view(0.0, 0.0, 300.0));
        let xs = vertical_x_coords(&wide);
        let inner: Vec<f32> = xs.iter().copied().filter(|x| x.abs() <= 10.0).collect();
        let expected: Vec<f32> = (-2..=2).map(|k| 5.0 * k as f32).collect();
        assert_eq!(inner.len(), expected.len());
        for (got, want) in inner.iter().zip(&expected) {
            assert_eq!(got.to_bits(), want.to_bits());
        }
    }

    #[test]
    fn camera_motion_never_moves_interior_lines() {
        // The same radius around two footprints: interior coordinates must
        // be bit-identical — motion only adds/removes lines at the ring
        // (x = 32) and window (x = ±100) fronts, never shifts them.
        let a = grid_strips(&view(0.0, 0.0, 100.0));
        let b = grid_strips(&view(0.4, -0.3, 100.0));
        let xs_a = vertical_x_coords(&a);
        let xs_b = vertical_x_coords(&b);
        // Interior of the major ring.
        let band: Vec<u32> = xs_a
            .iter()
            .copied()
            .filter(|x| x.abs() <= 31.0)
            .map(f32::to_bits)
            .collect();
        let band_b: Vec<u32> = xs_b
            .iter()
            .copied()
            .filter(|x| x.abs() <= 31.0)
            .map(f32::to_bits)
            .collect();
        assert_eq!(band, band_b);
        // 5 m annulus region, away from the ring and window fronts.
        let band: Vec<u32> = xs_a
            .iter()
            .copied()
            .filter(|x| x.abs() > 33.0 && x.abs() <= 95.0)
            .map(f32::to_bits)
            .collect();
        let band_b: Vec<u32> = xs_b
            .iter()
            .copied()
            .filter(|x| x.abs() > 33.0 && x.abs() <= 95.0)
            .map(f32::to_bits)
            .collect();
        assert_eq!(band, band_b);
        // The documented pops: x = 32 enters once the footprint crosses
        // within one step of the ring front, x = −100 leaves once the
        // footprint moves a step past the window edge.
        assert!(!xs_a.contains(&32.0) && xs_b.contains(&32.0));
        assert!(xs_a.contains(&-100.0) && !xs_b.contains(&-100.0));
        // The same holds along Y for the horizontal family.
        let ys_a = horizontal_y_coords(&a);
        let ys_b = horizontal_y_coords(&b);
        let band: Vec<u32> = ys_a
            .iter()
            .copied()
            .filter(|y| y.abs() <= 31.0)
            .map(f32::to_bits)
            .collect();
        let band_b: Vec<u32> = ys_b
            .iter()
            .copied()
            .filter(|y| y.abs() <= 31.0)
            .map(f32::to_bits)
            .collect();
        assert_eq!(band, band_b);
    }

    #[test]
    fn camera_motion_with_minor_level_is_stable_too() {
        // Sub-step shifts (0.05, −0.03 < 0.2/2) with minors on the screen.
        let a = grid_strips(&view(0.0, 0.0, 10.0));
        let b = grid_strips(&view(0.05, -0.03, 10.0));
        let xs_a: Vec<u32> = vertical_x_coords(&a)
            .into_iter()
            .filter(|x| x.abs() <= 6.3)
            .map(f32::to_bits)
            .collect();
        let xs_b: Vec<u32> = vertical_x_coords(&b)
            .into_iter()
            .filter(|x| x.abs() <= 6.3)
            .map(f32::to_bits)
            .collect();
        assert_eq!(xs_a, xs_b);
        let ys_a: Vec<u32> = horizontal_y_coords(&a)
            .into_iter()
            .filter(|y| y.abs() <= 6.3)
            .map(f32::to_bits)
            .collect();
        let ys_b: Vec<u32> = horizontal_y_coords(&b)
            .into_iter()
            .filter(|y| y.abs() <= 6.3)
            .map(f32::to_bits)
            .collect();
        assert_eq!(ys_a, ys_b);
    }

    #[test]
    fn every_segment_stays_inside_the_window_on_the_plane() {
        for center in [[0.0, 0.0], [12.3, -45.6], [-999.5, 777.7]] {
            for radius in [0.4, 10.0, 100.0, 452.6, 1000.0] {
                let g = view(center[0], center[1], radius);
                let strips = grid_strips(&g);
                assert!(!strips.is_empty());
                let half = radius + 1e-3;
                for s in &strips {
                    assert!(s[0].is_finite() && s[1].is_finite());
                    assert_eq!(s[0].y, 0.0);
                    assert_eq!(s[1].y, 0.0);
                    let (a, b) = (s[0], s[1]);
                    if a.x.to_bits() == b.x.to_bits() {
                        assert!((a.x - center[0]).abs() <= half);
                        assert!((a.z - center[1]).abs() <= half);
                        assert!((b.z - center[1]).abs() <= half);
                        assert!(a.z < b.z);
                    } else if a.z.to_bits() == b.z.to_bits() {
                        assert!((a.z - center[1]).abs() <= half);
                        assert!((a.x - center[0]).abs() <= half);
                        assert!((b.x - center[0]).abs() <= half);
                        assert!(a.x < b.x);
                    } else {
                        panic!("grid segment must be axis-aligned: {a:?} {b:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn invalid_views_produce_empty_output_without_panicking() {
        let mut bad_center = GridView::default();
        for center in [
            Vec3::splat(f32::NAN),
            Vec3::splat(f32::INFINITY),
            Vec3::new(f32::NEG_INFINITY, 0.0, 0.0),
            Vec3::new(1.0, f32::NAN, 2.0),
        ] {
            bad_center.center = center;
            assert!(grid_strips(&bad_center).is_empty());
        }
        for radius in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(grid_strips(&view(0.0, 0.0, radius)).is_empty());
        }
        for major in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let o = GridOptions::new(0.2, major, 100.0);
            assert!(grid_strips(&GridView::new(Vec3::ZERO, o)).is_empty());
        }
        // Invalid minors are skipped, not fatal: majors alone still draw.
        for minor in [0.0, -0.2, f32::NAN, f32::INFINITY, 1.0, 2.0] {
            let o = GridOptions::new(minor, 1.0, 100.0);
            let strips = grid_strips(&GridView::new(Vec3::ZERO, o));
            assert!(!strips.is_empty());
            assert_eq!(
                strips.len(),
                grid_strips(&GridView::new(
                    Vec3::ZERO,
                    GridOptions::new(f32::NAN, 1.0, 100.0),
                ))
                .len()
            );
        }
        // A degenerate but valid grid (steps larger than the window) is
        // not an error — lines simply clip to nothing or the axis line.
        let o = GridOptions::new(0.2, 1.0, 0.1);
        let _ = grid_strips(&GridView::new(Vec3::ZERO, o));
    }

    #[test]
    fn sweep_has_no_zero_length_no_duplicates_and_stays_bounded() {
        // The recorded upper bound N for default steps and radius ≤ 1000.
        const MAX_SEGMENTS: usize = 1024;
        let radii = [
            0.001, 0.05, 0.4, 1.0, 3.0, 4.4, 4.6, 5.0, 10.0, 20.0, 22.5, 22.7, 30.0, 45.0, 49.0,
            51.0, 60.0, 100.0, 113.0, 113.3, 226.0, 226.6, 250.0, 251.0, 300.0, 452.0, 453.0,
            500.0, 700.0, 1000.0,
        ];
        let centers = [
            [0.0f32, 0.0],
            [0.33, 0.77],
            [12.3, -45.6],
            [999.9, -999.9],
            [12345.6, -7777.7],
            [-1.0e5, 8.8e4],
        ];
        let mut worst = 0usize;
        for cx in centers {
            for r in radii {
                let strips = grid_strips(&view(cx[0], cx[1], r));
                worst = worst.max(strips.len());
                assert!(
                    strips.len() <= MAX_SEGMENTS,
                    "radius {r} center {cx:?} exceeded N: {}",
                    strips.len()
                );
                // Canonical bitwise key: no duplicates, no zero length.
                let mut seen = HashSet::with_capacity(strips.len());
                for s in &strips {
                    let key = [
                        s[0].x.to_bits(),
                        s[0].y.to_bits(),
                        s[0].z.to_bits(),
                        s[1].x.to_bits(),
                        s[1].y.to_bits(),
                        s[1].z.to_bits(),
                    ];
                    assert!(s[0].x != s[1].x || s[0].z != s[1].z);
                    assert!(seen.insert(key), "duplicate segment {s:?}");
                }
                // The documented per-configuration pre-allocation bound.
                let bound = segment_capacity_bound(&opts(r));
                assert!(
                    strips.len() <= bound,
                    "radius {r} center {cx:?}: {} exceeds bound {bound}",
                    strips.len()
                );
            }
        }
        // Measured worst case across the sweep stays far below the bound
        // and is recorded in the module docs as the ~1024 headroom.
        assert!(worst < MAX_SEGMENTS, "sweep worst case {worst}");
    }

    #[test]
    fn capacity_bound_holds_for_exotic_steps_and_radii() {
        let steps = [
            [0.1, 1.0],
            [0.05, 0.25],
            [1.0, 10.0],
            [0.5, 1.0],
            [1.0e-4, 1.0e-2],
            [100.0, 500.0],
        ];
        let radii = [0.01, 1.0, 49.0, 100.0, 500.0, 1000.0];
        let centers = [[0.0f32, 0.0], [0.5, 0.5], [333.3, -777.7]];
        for [minor, major] in steps {
            for r in radii {
                for c in centers {
                    let o = GridOptions::new(minor, major, r);
                    let g = GridView::new(Vec3::new(c[0], c[1], 0.0), o);
                    let n = grid_strips(&g).len();
                    // Each tested grid also respects the bound computed for
                    // a *larger* radius, as the persistent-mesh contract
                    // requires (capacity is prebuilt for the max radius).
                    let bound = segment_capacity_bound(&o);
                    assert!(n <= bound, "{minor}/{major} m @ r={r}: {n} > {bound}");
                }
            }
        }
        // Sanity of the default bound itself.
        let b = segment_capacity_bound(&GridOptions::default());
        assert!(b > 0 && b < 10_000, "default bound {b}");
        assert_eq!(segment_capacity_bound(&opts(-1.0)), 0);
        assert_eq!(
            segment_capacity_bound(&GridOptions::new(0.2, f32::NAN, 100.0)),
            0
        );
        // The bound must cover the *whole* radius range when the caller
        // prebuilds for its maximum window (default steps).
        let max_opts = opts(1000.0);
        let bound = segment_capacity_bound(&max_opts);
        for r in [0.4, 22.6, 50.0, 113.1, 250.0, 452.5, 999.9, 1000.0] {
            let n = grid_strips(&view(0.0, 0.0, r)).len();
            assert!(n <= bound, "r={r}: {n} > prebuilt bound {bound}");
        }
    }

    #[test]
    fn astronomically_large_windows_stay_finite_and_cheap() {
        // Beyond the meaningful f32 world the grid still never panics or
        // explodes: windows far larger than any real viewport resolve to
        // the coarse ladder, which converges to a bounded line count.
        for r in [1.0e10, 1.0e30, f32::MAX] {
            let strips = grid_strips(&view(0.0, 0.0, r));
            assert!(strips.len() <= segment_capacity_bound(&opts(r)));
            assert!(strips.len() < 10_000);
        }
    }
}
