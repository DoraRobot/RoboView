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
//! that stays readable on screen for the current window:
//!
//! ```text
//! radius ≤ 250·step   →   step stays as-is (≈1 m at the default ±100 m
//!                        window; on-screen spacing ≥ ~2 px)
//! radius > 250·step   →   the WHOLE grid switches to the next ladder step
//!                        (1 → 2 → 5 → 10 → 20 → 50 → 100 m)
//! ```
//!
//! So the plane reads uniformly at every camera height; climbing up just
//! coarsens the whole grid together (one discrete pop), and zooming in
//! refines it back — exactly the behaviour of Blender's floor grid.
//!
//! # Step switches are discrete "pops", never crawling (A11)
//!
//! Every line sits on a fixed world coordinate `k·step` (a multiple of the
//! step measured from the world origin — `snap_floor` is exported for the
//! same alignment elsewhere). Moving the camera therefore never moves a
//! line: it only adds or removes whole lines where they enter or leave the
//! window, and a step change happens at one radius as a discrete whole-grid
//! event. Between events, line coordinates are bit-identical — no crawl, no
//! fade, no ring seams to mind.
//!
//! # Output, cost, and limits
//!
//! Returns axis-aligned `[Vec3; 2]` segments on Z=0 (each ordered low→high
//! along its axis), all vertical lines (fixed `x`) then all horizontal
//! lines (fixed `y`), coordinates ascending, every `step` meters — the
//! uniform grid. The visible count is bounded: `radius/step ≤ 250` by step
//! selection, so each axis emits at most ~500 lines (the recorded limit is
//! ~1000 segments for any radius; [`segment_capacity_bound`] gives the
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

/// The uniform step is used only while `radius ≤ MAX_RADIUS_PER_STEP ·
/// step`, keeping its on-screen spacing above roughly 2 px (see module
/// docs). 250 ≈ 1000 px / (2·2 px) for the reference viewport. When a
/// window outgrows it, the WHOLE grid switches to the next ladder step.
const MAX_RADIUS_PER_STEP: f32 = 250.0;

/// Safety valve on the step ladder (radius so huge that the ladder would
/// otherwise run forever); real windows never get near it.
const LADDER_GUARD: usize = 1024;

/// Options of a [`GridView`]: the uniform grid step and the visible
/// window half-extent. Invalid configurations (see [`grid_strips`]) yield an
/// empty grid instead of panicking.
///
/// The grid is **uniform** — one step over the whole window, never
/// concentric LOD rings (004 A/M: mature tools draw the ground as one
/// coherent grid; only the *whole* step changes with camera height).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridOptions {
    /// Base step of the uniform grid (spec §6 default 1 m). The step used
    /// for a given window is the smallest ladder value `≥ base` that stays
    /// readable on screen (`radius ≤ MAX_RADIUS_PER_STEP·step`) — so the
    /// whole grid switches together when the camera climbs, instead of
    /// thinning in rings.
    pub step: f32,
    /// Half extent of the square generation window around the camera
    /// footprint (spec §6 default ±100 m; it expands and contracts with
    /// the camera zoom).
    pub radius: f32,
}

impl GridOptions {
    /// Builds options from the raw values (no validation — invalid values
    /// are handled by [`grid_strips`]).
    pub fn new(step: f32, radius: f32) -> Self {
        Self { step, radius }
    }
}

impl Default for GridOptions {
    fn default() -> Self {
        Self::new(DEFAULT_STEP, DEFAULT_RADIUS)
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

/// The uniform step of the whole grid for a window of `radius` meters:
/// the smallest ladder value (from the 1-2-5 sequence starting at `base`)
/// with `radius ≤ MAX_RADIUS_PER_STEP·step` — one step everywhere inside
/// the window. `base` must be positive; a ladder stop after
/// [`LADDER_GUARD`] steps returns the current value (huge windows run out
/// of f32 precision long before the guard does).
pub(crate) fn uniform_step(base: f32, radius: f32) -> f32 {
    let mut step = if base.is_finite() && base > 0.0 {
        base as f64
    } else {
        1.0
    };
    for _ in 0..LADDER_GUARD {
        if (radius as f64) <= MAX_RADIUS_PER_STEP as f64 * step {
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
    let step = uniform_step(opts.step, opts.radius);
    if !step.is_finite() || step <= 0.0 {
        return Vec::new();
    }
    let center = view.center.truncate();
    let half = opts.radius as f64;
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
/// given value). The uniform step keeps `radius/step ≤ 250` by selection
/// (see [`uniform_step`]) — for *any* radius, not just the option's — so
/// each axis emits at most `2·250 + 2·(window rounding slack) + 2` lines
/// and the total is a fixed small number for every configuration: one
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

    /// Options with the default step and the given window radius.
    fn opts(radius: f32) -> GridOptions {
        GridOptions::new(1.0, radius)
    }

    /// Default step and radius, centered at `(x, y)`.
    fn view(x: f32, y: f32, radius: f32) -> GridView {
        GridView::new(Vec3::new(x, y, 0.0), opts(radius))
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
    fn uniform_step_picks_the_first_ladder_value_that_stays_readable() {
        // radius ≤ 250·step keeps the step; one ladder rung too big and
        // the WHOLE grid switches (1 → 2 → 5 → 10 → 20 → 50 → 100 → 200).
        let cases = [
            (100.0, 1.0),
            (250.0, 1.0),
            (251.0, 2.0),
            (400.0, 2.0),
            (500.0, 2.0),
            (501.0, 5.0),
            (1250.0, 5.0),
            (1251.0, 10.0),
            (12500.0, 50.0),
            (62500.0, 500.0),
        ];
        for (radius, expected) in cases {
            assert_eq!(uniform_step(1.0, radius), expected, "radius {radius}");
        }
        // A custom base below 1 m climbs onto the ladder when needed.
        assert_eq!(uniform_step(0.5, 50.0), 0.5);
        assert_eq!(uniform_step(0.5, 200.0), 1.0);
        // Non-finite/negative bases fall back to the 1 m ladder.
        assert_eq!(uniform_step(f32::NAN, 1e3), 5.0);
        assert_eq!(uniform_step(-1.0, 1e3), 5.0);
        // Huge windows keep climbing the ladder (no stall, no tiny-step
        // regression — powi rounding at 1e35 scale stays inside tolerance).
        let huge = uniform_step(1.0, 3.0e37);
        assert!((huge - 2.0e35).abs() <= 2.0e35 * 1e-4, "huge step {huge}");
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
        // The user-visible property (004 revision): the visible ground is
        // one coherent grid — no near/far density difference. Every fixed
        // coordinate is a multiple of one step and consecutive coordinates
        // are exactly one step apart, at every tested zoom.
        for (radius, step, count) in [
            (100.0, 1.0, 201),
            (251.0, 2.0, 2 * (251.0f32 / 2.0).floor() as usize + 1),
            (600.0, 5.0, 2 * (600.0f32 / 5.0).floor() as usize + 1),
            (1300.0, 10.0, 2 * (1300.0f32 / 10.0).floor() as usize + 1),
            (6250.0, 50.0, 2 * (6250.0f32 / 50.0).floor() as usize + 1),
        ] {
            let strips = grid_strips(&view(0.0, 0.0, radius));
            let xs = vertical_x_coords(&strips);
            assert_eq!(xs.len(), count, "radius {radius}, step {step}");
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
        // Climbing the camera switches every line at once at the gate
        // radius 250·step — no mixed densities, no inner/outer zones.
        let step_of = |radius: f32, strips: &[[Vec3; 2]]| {
            let xs = vertical_x_coords(strips);
            let mut s = None;
            for w in xs.windows(2) {
                let d = w[1] - w[0];
                assert!(d <= 1.5 * s.unwrap_or(d), "radius {radius}: single step");
                s = Some(d);
            }
            s.expect("non-empty grid")
        };
        let mut prev = 0.0;
        for radius in [
            100.0, 250.0, 251.0, 500.0, 501.0, 1250.0, 1251.0, 6250.0, 6251.0,
        ] {
            let strips = grid_strips(&view(0.0, 0.0, radius));
            let step = step_of(radius, &strips);
            assert_eq!(step, uniform_step(1.0, radius), "radius {radius}");
            assert!(step >= prev, "radius {radius}: steps never shrink");
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
                grid_strips(&GridView::new(good, GridOptions::new(1.0, radius))).is_empty(),
                "radius {radius}"
            );
        }
        for step in [0.0, -2.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(
                grid_strips(&GridView::new(good, GridOptions::new(step, 100.0))).is_empty(),
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
    }

    #[test]
    fn astronomical_windows_stay_finite_and_panic_free() {
        // Ladder gates the step, so even a 1e12 m window yields ~a few
        // hundred finite lines (coerce, never overflow).
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
        // The uniform step keeps radius/step ≤ 250, so any window fits a
        // few-hundred-line — the prebuilt persistent mesh (viewport.rs)
        // relies on this bound.
        for radius in [
            0.001, 1.0, 100.0, 260.0, 600.0, 1300.0, 1e4, 1e6, 1e9, 1e12, 3.0e37,
        ] {
            let options = opts(radius);
            let strips = grid_strips(&view(5.0, -9.0, radius));
            assert!(
                strips.len() <= segment_capacity_bound(&options),
                "radius {radius}: {} > {}",
                strips.len(),
                segment_capacity_bound(&options)
            );
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
        // Directly the property the 004 revision adds: at the default
        // window there is no dense center and sparse edge — the border
        // lines are one step from their neighbours, same as at the origin.
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
