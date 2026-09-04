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
//! window is therefore omitted here, at generation time: `grid_strips`
//! only ever emits segments that the camera can see.
//!
//! # Window and step — one uniform grid (004 revised, 2026-09-04)
//!
//! The generation window is the square `[center.x ± radius] × [center.y ±
//! radius]` at Z=0 — `center` is the camera footprint (`x`, `y`; `z` is
//! ignored) and `radius` approximates the visible ground extent (plan §3.3:
//! 默认 ±100 m 内随相机外扩/内收). The default is ±100 m.
//!
//! The grid is **uniform**: every line every `step` meters, measured from
//! the world origin, across the whole window — a coherent plane like the
//! grids of mature 3D tools, never a concentric ring plan. `step` is the
//! smallest 1-2-5 ladder value starting at the base step (default 1 m)
//! whose on-screen spacing at the camera-target plane stays readable
//! (`step · px_per_m ≥ 2 px`):
//!
//! ```text
//! step · px_per_m ≥ 2px   →   step stays as-is (the default pose reads 1 m)
//! step · px_per_m <  2px   →   the WHOLE grid switches to the next ladder
//!                              step (1 → 2 → 5 → 10 → 20 → 50 → 100 m)
//! ```
//!
//! `px_per_m` is the pixels-per-meter at the camera-target plane — it
//! depends on the **zoom** (eye-to-target distance / viewport) only, never
//! on the orientation. Rotating pitch or yaw therefore never reselects
//! the step: the near-field grid keeps its world size, exactly like the
//! floor grids of mature 3D tools (the perspective squeeze far away is a
//! projection fact, not a density change).
//!
//! The generation window (`center ± radius`, the visible-ground measure)
//! is additionally clamped to `250·step`: without alpha blending nothing
//! can fade on the GPU, so the far end of a horizon-grazing view is cut
//! at the same bound the readability gate uses — the no-alpha equivalent
//! of Blender's floor-grid distance fade. The clamp also keeps the
//! per-axis line count ≤ 2·250 + 1 for any pose (`segment_capacity_bound`
//! is unchanged).
//!
//! # Step switches are discrete "pops", never crawling (A11)
//!
//! Every line sits on a fixed world coordinate `k·step` (a multiple of the
//! step measured from the world origin — `snap_floor` is exported for the
//! same alignment elsewhere). Moving the camera therefore never moves a
//! line: it only adds or removes whole lines where they enter or leave the
//! window, and a step change happens at one zoom gate as a discrete
//! whole-grid event. Between events, line coordinates are bit-identical —
//! no crawl, no fade, no ring seams to mind.
//!
//! # Output, cost, and limits
//!
//! Returns axis-aligned `[Vec3; 2]` segments on Z=0 (each ordered low→high
//! along its axis), all vertical lines (fixed `x`) then all horizontal
//! lines (fixed `y`), coordinates ascending, every `step` meters — the
//! uniform grid. The visible count is bounded: the window clamp (250·step)
//! keeps each axis at most ~500 lines (the recorded limit is ~1000
//! segments for any pose; [`segment_capacity_bound`] gives the
//! pre-allocation guarantee).
//!
//! Generation lives in f32 world coordinates, so it is meaningful only
//! where f32 can still tell grid steps apart — roughly ±1e5–1e6 m from the
//! origin at the default steps (f32 ulp(1e6) ≈ 0.06 m). Callers keep the
//! window and footprint near the origin (objects in this project span
//! meters, spec G1). Outside that range the output stays finite and
//! panic-free but line coordinates quantize.

use glam::Vec3;

/// Default uniform grid step (spec §6: 主线 1 m — one step for the whole
/// uniform ground plane).
const DEFAULT_STEP: f32 = 1.0;

/// Default generation-window half extent (spec §6: 默认 ±100 m).
const DEFAULT_RADIUS: f32 = 100.0;

/// Default pixels-per-meter of the step ladder (a typical target-plane
/// density at the reference pose: 1 m step reads at 2 px — the ladder
/// gate itself). Purely a fixture for [`Default`]; the viewport always
/// passes a live value.
const DEFAULT_PX_PER_M: f32 = 2.0;

/// Minimum on-screen spacing of the grid lines at the camera-target plane
/// (see module docs): a step is readable while `step · px_per_m ≥ 2 px`,
/// and the generation window is clamped to 250·step (the no-alpha fade
/// cutoff). 250 ≈ 1000 px / (2·2 px) for the reference viewport.
const MIN_GRID_SCREEN_SPACING_PX: f32 = 2.0;

/// Generation-window clamp in units of the current step (the no-alpha
/// fade cutoff, see module docs): the farthest line is at most 250 steps
/// away from the camera footprint, so a horizon-grazing view cannot grow
/// an unbounded line set and the visible plain ends where a GPU fade
/// would have zeroed it.
const MAX_RADIUS_PER_STEP: f32 = 250.0;

/// Safety valve on the step ladder (radius so huge that the ladder would
/// otherwise run forever); real windows never get near it.
const LADDER_GUARD: usize = 1024;

/// The origin rows of a uniform grid's strip set (004 revision
/// 2026-09-05): the unique horizontal strip at y=0 (the world X axis row,
/// red in the viewport) and the unique vertical strip at x=0 (the Y axis
/// column, green). `None` when the strips contain no such row — exactly
/// the cases where the grid window does not cover the origin rows.
///
/// The caller must treat the result as *part of the strips*: it contains
/// segments verbatim from `strips`, so the colored rows can never diverge
/// from the grid's extent, ladder step or lifetime (the owner ruling:
/// "this line is a grid line, just colored").
pub fn origin_rows(strips: &[[Vec3; 2]]) -> [Option<[Vec3; 2]>; 2] {
    let x_row = strips
        .iter()
        .find(|s| s[0].y == 0.0 && s[1].y == 0.0)
        .copied();
    let y_col = strips
        .iter()
        .find(|s| s[0].x == 0.0 && s[1].x == 0.0)
        .copied();
    [x_row, y_col]
}

/// Options of a [`GridView`]: the uniform grid step and the visible
/// window half-extent. Invalid configurations (see [`grid_strips`]) yield an
/// empty grid instead of panicking.
///
/// The grid is **uniform** — one step over the whole window, never
/// concentric LOD rings (004 A/M: mature tools draw the ground as one
/// coherent grid; only the *whole* step changes with camera height).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridOptions {
    /// Base step of the uniform grid (spec §6 default 1 m) — the finest
    /// step the ladder ever uses. The step actually drawn is the smallest
    /// ladder value `≥ base` whose on-screen spacing at the target plane
    /// stays readable, so the whole grid switches together on a **zoom**,
    /// never because the orientation changed.
    pub step: f32,
    /// Half extent of the square generation window around the camera
    /// footprint (spec §6 default ±100 m; it expands and contracts with
    /// the camera zoom), clamped to `MAX_RADIUS_PER_STEP·step` inside
    /// [`grid_strips`] — the no-alpha fade cutoff.
    pub radius: f32,
    /// Pixels per world-meter at the camera-target plane — the zoom
    /// metric driving the step ladder. It depends on the eye-to-target
    /// distance and the viewport only, never on pitch or yaw.
    pub px_per_m: f32,
}

impl GridOptions {
    /// The half extent the grid actually generates for these options:
    /// the visible-ground `radius` clamped to the 250·step fade bound —
    /// the single source both [`grid_strips`] and the app's grid-bound
    /// overlays (the origin axis-color rows, viewport.rs) clip against,
    /// so no overlay can outlive the grid (005 revision 2026-09-05: the
    /// axis lines had used the raw window radius and floated past the
    /// grid while zoomed out).
    pub fn half_extent(&self) -> f32 {
        let step = uniform_step(self.step, self.px_per_m);
        (self.radius).min(MAX_RADIUS_PER_STEP * step)
    }

    /// Builds options from the raw values (no validation — invalid values
    /// are handled by [`grid_strips`], and a non-positive or non-finite
    /// `px_per_m` degrades to the base step).
    pub fn new(step: f32, radius: f32, px_per_m: f32) -> Self {
        Self {
            step,
            radius,
            px_per_m,
        }
    }
}

impl Default for GridOptions {
    fn default() -> Self {
        Self::new(DEFAULT_STEP, DEFAULT_RADIUS, DEFAULT_PX_PER_M)
    }
}

/// A ground-grid view: the camera footprint and the [`GridOptions`].
///
/// `center.z` is ignored — the footprint is the `x`, `y` projection, so
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

/// The next step of the uniform 1-2-5 ladder (`1 → 2 → 5 → 10 → 20 → …`).
/// Pure-f64 mantissa math: valid at any magnitude (no i64 truncation,
/// which would stall the ladder for steps beyond the i64 lattice).
fn next_step(step: f64) -> f64 {
    if step < 1.0 {
        return 1.0;
    }
    let k = step.log10().floor();
    let p = 10.0_f64.powi(k as i32);
    let m = step / p; // mantissa in [1, 10)
    let head = if m < 2.0 {
        2.0
    } else if m < 5.0 {
        5.0
    } else {
        10.0
    };
    let next = head * p;
    // powi is inexact at huge scales; a candidate that failed to grow
    // would stall (or stall the selection loop) — fall back to doubling.
    if next > step { next } else { step * 2.0 }
}

/// Floors `value` to a multiple of `step` measured from the world origin
/// (the lattice alignment of grid lines) — exported for the same
/// alignment elsewhere. Non-finite inputs propagate (NaN in, NaN out),
/// never panic (see `snap_floor_never_panics_on_non_finite_inputs`).
pub fn snap_floor(value: f32, step: f32) -> f32 {
    (value / step).floor() * step
}

/// The uniform step of the whole grid: the smallest ladder value (from
/// the 1-2-5 sequence starting at `base`) whose on-screen spacing at the
/// camera-target plane stays readable — `step · px_per_m ≥
/// MIN_GRID_SCREEN_SPACING_PX` — one step everywhere, one event on a
/// zoom. A non-positive or non-finite `px_per_m` degrades to the base
/// step (no density signal); `base` must be positive; a ladder stop after
/// [`LADDER_GUARD`] steps returns the current value (a dust-thin pixel
/// metric cannot exhaust the ladder before f32 precision does).
pub(crate) fn uniform_step(base: f32, px_per_m: f32) -> f32 {
    let mut step = if base.is_finite() && base > 0.0 {
        base as f64
    } else {
        1.0
    };
    let px = if px_per_m.is_finite() && px_per_m > 0.0 {
        px_per_m as f64
    } else {
        0.0
    };
    if px <= 0.0 {
        // No density signal: degrade to the base step, never panic.
        return step as f32;
    }
    for _ in 0..LADDER_GUARD {
        if step * px >= MIN_GRID_SCREEN_SPACING_PX as f64 {
            break;
        }
        step = next_step(step);
    }
    step as f32
}

/// Generates the uniform grid: lines every visible `step` meters in both
/// directions, covering the square `center ± radius` at Z=0.
///
/// Returns an empty vector when the view cannot describe a grid:
/// `center` not finite in `x` or `y` (the `z` footprint component is
/// ignored, so a non-finite `z` does not invalidate the plane);
/// `radius` not positive or not finite; `step` not positive or not finite.
pub fn grid_strips(view: &GridView) -> Vec<[Vec3; 2]> {
    let opts = &view.options;
    if !view.center.truncate().is_finite()
        || !opts.radius.is_finite()
        || opts.radius <= 0.0
        || !opts.step.is_finite()
        || opts.step <= 0.0
    {
        return Vec::new();
    }
    let step = uniform_step(opts.step, opts.px_per_m);
    if !step.is_finite() || step <= 0.0 {
        return Vec::new();
    }
    let center = view.center.truncate();
    // Visible-ground window, clamped to the no-alpha fade cutoff (see
    // module docs): the farthest line is ≤ 250 steps away.
    let half = opts.half_extent() as f64;
    let step = step as f64;
    // Lattice-index run over the window: every k·step with |k·step − c| ≤
    // half is a whole line (a f32-rounded lattice line can sit exactly on
    // the edge even when the raw division says otherwise — widen by one
    // and filter below).
    let k_lo = ((center.x as f64 - half) / step).floor() as i64 - 1;
    let k_hi = ((center.x as f64 + half) / step).ceil() as i64 + 1;
    let perp_lo_y = center.y as f64 - half;
    let perp_hi_y = center.y as f64 + half;
    let perp_lo_x = center.x as f64 - half;
    let perp_hi_x = center.x as f64 + half;

    const K_LIMIT: i64 = 9_000_000_000_000_000_000; // i64 guard (f32 lattice dies long before)
    let mut out: Vec<[Vec3; 2]> = Vec::with_capacity(segment_capacity_bound(opts));

    // Vertical lines (fixed x), then horizontal lines (fixed y), ascending.
    // Each line spans the full window along the other axis: one coherent
    // grid plane, no rings.
    if k_lo > -K_LIMIT && k_hi < K_LIMIT {
        for k in k_lo..=k_hi {
            let coord = (k as f64 * step) as f32 as f64;
            let u = (coord - center.x as f64).abs();
            if u > half {
                continue;
            }
            out.push([
                Vec3::new(coord as f32, perp_lo_y as f32, 0.0),
                Vec3::new(coord as f32, perp_hi_y as f32, 0.0),
            ]);
        }
        let k_lo_y = ((center.y as f64 - half) / step).floor() as i64 - 1;
        let k_hi_y = ((center.y as f64 + half) / step).ceil() as i64 + 1;
        if k_lo_y > -K_LIMIT && k_hi_y < K_LIMIT {
            for k in k_lo_y..=k_hi_y {
                let coord = (k as f64 * step) as f32 as f64;
                let u = (coord - center.y as f64).abs();
                if u > half {
                    continue;
                }
                out.push([
                    Vec3::new(perp_lo_x as f32, coord as f32, 0.0),
                    Vec3::new(perp_hi_x as f32, coord as f32, 0.0),
                ]);
            }
        }
    }
    out
}

/// Upper bound on the number of segments [`grid_strips`] returns for any
/// [`GridView`] carrying the same options (any center; `radius` up to the
/// given value). The window clamp (250·step, see [`grid_strips`])) bounds
/// each axis at `2·250 + 1` lines regardless of the visible ground size,
/// so the total is a fixed small number for every configuration: one
/// prebuild of this size covers any window up to the options radius (the
/// grid module's capacity-guarantee contract, viewport.rs relies on it).
pub fn segment_capacity_bound(options: &GridOptions) -> usize {
    if !options.radius.is_finite() || options.radius <= 0.0 {
        return 1;
    }
    // Worst case per axis: 2·floor(radius/step) + 1 lines ≤ 2·250 + 1,
    // plus the ±1 index widening of the window run and edge rounding.
    let per_axis = 2 * (MAX_RADIUS_PER_STEP as usize) + 5;
    2 * per_axis + 8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Options with the default step, the given window radius and the
    /// reference density (2 px/m — the ladder gate, so the step stays 1 m).
    fn opts(radius: f32) -> GridOptions {
        GridOptions::new(1.0, radius, 2.0)
    }

    /// Options with an explicit pixel density.
    fn opts_px(radius: f32, px_per_m: f32) -> GridOptions {
        GridOptions::new(1.0, radius, px_per_m)
    }

    /// Default step/radius and density, centered at `(x, y)`.
    fn view(x: f32, y: f32, radius: f32) -> GridView {
        GridView::new(Vec3::new(x, y, 0.0), opts(radius))
    }

    /// View with an explicit pixel density.
    fn view_px(x: f32, y: f32, radius: f32, px_per_m: f32) -> GridView {
        GridView::new(Vec3::new(x, y, 0.0), opts_px(radius, px_per_m))
    }

    /// Segments with fixed `x` (lines running along Y), as (x, lo, hi).
    fn verticals(strips: &[[Vec3; 2]]) -> Vec<(f32, f32, f32)> {
        strips
            .iter()
            .filter(|s| s[0].x.to_bits() == s[1].x.to_bits() && s[0].y < s[1].y)
            .map(|s| (s[0].x, s[0].y, s[1].y))
            .collect()
    }

    /// Segments with fixed `y` (lines running along X), as (lo, hi, y).
    fn horizontals(strips: &[[Vec3; 2]]) -> Vec<(f32, f32, f32)> {
        strips
            .iter()
            .filter(|s| s[0].y.to_bits() == s[1].y.to_bits() && s[0].x < s[1].x)
            .map(|s| (s[0].x, s[1].x, s[0].y))
            .collect()
    }

    /// Sorted distinct fixed coordinates of the vertical lines (NaN-free
    /// by construction, so `total_cmp` order equals value order and works
    /// across signs).
    fn vertical_x_coords(strips: &[[Vec3; 2]]) -> Vec<f32> {
        let mut xs: Vec<f32> = verticals(strips).into_iter().map(|(x, _, _)| x).collect();
        xs.sort_by(f32::total_cmp);
        xs.dedup_by(|a, b| a.to_bits() == b.to_bits());
        xs
    }

    /// Sorted distinct fixed coordinates of the horizontal lines.
    fn horizontal_y_coords(strips: &[[Vec3; 2]]) -> Vec<f32> {
        let mut ys: Vec<f32> = horizontals(strips).into_iter().map(|(_, _, y)| y).collect();
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
    fn uniform_step_picks_the_first_ladder_value_at_the_density_gate() {
        // The whole-grid ladder gate: step·px_per_m ≥ 2 px.
        let cases = [
            (1.0, 3.0, 1.0),
            (1.0, 2.0, 1.0),
            (1.0, 1.9, 2.0),
            (1.0, 0.51, 5.0), // 2·0.51 = 1.02 < 2 → 5
            (1.0, 0.2, 10.0), // 5·0.2 = 1.0 < 2 → 10
            (1.0, 0.09, 50.0),
            (0.5, 10.0, 0.5), // finest = base
            (0.5, 3.0, 1.0),  // 0.5·3 = 1.5 < 2
        ];
        for (base, px, expected) in cases {
            assert_eq!(uniform_step(base, px), expected, "base {base} px {px}");
        }
        // A dust-thin density keeps climbing (no stall, no wrap-around).
        let huge = uniform_step(1.0, 3.0e-37);
        assert!(huge.is_finite() && huge * 3.0e-37 >= 2.0, "huge {huge}");
        // No density signal: degrade to the base step, never panic.
        for px in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(uniform_step(1.0, px), 1.0, "px {px}");
        }
        // Non-finite/negative bases fall back to the 1 m ladder.
        assert_eq!(uniform_step(f32::NAN, 3.0), 1.0);
        assert_eq!(uniform_step(-1.0, 3.0), 1.0);
    }

    #[test]
    fn pitch_changes_never_reselect_the_step() {
        // The 2026-09-04 fix: the step is driven by the camera-target
        // plane's pixel density (zoom), NOT by the visible-ground radius
        // — so a pure rotation (which changes how much ground is visible)
        // never changes the grid size, like mature 3D tools.
        for radius in [100.0, 5000.0, 1.0e6, 4.0e7] {
            let strips = grid_strips(&view_px(0.0, 0.0, radius, 2.0));
            let xs = vertical_x_coords(&strips);
            for w in xs.windows(2) {
                assert_eq!(
                    w[1] - w[0],
                    1.0,
                    "radius {radius}: the step must not move with the window"
                );
            }
        }
    }

    #[test]
    fn half_extent_is_the_grid_regenerated_extent() {
        assert_eq!(GridOptions::new(1.0, 100.0, 2.0).half_extent(), 100.0);
        // A horizon-grazing window is clamped to 250·step (the fade bound
        // the strips and the app overlays share).
        assert_eq!(GridOptions::new(1.0, 4.0e7, 2.0).half_extent(), 250.0);
        assert_eq!(GridOptions::new(1.0, 4.0e7, 0.09).half_extent(), 12_500.0);
    }

    #[test]
    fn origin_rows_are_verbatim_parts_of_the_strips() {
        // Default window: the X row is the exact y=0 strip and the Y
        // column the exact x=0 strip of the generated grid — same
        // endpoint bits, so a colored overlay cannot diverge.
        let strips = grid_strips(&GridView::default());
        let [x_row, y_col] = origin_rows(&strips);
        assert!(x_row.is_some() && y_col.is_some());
        let x_row = x_row.unwrap();
        let y_col = y_col.unwrap();
        assert_eq!(
            x_row,
            [Vec3::new(-100.0, 0.0, 0.0), Vec3::new(100.0, 0.0, 0.0)]
        );
        assert_eq!(
            y_col,
            [Vec3::new(0.0, -100.0, 0.0), Vec3::new(0.0, 100.0, 0.0)]
        );
        // The found segments must be members of the strips set (bitwise).
        assert!(strips.iter().any(|s| *s == x_row && s[0].y == 0.0));
        assert!(strips.iter().any(|s| *s == y_col && s[0].x == 0.0));
    }

    #[test]
    fn origin_rows_follow_the_zoom_ladder_and_the_window() {
        // Coarse ladder (whole-grid 5 m step): the X row is still the
        // exact y=0 strip, spanning the grid's clipped window ±1250 m,
        // and multiple of the step.
        let strips = grid_strips(&view_px(0.0, 0.0, 600.0, 0.51));
        let [x_row, y_col] = origin_rows(&strips);
        assert!(x_row.is_some() && y_col.is_some());
        assert_eq!(x_row.unwrap()[0], Vec3::new(-600.0, 0.0, 0.0));
        assert_eq!(x_row.unwrap()[1], Vec3::new(600.0, 0.0, 0.0));
        assert_eq!(y_col.unwrap()[0], Vec3::new(0.0, -600.0, 0.0));
        assert_eq!(y_col.unwrap()[1], Vec3::new(0.0, 600.0, 0.0));
        // A window covering neither axis has no origin row at all: None,
        // no synthetic segment.
        let far = grid_strips(&view(2.0e6, 3.0e5, 100.0));
        let [x_row, y_col] = origin_rows(&far);
        assert!(x_row.is_none(), "no y=0 row in an off-axis window");
        assert!(y_col.is_none(), "no x=0 column in an off-axis window");
        // A window covering exactly one axis row keeps exactly that row:
        // the y=0 grid line still exists at x≈2e6 (a distance grid line is
        // still a grid line — colored, never synthetic).
        let off_x = grid_strips(&view(2.0e6, 0.0, 100.0));
        let [x_row, y_col] = origin_rows(&off_x);
        assert!(
            x_row.is_some(),
            "y=0 row exists wherever the grid crosses y=0"
        );
        assert!(
            y_col.is_none(),
            "no x=0 column unless the window covers x=0"
        );
    }

    #[test]
    fn far_fade_clamps_the_generation_window() {
        // No alpha on the line pipeline, so the far end of a horizon view
        // is cut at 250·step — the fade-equivalent. A ±4e7 window (a
        // grazing pitch) still yields exactly ±250 m of 1 m lines.
        let strips = grid_strips(&view_px(0.0, 0.0, 4.0e7, 2.0));
        let xs = vertical_x_coords(&strips);
        let expected: Vec<f32> = (-250..=250).map(|k| k as f32).collect();
        assert_eq!(xs, expected);
        assert_eq!(strips.len(), 2 * 501);
        // A coarse step clamps the window at the same bound (250·50 m).
        let strips = grid_strips(&view_px(0.0, 0.0, 1.0e5, 0.09));
        let xs = vertical_x_coords(&strips);
        assert_eq!(xs.len(), 2 * (12_500.0 / 50.0) as usize + 1);
        assert_eq!(*xs.first().unwrap(), -12_500.0);
        assert_eq!(*xs.last().unwrap(), 12_500.0);
    }

    #[test]
    fn default_view_emits_one_uniform_quad() {
        // Radius 100 at step 1: every integer line from −100 to 100 in
        // both directions — one uniform density, no concentric rings.
        let strips = grid_strips(&GridView::default());
        assert_eq!(strips.len(), 402);
        assert_eq!(verticals(&strips).len(), 201);
        assert_eq!(horizontals(&strips).len(), 201);
    }

    #[test]
    fn default_view_lines_sit_on_world_multiples() {
        let strips = grid_strips(&GridView::default());
        let xs = vertical_x_coords(&strips);
        // Every integer from −100 to 100, bitwise-exact, from the origin
        // lattice — including the far half (the old concentric-ring grid
        // switched to 5 m there; the uniform grid does not).
        let expected: Vec<f32> = (-100..=100).map(|k| k as f32).collect();
        assert_eq!(xs, expected);
        assert_eq!(horizontal_y_coords(&strips), expected);
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
    fn whole_grid_uses_one_step_only_at_every_zoom() {
        // The user-visible property: the visible ground is one coherent
        // grid at every zoom — every fixed coordinate is a multiple of one
        // step and consecutive coordinates are exactly one step apart.
        for (radius, px, step, count) in [
            (100.0, 2.0, 1.0, 201),
            (600.0, 0.51, 5.0, 241),
            (1300.0, 0.2, 10.0, 261),
            (1.0e5, 0.09, 50.0, 501),
        ] {
            let strips = grid_strips(&view_px(0.0, 0.0, radius, px));
            let xs = vertical_x_coords(&strips);
            assert_eq!(xs.len(), count, "radius {radius} px {px}, step {step}");
            for w in xs.windows(2) {
                assert_eq!(w[1] - w[0], step, "radius {radius}: uniform spacing");
            }
            // All coordinates are exact multiples of the single step.
            for x in &xs {
                assert_eq!(*x / step, (*x / step).round(), "radius {radius}");
            }
        }
    }

    #[test]
    fn zoom_switches_the_whole_grid_as_one_event() {
        // Zooming out (px_per_m falling) switches every line at once at
        // the gate — no mixed densities, no inner/outer zones.
        let mut prev = 0.0;
        for [px, expected] in [
            [3.0, 1.0],
            [2.0, 1.0],
            [1.9, 2.0],
            [0.51, 5.0],
            [0.2, 10.0],
            [0.09, 50.0],
        ] {
            let strips = grid_strips(&view_px(0.0, 0.0, 1000.0, px));
            let xs = vertical_x_coords(&strips);
            for w in xs.windows(2) {
                assert_eq!(w[1] - w[0], expected, "px {px}: single step");
            }
            let step = xs.windows(2).next().map(|w| w[1] - w[0]).unwrap();
            assert_eq!(step, uniform_step(1.0, px), "px {px}");
            assert!(step >= prev, "px {px}: steps never shrink");
            prev = step;
        }
    }

    #[test]
    fn moving_camera_never_crawls_fixed_lines() {
        // A11: lines are fixed world multiples — a window shift adds or
        // removes whole lines only at the border; every shared coordinate
        // stays bit-identical.
        let a = grid_strips(&view(0.0, 0.0, 10.0));
        let b = grid_strips(&view(0.25, -3.7, 10.0));
        let xa = vertical_x_coords(&a);
        let xb = vertical_x_coords(&b);
        let shared_b: Vec<f32> = xb.iter().copied().filter(|x| x.abs() <= 9.0).collect();
        let shared_a: Vec<f32> = xa.iter().copied().filter(|x| x.abs() <= 9.0).collect();
        assert_eq!(shared_a, shared_b, "interior lines must not move");
    }

    #[test]
    fn window_outer_segments_stay_inside_the_window() {
        for radius in [1.0, 3.3, 100.0, 251.0, 6250.0] {
            let strips = grid_strips(&view(2.0, -1.0, radius));
            for s in &strips {
                let x = s[0].x;
                let y = s[0].y;
                assert!(x.abs() <= radius + 2.0, "x {x} radius {radius}");
                assert!(y.abs() <= radius + 2.0, "y {y} radius {radius}");
                assert_eq!(s[0].z, 0.0);
                assert_eq!(s[0].z, s[1].z);
            }
        }
    }

    #[test]
    fn invalid_options_yield_an_empty_grid() {
        let good = Vec3::ZERO;
        for radius in [0.0, -3.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(
                grid_strips(&GridView::new(good, GridOptions::new(1.0, radius, 2.0))).is_empty(),
                "radius {radius}"
            );
        }
        for step in [0.0, -2.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(
                grid_strips(&GridView::new(good, GridOptions::new(step, 100.0, 2.0))).is_empty(),
                "step {step}"
            );
        }
        for c in 0..2 {
            let mut center = Vec3::ZERO;
            center[c] = f32::NAN;
            assert!(grid_strips(&GridView::new(center, GridOptions::default())).is_empty());
            center[c] = f32::INFINITY;
            assert!(grid_strips(&GridView::new(center, GridOptions::default())).is_empty());
        }
        // A non-finite z is ignored (documented footprint contract), so it
        // still yields the regular grid.
        let mut zombie = Vec3::ZERO;
        zombie.z = f32::NAN;
        assert!(!grid_strips(&GridView::new(zombie, GridOptions::default())).is_empty());
        // A null pixel density is NOT invalid: it degrades to the base
        // step and still emits the grid.
        for px in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(
                !grid_strips(&GridView::new(good, GridOptions::new(1.0, 10.0, px))).is_empty(),
                "px {px}"
            );
        }
    }

    #[test]
    fn astronomical_windows_stay_finite_and_panic_free() {
        // The fade clamp bounds the line set, so even a 1e12 m window
        // yields a few hundred finite lines (coerce, never overflow).
        let radius = 1.0e12;
        let strips = grid_strips(&view(0.0, 0.0, radius));
        assert!(!strips.is_empty());
        for s in &strips {
            assert!(s[0].is_finite());
            assert!(s[1].is_finite());
        }
        assert!(strips.len() <= segment_capacity_bound(&opts(radius)));
    }

    #[test]
    fn segments_never_exceed_the_declared_capacity_bound() {
        // The window clamp (250·step) bounds any pose at ~1000 lines, so
        // the prebuilt persistent mesh (viewport.rs) fits them all.
        for radius in [
            0.001, 1.0, 100.0, 260.0, 600.0, 1300.0, 1e4, 1e6, 1e9, 1e12, 3.0e37,
        ] {
            for px in [0.09, 0.51, 2.0, 10.0] {
                let options = opts_px(radius, px);
                let strips = grid_strips(&view_px(5.0, -9.0, radius, px));
                assert!(
                    strips.len() <= segment_capacity_bound(&options),
                    "radius {radius} px {px}: {} > {}",
                    strips.len(),
                    segment_capacity_bound(&options)
                );
            }
        }
        // And that bound is a fixed small number: one prebuild covers any.
        for radius in [100.0, 260.0, 600.0, 1e4, 1e12] {
            assert!(
                segment_capacity_bound(&opts(radius)) <= 1024,
                "radius {radius}"
            );
        }
    }

    #[test]
    fn default_grid_renders_uniform_density_across_the_whole_window() {
        // At the default window there is no dense center and sparse edge —
        // the border lines are one step from their neighbours, same as at
        // the origin.
        let strips = grid_strips(&GridView::default());
        let xs = vertical_x_coords(&strips);
        for w in xs.windows(2) {
            assert_eq!(w[1] - w[0], 1.0);
        }
        let ys = horizontal_y_coords(&strips);
        for w in ys.windows(2) {
            assert_eq!(w[1] - w[0], 1.0);
        }
    }
}
