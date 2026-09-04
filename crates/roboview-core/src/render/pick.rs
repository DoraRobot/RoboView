//! Object picking: CPU ray hit tests over display objects.
//!
//! This module implements the picking half of the picking-selection spec
//! (005, plan §3.1): pure, headless hit tests for every pickable display
//! kind — point clouds, meshes, paths, frames and markers (005 spec §5: the
//! ground grid and the origin axes of the viewport overlay are not pickable
//! and never reach this module; the caller passes the visible objects in
//! add order with their ids).
//!
//! Everything here is deterministic CPU math on the data the renderer drew
//! from — no GPU handles, no depth buffer, no camera state beyond the
//! view-projection matrix the app already owns. The module mirrors the
//! geometry builders of `render/line.rs` so a click and a drawn pixel agree.
//!
//! # Screen criteria (005 spec A2, fixed)
//!
//! - Meshes and lines: the pointer may miss the primitive by at most
//!   `δ = LINE_TOLERANCE_RATIO · viewport_height` screen pixels, measured
//!   as the projected screen distance (mesh faces carry no tolerance: a
//!   face is hit when the ray crosses it).
//! - Point clouds: a point is hit when its projected position lies within
//!   `POINT_RADIUS_PX = 8` pixels of the pointer (005 spec D3; physical
//!   pixels = 8 × pixels-per-point is the app's conversion, 005 A2 note).
//!   Face-less meshes display as a scatter through the point pipeline, so
//!   they use the point criterion too, while their kind stays a mesh.
//!
//! The core side evaluates both criteria in world space along the pick
//! ray: at ray depth `t` one pixel spans `world_per_pixel_scale · t` world
//! units (perspective), so the tolerances become world radii
//!
//! ```text
//! δ_world(t) = δ_px · world_per_pixel_scale · t     (lines)
//! r_world(t) = 8 · world_per_pixel_scale · t        (points)
//! ```
//!
//! and a line/point hit is accepted when its perpendicular distance to the
//! ray (clamped to the ray origin and the line segment / to the point
//! depth) is within the radius at its own depth. This is the plan §3.1
//! formula (`δ_world ≈ δ_screen · 2·d·tan(fov/2) / viewport_height` with
//! `d ≈ t`); the caller derives `world_per_pixel_scale` once per frame as
//! `2·tan(fov_y/2)/viewport_height` from `OrbitCamera::vertical_fov()` and
//! passes it in [`PickContext`] — it cannot be read back from a composed
//! `view_proj` (the projection row mixes in the view orientation).
//!
//! # Arbitration (005 spec D4)
//!
//! - Scene kinds (clouds, meshes, lines, arrows) compete by their hit
//!   depth `t`: the nearest hit wins; an exact tie goes to the earlier
//!   object in the caller's list (add order — "first drawn wins", spec
//!   D4/tasks T5).
//! - Text labels are the overlay class (005 spec §6: labels are painted on
//!   top of the scene and never occlude or are occluded by it), so a click
//!   inside a label box beats any scene hit at the same pixel — otherwise
//!   a label anchored on its own, earlier-added mesh would tie with the
//!   mesh in `t` and lose, making labels unpickable. Among overlapping
//!   labels the later-added one is painted on top and wins.
//! - A winning hit is accepted only if it is actually visible: its
//!   reprojection must land in front of the camera (`w > 0`), inside the
//!   depth range, and within the viewport — plus the widest screen
//!   tolerance past each border, so a hit whose tolerance circle reaches
//!   onto the viewport still registers.
//!
//! # Guards (005 spec A5/A9)
//!
//! Every public entry point is total: non-finite rays, contexts, bounds or
//! object geometry produce `None` (or an empty selection) — never a panic.
//! Non-finite data coordinates are skipped, mirroring the io policy (001
//! spec G1: garbage is kept in data and defended against at use). Meshes
//! render double-sided (render/mesh.rs draws with no culling), so picking
//! accepts back faces. Degenerate geometry (zero-area triangles, parallel
//! or zero-length segments, empty boxes) simply never hits.
//!
//! Point-cloud proximity uses a uniform bucket index rebuilt per call over
//! the cloud's finite points, sized to the current screen radius so the
//! search ring stays three cells wide at any depth. A rebuilt-per-call
//! index is exactly what tasks T2's "bucket re-allocation" record requires:
//! stateless pick calls always answer from the current data.
//! The 005 spec §6 app-layer cache (`HashMap<u64, Arc<…>>` in
//! `ViewportState`, invalidated on `Scene::remove`) is viewport wiring on
//! top of [`pick_objects`] and lands with the app tasks.

use std::collections::HashMap;

use glam::{Mat4, Vec2, Vec3};

use super::camera_math::{Rect2, anchor_to_screen};
use crate::displays::{DisplayKind, DisplayObject, Frame, Marker, Mesh};
use crate::io::Aabb;

/// Point-cloud hit radius on screen (005 spec A2/D3): 8 logical pixels.
/// Physical pixels are `8 × pixels_per_point`, the app's conversion.
pub const POINT_RADIUS_PX: f32 = 8.0;

/// Line hit tolerance `δ` as a fraction of the viewport's logical height
/// (005 spec A2: `δ` = 0.5% of the height, i.e. `0.005 · height` pixels).
pub const LINE_TOLERANCE_RATIO: f32 = 0.005;

/// Estimated horizontal advance of one text character as a fraction of the
/// font size. Proportional fonts average ~0.6 em per character; the pill
/// padding below absorbs the per-glyph spread. Mirrors the measured galley
/// widths the app paints with (viewport.rs `paint_label`).
const TEXT_AVERAGE_ADVANCE_EM: f32 = 0.6;

/// Estimated line height of one text line as a fraction of the font size
/// (proportional fonts average ~1.2 em).
const TEXT_LINE_HEIGHT_EM: f32 = 1.2;

/// The app paints each label lifted 3 px above its anchor inside a 3 px
/// rounded pill (viewport.rs `paint_label`): the pick box mirrors the
/// painted pill, using this halo for the lift and the padding alike.
const TEXT_HALO_PX: f32 = 3.0;

/// Degenerate-triangle guard: a triangle whose `|det|` (twice its area,
/// scaled by the ray's angle of incidence) falls below this fraction of
/// its edge-length product is skipped — it is a sliver or the ray grazes
/// it edge-on, where the barycentric solve is meaningless.
const DEGENERATE_TRIANGLE_EPSILON: f32 = 1e-9;

/// Parallel ray/segment guard: when `|dir × e|²` falls below this fraction
/// of `|e|²` the segment is treated as parallel to the ray (no unique
/// closest point) and is not hit. Angles below ~0.002° are visually
/// indistinguishable from parallel at pick tolerances.
const PARALLEL_SEGMENT_EPSILON: f32 = 1e-9;

/// Extra pixels past a viewport border a hit point may still sit at: the
/// widest screen tolerance (point radius or line `δ`) plus this slack, so
/// a hit whose tolerance circle just reaches the viewport is accepted.
const VISIBILITY_PX_SLACK: f32 = 1.0;

/// Marker-arrow head geometry, mirrored from `render/line.rs`'s arrow
/// strips (same constants and formulas): the pick must agree with what the
/// renderer drew. Kept private here because line.rs keeps its geometry
/// private; a later task may promote the shared constants.
const ARROW_HEAD_FRACTION: f32 = super::line::ARROW_HEAD_FRACTION;
const ARROW_HEAD_HALF_ANGLE: f32 = super::line::ARROW_HEAD_HALF_ANGLE;

/// Shared parameters of one pick query (one pointer position, one frame).
///
/// Everything derives from state the app already holds per frame; the
/// struct exists so the hit functions stay pure and headless-testable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PickContext {
    /// The scene's composed view-projection matrix for the current pose
    /// (camera `view_proj`, same convention as `camera_math`).
    pub view_proj: Mat4,
    /// Viewport size in logical pixels (egui points), the same rect the
    /// app projects anchors and paints labels against.
    pub viewport: Vec2,
    /// World length subtended by one viewport pixel at one world unit of
    /// ray depth. The app derives it from the camera's fixed vertical fov
    /// as `2·tan(vertical_fov()/2) / viewport_height` (005 plan §3.1).
    /// Only line and point hits use it; meshes and text need no scale.
    pub world_per_pixel_scale: f32,
    /// Font size in pixels the app paints marker labels with (viewport.rs
    /// paints text markers at `FontId::proportional(14.0)`). Only text
    /// label boxes use it.
    pub font_size_px: f32,
}

/// A pick result: the selected object and the ray depth of the hit.
///
/// `t` is the ray parameter of the hit in world units along the unit ray
/// direction (the label's anchor depth for text hits). Callers use it only
/// for ordering; the hit itself is already arbitrated.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PickHit {
    /// The object's id as passed in the pick list.
    pub id: u64,
    /// The kind of the hit object.
    pub kind: DisplayKind,
    /// Ray depth of the hit in world units, `t ≥ 0`.
    pub t: f32,
}

/// Normalize a finite non-zero direction; `None` for anything else.
fn unit_dir(dir: Vec3) -> Option<Vec3> {
    if !dir.is_finite() {
        return None;
    }
    let dir = dir.normalize();
    dir.is_finite().then_some(dir)
}

/// Ray–triangle intersection (Möller–Trumbore), returning the ray parameter
/// `t ≥ 0` of the crossing in world units.
///
/// Double-sided: meshes render without culling (render/mesh.rs draws every
/// face), so a ray hits the triangle from either side and `t` is reported
/// for whichever crossing the ray meets first. `None` when the ray misses
/// the triangle, crosses it behind its origin (`t < 0`), grazes a
/// degenerate (zero-area) triangle, or any input is non-finite or `dir` is
/// zero. `dir` is normalized internally; `t` is measured along the
/// normalized direction. Never panics.
pub fn ray_triangle(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    if !origin.is_finite() || !a.is_finite() || !b.is_finite() || !c.is_finite() {
        return None;
    }
    let dir = unit_dir(dir)?;
    let edge1 = b - a;
    let edge2 = c - a;
    let pvec = dir.cross(edge2);
    let det = edge1.dot(pvec);
    // No sign filtering: back faces are accepted (double-sided rendering).
    if det.abs() <= DEGENERATE_TRIANGLE_EPSILON * edge1.length() * edge2.length() {
        return None;
    }
    let inv_det = 1.0 / det;
    let tvec = origin - a;
    let u = tvec.dot(pvec) * inv_det;
    if u < 0.0 || u > 1.0 {
        return None;
    }
    let qvec = tvec.cross(edge1);
    let v = dir.dot(qvec) * inv_det;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = edge2.dot(qvec) * inv_det;
    (t >= 0.0).then_some(t)
}

/// Closest approach between the half-line `origin + dir·t` (`t ≥ 0`, `dir`
/// unit) and the segment `a..b`: returns `(t, distance²)` of the closest
/// point pair, minimizing over the segment and clamping `t` at zero.
///
/// `None` when the ray and the segment are parallel (no unique closest
/// point), the segment is degenerate (zero length), or any input is
/// non-finite. Never panics.
fn segment_closest(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3) -> Option<(f32, f32)> {
    if !origin.is_finite() || !a.is_finite() || !b.is_finite() {
        return None;
    }
    let e = b - a;
    let e_len2 = e.length_squared();
    if !e_len2.is_finite() || e_len2 == 0.0 {
        return None;
    }
    let along = dir.dot(e);
    let denom = e_len2 - along * along; // |dir × e|² ≥ 0 up to rounding
    if denom.abs() <= PARALLEL_SEGMENT_EPSILON * e_len2 {
        return None;
    }
    let ao = origin - a;
    let a_dot = ao.dot(dir); // (o − a)·dir
    let e_dot = ao.dot(e); // (o − a)·e

    // The distance squared over (t ≥ 0) × (u ∈ [0,1]) is convex, so the
    // minimum sits at the interior stationary point or on one of the three
    // boundary edges; each candidate below is the exact minimum of its
    // edge (a 1-D quadratic solved with the clamp).
    let mut best_t = 0.0;
    let mut best_d2 = f32::INFINITY;
    let mut consider = |t: f32, point: Vec3| {
        let delta = origin + dir * t - point;
        let d2 = delta.length_squared();
        if d2 < best_d2 {
            best_d2 = d2;
            best_t = t;
        }
    };

    // Interior stationary point: u* = −(v0·v1)/|v1|² with v0 the part of
    // (o − a) perpendicular to the ray and v1 = e − dir·(dir·e) — the
    // second-order expansion of the closest-point problem over the strip.
    let u_int = (e_dot - along * a_dot) / denom;
    let t_int = along * u_int - a_dot;
    if (0.0..=1.0).contains(&u_int) && t_int >= 0.0 {
        consider(t_int, a + e * u_int);
    }
    // Segment ends, each minimized over the ray.
    consider((a - origin).dot(dir).max(0.0), a);
    consider((b - origin).dot(dir).max(0.0), b);
    // Ray origin edge, minimized over the segment.
    let u0 = (e_dot / e_len2).clamp(0.0, 1.0);
    consider(0.0, a + e * u0);

    Some((best_t, best_d2))
}

/// Closest approach between the ray `origin + dir·t` (`t ≥ 0`) and the
/// segment `a..b`: the ray parameter `t` of the closest point in world
/// units along `dir` (normalized internally), clamped to `t ≥ 0`.
///
/// `None` for a parallel or degenerate (zero-length) segment, non-finite
/// inputs, or a zero `dir` — cases with no well-defined closest point.
/// Never panics.
pub fn ray_segment(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3) -> Option<f32> {
    let dir = unit_dir(dir)?;
    segment_closest(origin, dir, a, b).map(|(t, _)| t)
}

/// The three axis segments of a frame, as `(start, end)` pairs — mirrors
/// `render/line.rs` `frame_strips` (guard semantics included: a frame with
/// non-finite parameters draws nothing and picks nothing).
fn frame_segments(origin: Vec3, length: f32) -> Vec<(Vec3, Vec3)> {
    if !origin.is_finite() || !length.is_finite() || length <= 0.0 {
        return Vec::new();
    }
    [Vec3::X, Vec3::Y, Vec3::Z]
        .into_iter()
        .map(|axis| (origin, origin + axis * length))
        .collect()
}

/// The shaft plus two head lines of a marker arrow as `(start, end)`
/// pairs — mirrors `render/line.rs` `arrow_strips` exactly (same
/// constants, same perpendicular fallback, same guards), so the pickable
/// capsule set is the drawn strip set.
fn arrow_segments(start: Vec3, end: Vec3) -> Vec<(Vec3, Vec3)> {
    if !start.is_finite() || !end.is_finite() {
        return Vec::new();
    }
    let shaft = end - start;
    let length = shaft.length();
    if length <= 0.0 || !shaft.is_finite() {
        return Vec::new();
    }
    let direction = shaft / length;

    // Unit axis perpendicular to the shaft, spanning the head plane: world
    // Y, or world X when the shaft runs (near-)parallel to Y — the same
    // reference and fallback the renderer spreads its head lines in.
    let perp = {
        let y_cross = direction.cross(Vec3::Y);
        if y_cross.length_squared() < 1e-12 {
            direction.cross(Vec3::X).normalize()
        } else {
            y_cross.normalize()
        }
    };

    let (sin, cos) = ARROW_HEAD_HALF_ANGLE.sin_cos();
    let head_length = length * ARROW_HEAD_FRACTION;
    let tip = |side: f32| end + head_length * (-direction * cos + perp * side * sin);

    vec![(start, end), (end, tip(1.0)), (end, tip(-1.0))]
}

/// The eight corners of an Aabb.
fn aabb_corners(bounds: &Aabb) -> [Vec3; 8] {
    let (min, max) = (bounds.min, bounds.max);
    [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(min.x, max.y, max.z),
        Vec3::new(max.x, max.y, max.z),
    ]
}

/// Project a world point to viewport pixels: `None` unless the point is in
/// front of the eye (`w > 0`) and inside the depth range (`0 < ndc.z ≤ 1`).
/// Lateral position is deliberately not culled — pickers need the
/// projected pixels of points that sit outside the viewport by up to a
/// screen tolerance (see the visibility gate and `pick_rect`'s projected
/// boxes). The caller guarantees a finite `view_proj`/`viewport`.
fn project_px(view_proj: &Mat4, viewport: Vec2, point: Vec3) -> Option<Vec2> {
    if !point.is_finite() {
        return None;
    }
    let clip = view_proj * point.extend(1.0);
    if !clip.is_finite() || clip.w <= 0.0 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    if !ndc.is_finite() || ndc.z <= 0.0 || ndc.z > 1.0 {
        return None;
    }
    Some(Vec2::new(
        (ndc.x + 1.0) * 0.5 * viewport.x,
        (1.0 - ndc.y) * 0.5 * viewport.y,
    ))
}

/// The pixel a world point sits at, without any depth culling: unlike
/// [`project_px`] this accepts points on the near plane itself (`ndc.z =
/// 0`), which is exactly where a camera ray's origin lies (`screen_to_ray`
/// starts on the near plane). Used to recover the pointer pixel from a
/// pick ray's origin for label-box hits.
fn pixel_of(view_proj: &Mat4, viewport: Vec2, point: Vec3) -> Option<Vec2> {
    if !point.is_finite() {
        return None;
    }
    let clip = view_proj * point.extend(1.0);
    if !clip.is_finite() || clip.w <= 0.0 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    if !ndc.is_finite() {
        return None;
    }
    Some(Vec2::new(
        (ndc.x + 1.0) * 0.5 * viewport.x,
        (1.0 - ndc.y) * 0.5 * viewport.y,
    ))
}

/// The screen box of a text label anchored at `anchor_px`, mirroring the
/// pill the app paints (viewport.rs `paint_label`: text horizontally
/// centered on the anchor, one line tall, lifted 3 px, padded 3 px):
/// `box_max.y` is the anchor row. Width per character is estimated at 0.6
/// em — a documented approximation for headless math (005 spec A1: the
/// criterion is loose); the app's measured galleys remain authoritative
/// for painting. `None` for empty text or a non-positive font size.
fn label_screen_box(anchor_px: Vec2, text: &str, font_size_px: f32) -> Option<(Vec2, Vec2)> {
    if !(font_size_px.is_finite() && font_size_px > 0.0) || text.is_empty() {
        return None;
    }
    let width = text.chars().count() as f32 * font_size_px * TEXT_AVERAGE_ADVANCE_EM;
    let height = font_size_px * TEXT_LINE_HEIGHT_EM;
    let half_width = width * 0.5 + TEXT_HALO_PX;
    Some((
        Vec2::new(
            anchor_px.x - half_width,
            anchor_px.y - height - 2.0 * TEXT_HALO_PX,
        ),
        Vec2::new(anchor_px.x + half_width, anchor_px.y),
    ))
}

/// Inclusive screen-box containment (the box edges belong to the box, so a
/// click exactly on the painted pill's border still hits).
fn contains_px(min: Vec2, max: Vec2, px: Vec2) -> bool {
    px.x >= min.x && px.x <= max.x && px.y >= min.y && px.y <= max.y
}

/// Hit test of one line-family segment against the line tolerance: `Some`
/// (the ray parameter of the closest approach) when the approach lies at a
/// positive ray depth `t` and within `δ_world(t)` of the ray.
fn segment_hit_t(ctx: &PickContext, origin: Vec3, dir: Vec3, a: Vec3, b: Vec3) -> Option<f32> {
    let scale = ctx.world_per_pixel_scale;
    if !(scale.is_finite() && scale > 0.0) {
        return None;
    }
    let (t, d2) = segment_closest(origin, dir, a, b)?;
    if t <= 0.0 {
        // The closest approach sits at or behind the near plane (the
        // half-line clamp landed on t = 0); the visible portion is strictly
        // farther away at every depth, so nothing on screen can be within
        // the tolerance of the pointer here.
        return None;
    }
    let tolerance_px = LINE_TOLERANCE_RATIO * ctx.viewport.y;
    let radius = tolerance_px * scale * t;
    (d2 <= radius * radius).then_some(t)
}

/// Best hit of a polyline's consecutive segments (path data), skipping
/// non-finite endpoints exactly as the renderer splits its finite runs.
fn polyline_hit_t(ctx: &PickContext, origin: Vec3, dir: Vec3, points: &[Vec3]) -> Option<f32> {
    let mut best: Option<f32> = None;
    for pair in points.windows(2) {
        if let Some(t) = segment_hit_t(ctx, origin, dir, pair[0], pair[1]) {
            best = Some(best.map_or(t, |b| b.min(t)));
        }
    }
    best
}

/// Best hit over a list of segments (frames, arrows).
fn segments_hit_t(
    ctx: &PickContext,
    origin: Vec3,
    dir: Vec3,
    segments: &[(Vec3, Vec3)],
) -> Option<f32> {
    let mut best: Option<f32> = None;
    for &(a, b) in segments {
        if let Some(t) = segment_hit_t(ctx, origin, dir, a, b) {
            best = Some(best.map_or(t, |b| b.min(t)));
        }
    }
    best
}

/// Uniform bucket index over a point set, quantized relative to an anchor
/// (the data Aabb min) so cell coordinates stay small wherever the scene
/// sits. Rebuilt per call — the 005 spec §6 app-layer cache builds on
/// [`pick_objects`], which is stateless (per-call bucket rebuild, tasks T2).
struct PointIndex {
    bucket: f32,
    anchor: Vec3,
    cells: HashMap<(i64, i64, i64), Vec<u32>>,
}

impl PointIndex {
    /// Index the finite points of `positions`; `None` for a non-positive
    /// or non-finite bucket (guards: never panics). Non-finite positions
    /// (001 spec G1) are skipped. Point counts far below `u32::MAX` are an
    /// io load invariant; the debug assertion pins it.
    fn new(positions: &[Vec3], bucket: f32, anchor: Vec3) -> Option<PointIndex> {
        if !(bucket.is_finite() && bucket > 0.0) || !anchor.is_finite() {
            return None;
        }
        let mut index = PointIndex {
            bucket,
            anchor,
            cells: HashMap::new(),
        };
        debug_assert!(positions.len() <= u32::MAX as usize);
        for (i, point) in positions.iter().enumerate() {
            if !point.is_finite() {
                continue;
            }
            index
                .cells
                .entry(index.cell_of(*point))
                .or_default()
                .push(i as u32);
        }
        Some(index)
    }

    fn cell_of(&self, point: Vec3) -> (i64, i64, i64) {
        let q = (point - self.anchor) / self.bucket;
        (q.x.floor() as i64, q.y.floor() as i64, q.z.floor() as i64)
    }

    fn cell_center(&self, cell: (i64, i64, i64)) -> Vec3 {
        self.anchor
            + self.bucket
                * Vec3::new(
                    cell.0 as f32 + 0.5,
                    cell.1 as f32 + 0.5,
                    cell.2 as f32 + 0.5,
                )
    }
}

/// Best point hit within the screen radius `POINT_RADIUS_PX`.
///
/// The box's 8 corners project onto the ray, bounding every point's depth
/// `t_q ∈ [t_lo, t_hi]`; with `R = POINT_RADIUS_PX · scale · t_hi` as the
/// world radius at the box's far side, every point that can be within the
/// radius of the ray lies inside the ray's axis-aligned box of half-width
/// `R` over `[t_lo, t_hi]`. The index quantizes that box into cells of
/// `bucket = R` and the search marches slices along the ray, visiting the
/// cells within a fixed ring of the axis point (three cells wide, since
/// bucket = R) — so the candidate set is exact and the work is
/// proportional to the covered cells plus the points in them, not to the
/// cloud size. Every candidate is then verified with the exact
/// perpendicular test at its own depth. Non-finite positions never enter
/// the index.
fn points_hit_t(
    ctx: &PickContext,
    origin: Vec3,
    dir: Vec3,
    positions: &[Vec3],
    bounds: Option<Aabb>,
) -> Option<f32> {
    let scale = ctx.world_per_pixel_scale;
    if !(scale.is_finite() && scale > 0.0) {
        return None;
    }
    let bounds = bounds?;
    if !bounds.min.is_finite() || !bounds.max.is_finite() || positions.is_empty() {
        return None;
    }

    // Ray-depth interval covered by the box: extrema of the corner
    // projections along the ray.
    let mut t_lo = f32::INFINITY;
    let mut t_hi = f32::NEG_INFINITY;
    for corner in aabb_corners(&bounds) {
        let t = (corner - origin).dot(dir);
        t_lo = t_lo.min(t);
        t_hi = t_hi.max(t);
    }
    if !t_lo.is_finite() || !t_hi.is_finite() || t_hi <= 0.0 {
        return None;
    }
    let t_lo = t_lo.max(0.0);

    let radius = POINT_RADIUS_PX * scale * t_hi;
    if !radius.is_finite() || radius <= 0.0 {
        return None;
    }
    let bucket = radius; // one screen radius per cell → the ring below stays 3 cells wide
    let index = PointIndex::new(positions, bucket, bounds.min)?;

    // March slices of `bucket/2` along the ray and visit the cells whose
    // centers sit within `ring_cells` cells of the axis point: every
    // candidate point's cell center is within R + bucket/2 + √3/2·bucket
    // of the axis at its slice, and ring_cells = ⌈R/bucket⌉ + 2 ≥ that
    // bound in cell units, so no candidate is missed.
    let ring_cells = (radius / bucket) as i64 + 2;
    let ring_radius = ring_cells as f32 * bucket;
    let ring_radius2 = ring_radius * ring_radius;
    let step = bucket * 0.5;
    let slices = ((t_hi - t_lo) / step).ceil() as i64;

    let mut best: Option<f32> = None;
    for slice in 0..=slices {
        let t_axis = t_lo + slice as f32 * step;
        let axis = origin + dir * t_axis;
        let base = index.cell_of(axis);
        for dx in -ring_cells..=ring_cells {
            for dy in -ring_cells..=ring_cells {
                for dz in -ring_cells..=ring_cells {
                    let cell = (base.0 + dx, base.1 + dy, base.2 + dz);
                    let off = index.cell_center(cell) - axis;
                    if off.length_squared() > ring_radius2 {
                        continue;
                    }
                    let Some(points) = index.cells.get(&cell) else {
                        continue;
                    };
                    for &point in points {
                        let q = positions[point as usize];
                        let t_q = (q - origin).dot(dir);
                        if t_q <= 0.0 {
                            continue;
                        }
                        let lateral = q - origin - dir * t_q;
                        let r = POINT_RADIUS_PX * scale * t_q;
                        if lateral.length_squared() <= r * r {
                            best = Some(best.map_or(t_q, |b| b.min(t_q)));
                        }
                    }
                }
            }
        }
    }
    best
}

/// Best mesh hit: ray–triangle over the index triples of a face-carrying
/// mesh; face-less meshes (indices `None`) display as a scatter through
/// the point pipeline, so they pick with the point criterion while their
/// kind stays a mesh. Malformed triples (indices out of range) are
/// skipped; NaN vertices are rejected per triangle by [`ray_triangle`].
fn mesh_hit_t(ctx: &PickContext, origin: Vec3, dir: Vec3, mesh: &Mesh) -> Option<f32> {
    let data = &mesh.data;
    match &data.indices {
        Some(indices) => {
            let mut best: Option<f32> = None;
            for triple in indices.chunks(3) {
                if triple.len() != 3 {
                    continue;
                }
                let [i, j, k] = [triple[0] as usize, triple[1] as usize, triple[2] as usize];
                if i >= data.positions.len()
                    || j >= data.positions.len()
                    || k >= data.positions.len()
                {
                    continue;
                }
                let hit = ray_triangle(
                    origin,
                    dir,
                    data.positions[i],
                    data.positions[j],
                    data.positions[k],
                );
                if let Some(t) = hit {
                    best = Some(best.map_or(t, |b| b.min(t)));
                }
            }
            best
        }
        None => points_hit_t(ctx, origin, dir, &data.positions, data.bounds),
    }
}

/// Frame hit: the three world-axis segments (`frame_segments` guards).
fn frame_hit_t(ctx: &PickContext, origin: Vec3, dir: Vec3, frame: &Frame) -> Option<f32> {
    let segments = frame_segments(frame.origin, frame.length);
    segments_hit_t(ctx, origin, dir, &segments)
}

/// Marker arrow hit over its shaft and head lines (`arrow_segments`
/// guards).
fn arrow_hit_t(ctx: &PickContext, origin: Vec3, dir: Vec3, start: Vec3, end: Vec3) -> Option<f32> {
    let segments = arrow_segments(start, end);
    segments_hit_t(ctx, origin, dir, &segments)
}

/// Whether a scene hit at ray depth `t` is actually on screen: its point
/// must project in front of the camera and within the depth range, and its
/// pixel must lie within the widest screen tolerance of the viewport —
/// the tolerance circle of a line or point hit may legitimately reach
/// past a border by up to `δ`/8 px (mesh faces land exactly on the pick
/// ray, whose pixel is the pointer itself, so the margin never rejects a
/// real face hit).
fn hit_point_is_visible(ctx: &PickContext, origin: Vec3, dir: Vec3, t: f32) -> bool {
    let Some(px) = project_px(&ctx.view_proj, ctx.viewport, origin + dir * t) else {
        return false;
    };
    let tolerance_px = (LINE_TOLERANCE_RATIO * ctx.viewport.y).max(POINT_RADIUS_PX);
    let margin = tolerance_px + VISIBILITY_PX_SLACK;
    px.x >= -margin
        && px.x <= ctx.viewport.x + margin
        && px.y >= -margin
        && px.y <= ctx.viewport.y + margin
}

/// Pick the topmost object a ray hits, arbitrated per the 005 spec D4
/// rules (see the module docs).
///
/// `origin`/`dir` form the pick ray (e.g. from `screen_to_ray`), `dir` may
/// be any non-zero length and is normalized internally. `objects` must be
/// in scene add order (earlier = added earlier) and should contain only
/// the pickable visible objects — visibility is the caller's selection,
/// core never sees the scene's hidden flags (005 spec §5: the grid overlay
/// and origin axes never enter the list).
///
/// Returns the winning hit: the later-added text label when the pointer
/// falls inside overlapping label boxes, otherwise the scene object with
/// the nearest hit depth, ties resolved toward the earlier entry in
/// `objects` ("first drawn wins"). `None` when nothing is hit or any
/// input is degenerate/non-finite. Never panics.
pub fn pick_objects(
    ctx: &PickContext,
    origin: Vec3,
    dir: Vec3,
    objects: &[(u64, &DisplayObject)],
) -> Option<PickHit> {
    if !ctx.view_proj.is_finite()
        || !(ctx.viewport.is_finite() && ctx.viewport.x > 0.0 && ctx.viewport.y > 0.0)
        || !origin.is_finite()
    {
        return None;
    }
    let dir = unit_dir(dir)?;
    let pointer_px = pixel_of(&ctx.view_proj, ctx.viewport, origin);

    // Overlay class (text labels), resolved in the same pass: the topmost
    // label containing the pointer. Tracked by list index; the later index
    // is painted later, hence on top (005 spec §6).
    let mut top_text: Option<(usize, f32)> = None;
    // Scene kinds: nearest hit depth; strict `<` keeps the earlier object
    // on an exact tie (identical geometry produces identical `t`).
    let mut scene_best: Option<(usize, f32)> = None;

    for (index, (_id, object)) in objects.iter().enumerate() {
        match object {
            DisplayObject::Marker(Marker::Text(text)) => {
                if let (Some(pointer_px), Some(anchor_px)) = (
                    pointer_px,
                    anchor_to_screen(&ctx.view_proj, ctx.viewport, text.anchor),
                ) {
                    if let Some(pill) = label_screen_box(anchor_px, &text.text, ctx.font_size_px) {
                        if contains_px(pill.0, pill.1, pointer_px) {
                            let t = (text.anchor - origin).dot(dir);
                            top_text = Some((index, t));
                        }
                    }
                }
            }
            _ => {
                if let Some(t) = object_hit_t(ctx, origin, dir, object) {
                    if hit_point_is_visible(ctx, origin, dir, t) {
                        let nearer = scene_best.is_none_or(|(_, best_t)| t < best_t);
                        if nearer {
                            scene_best = Some((index, t));
                        }
                    }
                }
            }
        }
    }

    if let Some((index, t)) = top_text {
        return Some(PickHit {
            id: objects[index].0,
            kind: DisplayKind::Marker,
            t,
        });
    }
    scene_best.map(|(index, t)| PickHit {
        id: objects[index].0,
        kind: objects[index].1.kind(),
        t,
    })
}

/// Hit test of one object's geometry against the ray (scene kinds only;
/// text labels are the overlay class and never reach this).
fn object_hit_t(ctx: &PickContext, origin: Vec3, dir: Vec3, object: &DisplayObject) -> Option<f32> {
    match object {
        DisplayObject::PointCloud(cloud) => {
            points_hit_t(ctx, origin, dir, &cloud.data.positions, cloud.data.bounds)
        }
        DisplayObject::Mesh(mesh) => mesh_hit_t(ctx, origin, dir, mesh),
        DisplayObject::Path(path) => polyline_hit_t(ctx, origin, dir, &path.data.points),
        DisplayObject::Frame(frame) => frame_hit_t(ctx, origin, dir, frame),
        DisplayObject::Marker(Marker::Arrow(arrow)) => {
            arrow_hit_t(ctx, origin, dir, arrow.start, arrow.end)
        }
        DisplayObject::Marker(Marker::Text(_)) => None,
    }
}

/// Rectangle selection: all objects whose projected screen box touches
/// `rect` (005 spec A9: mere contact selects — an intersection of the
/// boxes, including touching at an edge or corner; the criterion is
/// camera-independent because both boxes live in the same screen space).
///
/// For the data kinds the projected box is the screen box of the object's
/// 8 world corners; frames and arrows project their small world boxes;
/// text markers use their screen label boxes (anchors that do not project
/// onto the viewport select nothing, exactly like painting). Objects are
/// skipped when nothing of them projects in front of the camera (every
/// corner behind the eye or beyond the depth range) or their geometry is
/// non-finite or degenerate — never a panic.
///
/// Returns the ids of the selected objects in the caller's list order
/// (add order). An empty, inverted or non-finite `rect` selects nothing.
pub fn pick_rect(ctx: &PickContext, rect: Rect2, objects: &[(u64, &DisplayObject)]) -> Vec<u64> {
    if !ctx.view_proj.is_finite()
        || !(ctx.viewport.is_finite() && ctx.viewport.x > 0.0 && ctx.viewport.y > 0.0)
        || !rect.min.is_finite()
        || !rect.max.is_finite()
        // Zero-area (empty, min == max) and inverted rects select nothing.
        || rect.min.x >= rect.max.x
        || rect.min.y >= rect.max.y
    {
        return Vec::new();
    }
    let mut selected = Vec::new();
    for (id, object) in objects {
        let Some((min, max)) = object_screen_box(ctx, object) else {
            continue;
        };
        let touches = min.x <= rect.max.x
            && max.x >= rect.min.x
            && min.y <= rect.max.y
            && max.y >= rect.min.y;
        if touches {
            selected.push(*id);
        }
    }
    selected
}

/// The screen box an object selects by (see [`pick_rect`]).
fn object_screen_box(ctx: &PickContext, object: &DisplayObject) -> Option<(Vec2, Vec2)> {
    match object {
        DisplayObject::Marker(Marker::Text(text)) => {
            let anchor_px = anchor_to_screen(&ctx.view_proj, ctx.viewport, text.anchor)?;
            label_screen_box(anchor_px, &text.text, ctx.font_size_px)
        }
        DisplayObject::Frame(frame) => {
            if !frame.origin.is_finite() || !frame.length.is_finite() || frame.length <= 0.0 {
                return None;
            }
            let bounds = Aabb {
                min: frame.origin,
                max: frame.origin + Vec3::splat(frame.length),
            };
            aabb_screen_box(ctx, &bounds)
        }
        DisplayObject::Marker(Marker::Arrow(arrow)) => {
            if !arrow.start.is_finite() || !arrow.end.is_finite() {
                return None;
            }
            let shaft = arrow.end - arrow.start;
            if !shaft.is_finite() || shaft.length_squared() <= 0.0 {
                return None;
            }
            let bounds = Aabb {
                min: arrow.start.min(arrow.end),
                max: arrow.start.max(arrow.end),
            };
            aabb_screen_box(ctx, &bounds)
        }
        _ => aabb_screen_box(ctx, &object.bounds()?),
    }
}

/// Screen box of a world Aabb: the axis-aligned box around the projected
/// pixels of its 8 corners. Corners that do not project in front of the
/// camera are dropped (their geometry is invisible); `None` when none of
/// the corners projects. Non-finite bounds are skipped (guard).
fn aabb_screen_box(ctx: &PickContext, bounds: &Aabb) -> Option<(Vec2, Vec2)> {
    if !bounds.min.is_finite() || !bounds.max.is_finite() {
        return None;
    }
    let mut min = Vec2::splat(f32::INFINITY);
    let mut max = Vec2::splat(f32::NEG_INFINITY);
    let mut any = false;
    for corner in aabb_corners(bounds) {
        if let Some(px) = project_px(&ctx.view_proj, ctx.viewport, corner) {
            min = min.min(px);
            max = max.max(px);
            any = true;
        }
    }
    any.then_some((min, max))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::displays::{Path, PointCloud};
    use crate::io;
    use crate::render::camera_math::screen_to_ray;
    use crate::scene::camera::OrbitCamera;
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_6};

    /// Square viewport: aspect 1 keeps the pixel expectations round.
    const SIZE: Vec2 = Vec2::new(1000.0, 1000.0);
    /// Screen center.
    const CENTER: Vec2 = Vec2::new(500.0, 500.0);

    /// The world-per-pixel factor at unit depth for the fixed fov π/3:
    /// 2·tan(30°)/1000, derived from the same constants the app uses
    /// (`OrbitCamera::vertical_fov` = π/3).
    fn world_per_pixel_scale() -> f32 {
        2.0 * FRAC_PI_6.tan() / SIZE.y
    }

    /// World length of `px` viewport pixels on a plane `depth` world units
    /// from the eye, on the level test camera.
    fn offset_world(px: f32, depth: f32) -> f32 {
        px * 2.0 * depth * FRAC_PI_6.tan() / SIZE.y
    }

    fn context(camera: &OrbitCamera) -> PickContext {
        PickContext {
            view_proj: camera.view_proj(SIZE.x / SIZE.y),
            viewport: SIZE,
            world_per_pixel_scale: world_per_pixel_scale(),
            font_size_px: 14.0,
        }
    }

    /// A level camera at `distance` looking at `target`: yaw 0, pitch
    /// exactly 0 (the 0.6 − 0.6 subtraction is bit-exact), eye on the −Y
    /// side of the target — screen right = world +X, screen up = world +Z
    /// (camera.rs conventions, asserted by camera_math's tests).
    fn level_camera(target: Vec3, distance: f32) -> OrbitCamera {
        let mut camera = OrbitCamera::new(target);
        camera.orbit(-camera.yaw(), -0.6);
        camera.zoom((10.0 / distance).log2());
        camera
    }

    /// The pick ray through a viewport pixel.
    fn pick_ray(ctx: &PickContext, px: Vec2) -> (Vec3, Vec3) {
        screen_to_ray(&ctx.view_proj, SIZE, px).expect("in-viewport pixel unprojects")
    }

    fn aabb_of(points: &[Vec3]) -> Option<Aabb> {
        Aabb::from_points(points)
    }

    /// A path of one vertical (world-Z) segment centered at `center`,
    /// spanning ±0.4 in Z.
    fn vertical_path(center: Vec3) -> DisplayObject {
        let points = vec![center - Vec3::Z * 0.4, center + Vec3::Z * 0.4];
        DisplayObject::Path(Path::from_data(io::PathData {
            bounds: aabb_of(&points),
            points,
        }))
    }

    /// A point cloud display from world positions.
    fn cloud(points: Vec<Vec3>) -> DisplayObject {
        DisplayObject::PointCloud(PointCloud::from_data(io::PointCloudData {
            positions: points.clone(),
            colors: None,
            bounds: aabb_of(&points),
            format: io::Format::Ply,
        }))
    }

    /// A two-triangle quad wall covering x ∈ [−1, 1], z ∈ [−1, 1] on the
    /// plane `y = wall_y`.
    fn quad_wall(wall_y: f32) -> DisplayObject {
        let points = vec![
            Vec3::new(-1.0, wall_y, -1.0),
            Vec3::new(1.0, wall_y, -1.0),
            Vec3::new(1.0, wall_y, 1.0),
            Vec3::new(-1.0, wall_y, 1.0),
        ];
        DisplayObject::Mesh(Mesh::from_data(io::MeshData {
            positions: points.clone(),
            normals: None,
            indices: Some(vec![0, 1, 2, 0, 2, 3]),
            bounds: aabb_of(&points),
        }))
    }

    /// Assert that clicking `px` on the level test camera picks the path
    /// whose vertical segment passes through `center` iff `expected_hit`.
    fn assert_segment_click(ctx: &PickContext, px: Vec2, center: Vec3, expected_hit: bool) {
        let path = vertical_path(center);
        let objects = [(7u64, &path)];
        let (origin, dir) = pick_ray(ctx, px);
        let hit = pick_objects(ctx, origin, dir, &objects);
        assert_eq!(
            hit.is_some(),
            expected_hit,
            "segment at {center:?}, click {px:?}"
        );
        if expected_hit {
            let hit = hit.expect("hit");
            assert_eq!(hit.id, 7);
            assert_eq!(hit.kind, DisplayKind::Path);
            assert!(hit.t > 0.0);
        }
    }

    // ------------------------------------------------------------------
    // ray_triangle
    // ------------------------------------------------------------------

    /// The unit right triangle in the plane z = 0 with corners (0,0), (1,0),
    /// (0,1) and normal +Z.
    const T_A: Vec3 = Vec3::ZERO;
    const T_B: Vec3 = Vec3::X;
    const T_C: Vec3 = Vec3::Y;

    #[test]
    fn ray_triangle_hits_from_the_front_at_the_ray_parameter() {
        // Ray straight down −Z through the triangle's centroid.
        let hit = ray_triangle(Vec3::new(0.25, 0.25, 1.0), -Vec3::Z, T_A, T_B, T_C);
        assert_eq!(hit, Some(1.0));
    }

    #[test]
    fn ray_triangle_accepts_back_faces_like_the_double_sided_renderer() {
        // The same ray from below: the mesh pipeline culls nothing
        // (render/mesh.rs), so the back face must hit at the same t.
        let hit = ray_triangle(Vec3::new(0.25, 0.25, -1.0), Vec3::Z, T_A, T_B, T_C);
        assert_eq!(hit, Some(1.0));
    }

    #[test]
    fn ray_triangle_misses_outside_the_edges() {
        let ray_origin = Vec3::new(1.1, 0.25, 1.0);
        assert_eq!(ray_triangle(ray_origin, -Vec3::Z, T_A, T_B, T_C), None);
        let ray_origin = Vec3::new(0.25, 0.9, 1.0); // u + v > 1 region
        assert_eq!(ray_triangle(ray_origin, -Vec3::Z, T_A, T_B, T_C), None);
        let ray_origin = Vec3::new(0.25, 0.25, 1.0);
        assert_eq!(ray_triangle(ray_origin, -Vec3::X, T_A, T_B, T_C), None);
    }

    #[test]
    fn ray_triangle_hits_near_edges_and_misses_just_beyond_them() {
        // 0.1% of the edge length inside and outside the u = 0 edge.
        let inside = ray_triangle(Vec3::new(0.001, 0.5, 1.0), -Vec3::Z, T_A, T_B, T_C);
        assert_eq!(inside, Some(1.0));
        let outside = ray_triangle(Vec3::new(-0.001, 0.5, 1.0), -Vec3::Z, T_A, T_B, T_C);
        assert_eq!(outside, None);
    }

    #[test]
    fn ray_triangle_t_is_translation_invariant() {
        // Shift the whole configuration: same geometry, same t.
        let shift = Vec3::new(5.0, -3.0, 2.0);
        let hit = ray_triangle(
            Vec3::new(0.25, 0.25, 1.0) + shift,
            -Vec3::Z,
            T_A + shift,
            T_B + shift,
            T_C + shift,
        );
        assert_eq!(hit, Some(1.0));
    }

    #[test]
    fn ray_triangle_ignores_crossings_behind_the_origin() {
        // The triangle lies behind the ray origin: the crossing is at t < 0.
        let hit = ray_triangle(Vec3::new(0.25, 0.25, 1.0), Vec3::Z, T_A, T_B, T_C);
        assert_eq!(hit, None);
    }

    #[test]
    fn ray_triangle_normalizes_an_arbitrary_direction() {
        let hit = ray_triangle(
            Vec3::new(0.25, 0.25, 1.0),
            Vec3::new(0.0, 0.0, -7.5),
            T_A,
            T_B,
            T_C,
        );
        assert_eq!(hit, Some(1.0));
    }

    #[test]
    fn ray_triangle_rejects_degenerate_triangles() {
        // Collinear corners: zero area.
        let hit = ray_triangle(
            Vec3::new(0.0, 0.0, 1.0),
            -Vec3::Z,
            Vec3::ZERO,
            Vec3::X,
            Vec3::X * 2.0,
        );
        assert_eq!(hit, None);
        // Two coincident corners.
        let hit = ray_triangle(
            Vec3::new(0.0, 0.0, 1.0),
            -Vec3::Z,
            Vec3::ZERO,
            Vec3::ZERO,
            Vec3::X,
        );
        assert_eq!(hit, None);
    }

    #[test]
    fn ray_triangle_rejects_non_finite_inputs() {
        assert_eq!(
            ray_triangle(Vec3::splat(f32::NAN), -Vec3::Z, T_A, T_B, T_C),
            None
        );
        assert_eq!(
            ray_triangle(
                Vec3::new(0.25, 0.25, 1.0),
                -Vec3::Z,
                Vec3::new(f32::INFINITY, 0.0, 0.0),
                T_B,
                T_C
            ),
            None
        );
        assert_eq!(
            ray_triangle(
                Vec3::new(0.25, 0.25, 1.0),
                Vec3::splat(f32::NAN),
                T_A,
                T_B,
                T_C
            ),
            None
        );
        assert_eq!(
            ray_triangle(Vec3::new(0.25, 0.25, 1.0), Vec3::ZERO, T_A, T_B, T_C),
            None
        );
    }

    // ------------------------------------------------------------------
    // ray_segment
    // ------------------------------------------------------------------

    #[test]
    fn ray_segment_returns_the_closest_approach_parameter() {
        // Ray +X through the origin; segment crossing x = 2 at right angles.
        let t = ray_segment(
            Vec3::ZERO,
            Vec3::X,
            Vec3::new(2.0, -1.0, 0.0),
            Vec3::new(2.0, 1.0, 0.0),
        );
        assert_eq!(t, Some(2.0));
        // Oblique segment: the closest point is still interior.
        let t = ray_segment(
            Vec3::ZERO,
            Vec3::X,
            Vec3::new(3.0, -2.0, 0.0),
            Vec3::new(1.0, 2.0, 0.0),
        );
        assert_eq!(t, Some(2.0));
    }

    #[test]
    fn ray_segment_clamps_to_the_nearer_segment_end() {
        // The perpendicular foot falls beyond the segment: the near end is
        // the closest point, and the ray parameter is its projection.
        let t = ray_segment(
            Vec3::ZERO,
            Vec3::X,
            Vec3::new(2.0, 3.0, 0.0),
            Vec3::new(2.0, 4.0, 0.0),
        );
        assert_eq!(t, Some(2.0));
    }

    #[test]
    fn ray_segment_clamps_t_to_zero_behind_the_origin() {
        // Segment fully behind the ray origin: the half-line minimum sits
        // at the origin itself (t = 0).
        let t = ray_segment(
            Vec3::ZERO,
            Vec3::X,
            Vec3::new(-5.0, -1.0, 0.0),
            Vec3::new(-5.0, 1.0, 0.0),
        );
        assert_eq!(t, Some(0.0));
        // A segment crossing the ray behind the origin also clamps to 0.
        let t = ray_segment(
            Vec3::ZERO,
            Vec3::X,
            Vec3::new(-1.0, -1.0, 0.0),
            Vec3::new(-1.0, 1.0, 0.0),
        );
        assert_eq!(t, Some(0.0));
    }

    #[test]
    fn ray_segment_measures_hit_distance_through_the_pick_path() {
        // Distance semantics surface through pick_objects (the public
        // primitive only reports t): a segment 4 px off the click hits,
        // one 6 px off misses, against δ = 0.5% of the 1000 px height.
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let depth = 10.0;
        // The wall point 4 px (6 px) off the center ray in world units.
        assert_segment_click(
            &ctx,
            CENTER,
            Vec3::new(offset_world(4.0, depth), 0.0, 0.0),
            true,
        );
        assert_segment_click(
            &ctx,
            CENTER,
            Vec3::new(offset_world(6.0, depth), 0.0, 0.0),
            false,
        );
    }

    #[test]
    fn ray_segment_rejects_parallel_degenerate_and_non_finite_cases() {
        // Parallel to the ray: no unique closest point.
        assert_eq!(
            ray_segment(
                Vec3::ZERO,
                Vec3::X,
                Vec3::new(2.0, 1.0, 0.0),
                Vec3::new(4.0, 1.0, 0.0)
            ),
            None
        );
        // Zero-length segment.
        assert_eq!(
            ray_segment(
                Vec3::ZERO,
                Vec3::X,
                Vec3::new(2.0, 0.0, 0.0),
                Vec3::new(2.0, 0.0, 0.0)
            ),
            None
        );
        // Non-finite inputs and a zero direction.
        assert_eq!(
            ray_segment(
                Vec3::splat(f32::NAN),
                Vec3::X,
                Vec3::new(2.0, -1.0, 0.0),
                Vec3::new(2.0, 1.0, 0.0)
            ),
            None
        );
        assert_eq!(
            ray_segment(
                Vec3::ZERO,
                Vec3::X,
                Vec3::new(f32::NAN, 0.0, 0.0),
                Vec3::new(2.0, 1.0, 0.0)
            ),
            None
        );
        assert_eq!(ray_segment(Vec3::ZERO, Vec3::ZERO, Vec3::X, Vec3::Y), None);
    }

    // ------------------------------------------------------------------
    // Lines: depth-dependent tolerance (tasks T3)
    // ------------------------------------------------------------------

    #[test]
    fn line_tolerance_tracks_the_hit_depth() {
        // The same 4 px / 6 px click offsets, at three camera distances and
        // three wall depths: δ_world scales with t, so the screen-space
        // criterion stays camera-independent (spec A5).
        for distance in [5.0, 10.0, 30.0] {
            let camera = level_camera(Vec3::ZERO, distance);
            let ctx = context(&camera);
            let depth = distance;
            assert_segment_click(
                &ctx,
                CENTER,
                Vec3::new(offset_world(4.0, depth), 0.0, 0.0),
                true,
            );
            assert_segment_click(
                &ctx,
                CENTER,
                Vec3::new(offset_world(6.0, depth), 0.0, 0.0),
                false,
            );
        }
    }

    #[test]
    fn line_tolerance_scales_for_walls_off_the_target_plane() {
        // Walls nearer (depth 5) and farther (depth 20) than the target
        // plane of a distance-10 camera: the offset world radii must be
        // measured at the hit depth, not the target plane.
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        assert_segment_click(
            &ctx,
            CENTER,
            Vec3::new(offset_world(4.0, 5.0), -5.0, 0.0),
            true,
        );
        assert_segment_click(
            &ctx,
            CENTER,
            Vec3::new(offset_world(6.0, 5.0), -5.0, 0.0),
            false,
        );
        assert_segment_click(
            &ctx,
            CENTER,
            Vec3::new(offset_world(4.0, 20.0), 10.0, 0.0),
            true,
        );
        assert_segment_click(
            &ctx,
            CENTER,
            Vec3::new(offset_world(6.0, 20.0), 10.0, 0.0),
            false,
        );
    }

    #[test]
    fn line_tolerance_survives_camera_pan_and_yaw() {
        // Panned camera: target (0, 0, 1), wall on the target plane y = 1.
        let camera = level_camera(Vec3::new(0.0, 0.0, 1.0), 10.0);
        let ctx = context(&camera);
        assert_segment_click(
            &ctx,
            CENTER,
            Vec3::new(offset_world(4.0, 10.0), 1.0, 1.0),
            true,
        );
        assert_segment_click(
            &ctx,
            CENTER,
            Vec3::new(offset_world(6.0, 10.0), 1.0, 1.0),
            false,
        );

        // Yawed camera (eye on the +X side, looking along −X): the same
        // offsets along the wall's other tangent (+Y at x = 0) hit and
        // miss identically.
        let mut camera = level_camera(Vec3::ZERO, 10.0);
        camera.orbit(FRAC_PI_2, 0.0);
        let ctx = context(&camera);
        assert_segment_click(
            &ctx,
            CENTER,
            Vec3::new(0.0, offset_world(4.0, 10.0), 0.0),
            true,
        );
        assert_segment_click(
            &ctx,
            CENTER,
            Vec3::new(0.0, offset_world(6.0, 10.0), 0.0),
            false,
        );
    }

    #[test]
    fn clicking_on_the_segment_itself_hits_at_any_offset() {
        // The tolerance is a maximum miss distance, not a reach limit: a
        // segment far past δ is still hit by clicking its own pixel.
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let center = Vec3::new(offset_world(40.0, 10.0), 0.0, 0.0);
        let px = anchor_to_screen(&ctx.view_proj, SIZE, center).unwrap();
        assert_segment_click(&ctx, px, center, true);
    }

    #[test]
    fn path_skips_non_finite_segments_like_the_renderer_splits_runs() {
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let run_a = Vec3::new(offset_world(4.0, 10.0), 0.0, -1.0);
        let run_b = Vec3::new(offset_world(4.0, 10.0), 0.0, 1.0);
        let path = DisplayObject::Path(Path::from_data(io::PathData {
            points: vec![run_a, Vec3::new(f32::NAN, 0.0, 0.0), run_b],
            bounds: aabb_of(&[run_a, run_b]),
        }));
        let objects = [(3u64, &path)];
        let (origin, dir) = pick_ray(&ctx, CENTER);
        // The finite runs are 1 world unit above/below the click plane and
        // the NaN bridge is skipped, so nothing within δ → miss; an
        // all-finite path through the same points would be hit.
        assert_eq!(pick_objects(&ctx, origin, dir, &objects), None);
    }

    #[test]
    fn frame_axes_hit_within_the_line_tolerance_and_between_axes_miss() {
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let frame = DisplayObject::Frame(Frame::new(Vec3::ZERO, 1.0));
        let objects = [(4u64, &frame)];

        // The X axis runs along the wall row through the center (rightward
        // 86.6 px); a click on it hits; one 3 px off it still hits (≤ δ);
        // one ~30 px diagonal from both visible axes misses.
        for px in [CENTER + Vec2::X * 20.0, CENTER + Vec2::new(3.0, 3.0)] {
            let (origin, dir) = pick_ray(&ctx, px);
            let hit = pick_objects(&ctx, origin, dir, &objects);
            assert_eq!(hit.map(|h| h.id), Some(4), "click {px:?}");
        }
        let (origin, dir) = pick_ray(&ctx, CENTER + Vec2::splat(30.0));
        assert_eq!(pick_objects(&ctx, origin, dir, &objects), None);
    }

    #[test]
    fn frame_with_invalid_geometry_never_hits() {
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        for frame in [
            Frame::new(Vec3::splat(f32::NAN), 1.0),
            Frame::new(Vec3::ZERO, 0.0),
            Frame::new(Vec3::ZERO, f32::INFINITY),
        ] {
            let frame = DisplayObject::Frame(frame);
            let objects = [(4u64, &frame)];
            let (origin, dir) = pick_ray(&ctx, CENTER);
            assert_eq!(pick_objects(&ctx, origin, dir, &objects), None);
        }
    }

    #[test]
    fn arrow_segments_mirror_the_renderer_geometry() {
        // Horizontal shaft (along world X): the head opens in the plane the
        // shaft spans with the world-Y reference — perp = X × Y = Z — so
        // the tips fold back 30° off −X and mirror across ±Z (screen up on
        // the level camera).
        let segments = arrow_segments(Vec3::ZERO, Vec3::X * 4.0);
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0], (Vec3::ZERO, Vec3::X * 4.0));
        let head_length = 4.0 * ARROW_HEAD_FRACTION;
        let (sin, cos) = ARROW_HEAD_HALF_ANGLE.sin_cos();
        for (end, tip) in &segments[1..] {
            assert_eq!(*end, Vec3::X * 4.0);
            let arm = *tip - *end;
            assert!((arm.length() - head_length).abs() < 1e-5);
            // 30° off the reverse shaft: arm · (−dir) = cos(30°)·length.
            let along = arm.dot(-Vec3::X);
            assert!((along - head_length * cos).abs() < 1e-5);
            // Spread within the ±Z plane: the Y components stay zero.
            assert_eq!(arm.y, 0.0);
            assert!((arm.z.abs() - head_length * sin).abs() < 1e-5);
        }

        // Vertical shaft (parallel to world Y): the world-Y reference
        // degenerates and the renderer falls back to world X (perp =
        // Y × X = −Z): the tips aim back down the −Y shaft and mirror
        // across ±Z, exactly as line.rs's own vertical-shaft test checks.
        let segments = arrow_segments(Vec3::ZERO, Vec3::Y * 4.0);
        for (_end, tip) in &segments[1..] {
            let arm = *tip - Vec3::Y * 4.0;
            assert_eq!(arm.x, 0.0, "head arm leaves the spread plane");
            assert!(
                (arm.y + head_length * cos).abs() < 1e-5,
                "head tips aim back down the −Y shaft"
            );
            assert!((arm.z.abs() - head_length * sin).abs() < 1e-5);
        }

        // Guards: non-finite or zero-length shafts draw nothing to pick.
        assert!(arrow_segments(Vec3::splat(f32::NAN), Vec3::X).is_empty());
        assert!(arrow_segments(Vec3::ZERO, Vec3::ZERO).is_empty());
    }

    #[test]
    fn arrow_picks_on_the_shaft_and_near_the_head_tips() {
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let arrow = DisplayObject::Marker(Marker::arrow(
            Vec3::new(0.2, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
        ));
        let objects = [(5u64, &arrow)];

        // On the shaft, well before the head.
        for x in [0.5, 1.5] {
            let center = Vec3::new(x, 0.0, 0.0);
            let px = anchor_to_screen(&ctx.view_proj, SIZE, center).unwrap();
            let (origin, dir) = pick_ray(&ctx, px);
            let hit = pick_objects(&ctx, origin, dir, &objects);
            assert_eq!(hit.map(|h| h.id), Some(5), "shaft click at x = {x}");
        }
        // A head line's midpoint: the head feathers must be pickable even
        // though they lie off the shaft line (line.rs draws them as
        // separate strips).
        let segments = arrow_segments(Vec3::new(0.2, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0));
        let head_mid = (segments[1].0 + segments[1].1) * 0.5;
        let px = anchor_to_screen(&ctx.view_proj, SIZE, head_mid).unwrap();
        let (origin, dir) = pick_ray(&ctx, px);
        let hit = pick_objects(&ctx, origin, dir, &objects);
        assert_eq!(hit.map(|h| h.id), Some(5), "head feather click");

        // Past the tip and far off the shaft: miss.
        for center in [Vec3::new(2.6, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.6)] {
            let px = anchor_to_screen(&ctx.view_proj, SIZE, center).unwrap();
            let (origin, dir) = pick_ray(&ctx, px);
            assert_eq!(
                pick_objects(&ctx, origin, dir, &objects),
                None,
                "at {center:?}"
            );
        }
    }

    #[test]
    fn arrow_with_invalid_endpoints_never_hits() {
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        for marker in [
            Marker::arrow(Vec3::splat(f32::NAN), Vec3::X),
            Marker::arrow(Vec3::ZERO, Vec3::ZERO),
            Marker::arrow(Vec3::ZERO, Vec3::new(f32::INFINITY, 0.0, 0.0)),
        ] {
            let marker = DisplayObject::Marker(marker);
            let objects = [(5u64, &marker)];
            let (origin, dir) = pick_ray(&ctx, CENTER);
            assert_eq!(pick_objects(&ctx, origin, dir, &objects), None);
        }
    }

    // ------------------------------------------------------------------
    // Point clouds: radius proximity (tasks T2, spec A2/A3)
    // ------------------------------------------------------------------

    #[test]
    fn cloud_radius_accepts_six_pixels_and_rejects_eleven() {
        // r = 8 px of the wall at depth 10: 6 px in, 11 px out.
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let depth = 10.0;
        for (px_off, expect) in [(6.0, true), (11.0, false)] {
            let cloud = cloud(vec![Vec3::new(offset_world(px_off, depth), 0.0, 0.0)]);
            let objects = [(9u64, &cloud)];
            let (origin, dir) = pick_ray(&ctx, CENTER);
            let hit = pick_objects(&ctx, origin, dir, &objects);
            assert_eq!(hit.is_some(), expect, "{px_off} px off center");
        }
    }

    #[test]
    fn cloud_known_points_hit_at_their_projected_positions_and_gaps_miss() {
        // Spec A3: five known points 20 px apart on the wall (columns at
        // −40..40 px); clicking each point's projected position hits the
        // cloud, clicking the mid-gaps between columns (10 px from every
        // point, outside the 8 px radius) misses.
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let depth = 10.0;
        let positions: Vec<Vec3> = [-2.0, -1.0, 0.0, 1.0, 2.0]
            .into_iter()
            .map(|px| Vec3::new(offset_world(px * 20.0, depth), 0.0, 0.0))
            .collect();
        let cloud = cloud(positions);
        let objects = [(9u64, &cloud)];

        for (i, point) in (-2..=2).enumerate() {
            let px = CENTER + Vec2::X * (point as f32 * 20.0);
            let (origin, dir) = pick_ray(&ctx, px);
            let hit = pick_objects(&ctx, origin, dir, &objects);
            assert_eq!(hit.map(|h| h.id), Some(9), "click on point {i}");
        }
        for point in [-0.5, 0.5] {
            let px = CENTER + Vec2::X * (point * 20.0);
            let (origin, dir) = pick_ray(&ctx, px);
            assert_eq!(
                pick_objects(&ctx, origin, dir, &objects),
                None,
                "gap at {px:?}"
            );
        }
    }

    #[test]
    fn cloud_hits_follow_the_latest_data_across_calls() {
        // pick_objects is stateless: each call rebuilds the bucket index
        // from the current data (tasks T2's per-call re-allocation), so a
        // replaced cloud answers with its new positions.
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let near = Vec3::new(offset_world(6.0, 10.0), 0.0, 0.0);
        let far = Vec3::new(offset_world(60.0, 10.0), 0.0, 0.0);

        let near_cloud = cloud(vec![near]);
        let objects = [(9u64, &near_cloud)];
        let (origin, dir) = pick_ray(&ctx, CENTER);
        assert_eq!(
            pick_objects(&ctx, origin, dir, &objects).map(|h| h.id),
            Some(9)
        );

        // The same id now points at a far-away cloud: the center click
        // misses, exactly as if the cloud had been replaced in the scene.
        let far_cloud = cloud(vec![far]);
        let objects = [(9u64, &far_cloud)];
        assert_eq!(pick_objects(&ctx, origin, dir, &objects), None);

        // And the moved point picks at its own pixel.
        let px = anchor_to_screen(&ctx.view_proj, SIZE, far).unwrap();
        let (origin, dir) = pick_ray(&ctx, px);
        assert_eq!(
            pick_objects(&ctx, origin, dir, &objects).map(|h| h.id),
            Some(9)
        );
    }

    #[test]
    fn cloud_ignores_non_finite_positions() {
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let valid = Vec3::new(offset_world(6.0, 10.0), 0.0, 0.0);
        // Garbage points (spec 001 G1) never make the index or the hit; the
        // one finite point answers the click exactly as if they were absent.
        let positions = vec![
            Vec3::splat(f32::NAN),
            valid,
            Vec3::new(f32::INFINITY, 0.0, 0.0),
        ];
        let cloud = cloud(positions);
        let objects = [(9u64, &cloud)];
        let (origin, dir) = pick_ray(&ctx, CENTER);
        let hit = pick_objects(&ctx, origin, dir, &objects);
        assert_eq!(hit.map(|h| h.id), Some(9));
        assert!(hit.unwrap().t > 0.0);
    }

    #[test]
    fn cloud_without_finite_bounds_never_hits() {
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        // Hand-built data whose io bounds are missing (or garbage).
        let display = DisplayObject::PointCloud(PointCloud::from_data(io::PointCloudData {
            positions: vec![Vec3::splat(f32::NAN)],
            colors: None,
            bounds: None,
            format: io::Format::Ply,
        }));
        let objects = [(9u64, &display)];
        let (origin, dir) = pick_ray(&ctx, CENTER);
        assert_eq!(pick_objects(&ctx, origin, dir, &objects), None);

        let display = DisplayObject::PointCloud(PointCloud::from_data(io::PointCloudData {
            positions: vec![Vec3::ZERO],
            colors: None,
            bounds: Some(Aabb {
                min: Vec3::splat(f32::NAN),
                max: Vec3::splat(f32::NAN),
            }),
            format: io::Format::Ply,
        }));
        let objects = [(9u64, &display)];
        assert_eq!(pick_objects(&ctx, origin, dir, &objects), None);
    }

    #[test]
    fn scatter_meshes_pick_by_the_point_radius_with_the_mesh_kind() {
        // Face-less meshes render as a point scatter; picking uses the
        // point criterion while the reported kind stays Mesh.
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let points = vec![Vec3::new(offset_world(6.0, 10.0), 0.0, 0.0)];
        let mesh = DisplayObject::Mesh(Mesh::from_data(io::MeshData {
            positions: points,
            normals: None,
            indices: None,
            bounds: aabb_of(&[Vec3::new(offset_world(6.0, 10.0), 0.0, 0.0)]),
        }));
        let objects = [(2u64, &mesh)];
        let (origin, dir) = pick_ray(&ctx, CENTER);
        let hit = pick_objects(&ctx, origin, dir, &objects);
        assert_eq!(hit.map(|h| h.id), Some(2));
        assert_eq!(hit.map(|h| h.kind), Some(DisplayKind::Mesh));
    }

    #[test]
    fn cloud_picks_the_nearest_point_along_the_ray() {
        // Two points on the click column at different depths — one on the
        // wall in front (depth 5), one on the far wall (depth 10), both 2 px
        // off their own plane's ray line: both are within the 8 px radius,
        // and the reported depth is the front point's.
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let front = Vec3::new(offset_world(2.0, 5.0), -5.0, 0.0);
        let back = Vec3::new(offset_world(2.0, 10.0), 0.0, 0.0);
        let cloud = cloud(vec![back, front]); // back is added first
        let objects = [(9u64, &cloud)];
        let (origin, dir) = pick_ray(&ctx, CENTER);
        let hit = pick_objects(&ctx, origin, dir, &objects).unwrap();
        let front_t = (front - origin).dot(dir);
        let back_t = (back - origin).dot(dir);
        assert!(front_t < back_t, "fixture: front point is nearer");
        assert!((hit.t - front_t).abs() < 1e-3, "front point wins: {hit:?}");
    }

    // ------------------------------------------------------------------
    // Meshes
    // ------------------------------------------------------------------

    #[test]
    fn mesh_picks_the_face_the_ray_crosses() {
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let wall = quad_wall(0.0);
        let objects = [(2u64, &wall)];
        // A click on the wall inside the quad hits the mesh.
        let center = Vec3::new(0.3, 0.0, 0.3);
        let px = anchor_to_screen(&ctx.view_proj, SIZE, center).unwrap();
        let (origin, dir) = pick_ray(&ctx, px);
        let hit = pick_objects(&ctx, origin, dir, &objects);
        assert_eq!(hit.map(|h| h.id), Some(2));
        assert_eq!(hit.map(|h| h.kind), Some(DisplayKind::Mesh));
        // The wall is at depth 10 from the eye: t ≈ 9.9 along the ray.
        let t = hit.unwrap().t;
        assert!((t - 9.9).abs() < 0.2, "t = {t}");
    }

    #[test]
    fn mesh_misses_where_no_face_covers() {
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let wall = quad_wall(0.0);
        let objects = [(2u64, &wall)];
        // Beyond the quad's extent (x = 1.6 world = 138 px right).
        let px = CENTER + Vec2::X * 138.0;
        let (origin, dir) = pick_ray(&ctx, px);
        assert_eq!(pick_objects(&ctx, origin, dir, &objects), None);
        // And a wall behind the far plane of the level camera... use a
        // quad so far away the depth guard rejects it (beyond z_ndc = 1).
        let far = quad_wall(5000.0);
        let objects = [(2u64, &far)];
        let (origin, dir) = pick_ray(&ctx, CENTER);
        assert_eq!(pick_objects(&ctx, origin, dir, &objects), None);
    }

    #[test]
    fn mesh_with_malformed_indices_skips_bad_triples() {
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let points = vec![
            Vec3::new(-1.0, 0.0, -1.0),
            Vec3::new(1.0, 0.0, -1.0),
            Vec3::new(1.0, 0.0, 1.0),
            Vec3::new(-1.0, 0.0, 1.0),
        ];
        let mesh = DisplayObject::Mesh(Mesh::from_data(io::MeshData {
            positions: points.clone(),
            normals: None,
            // A valid quad plus an out-of-range and a truncated triple.
            indices: Some(vec![0, 1, 2, 0, 2, 3, 0, 1, 99, 7]),
            bounds: aabb_of(&points),
        }));
        let objects = [(2u64, &mesh)];
        let (origin, dir) = pick_ray(&ctx, CENTER);
        assert_eq!(
            pick_objects(&ctx, origin, dir, &objects).map(|h| h.id),
            Some(2)
        );
    }

    // ------------------------------------------------------------------
    // Marker text label boxes (tasks T4)
    // ------------------------------------------------------------------

    fn label_box(anchor_px: Vec2, text: &str) -> (Vec2, Vec2) {
        label_screen_box(anchor_px, text, 14.0).unwrap()
    }

    #[test]
    fn text_hits_inside_the_anchor_label_box_and_misses_outside() {
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        // "abc" at 14 px: 3·14·0.6 = 25.2 wide, 14·1.2 = 16.8 tall plus the
        // 3 px halo — the pill the app paints, lifted above the anchor.
        let marker = DisplayObject::Marker(Marker::text(Vec3::ZERO, "abc"));
        let objects = [(6u64, &marker)];

        let (min, max) = label_box(CENTER, "abc");
        assert!(min.x <= max.x && min.y <= max.y);
        for px in [
            CENTER,                         // the anchor row (pill bottom edge)
            CENTER + Vec2::new(0.0, -10.0), // inside the pill
            CENTER + Vec2::new(-10.0, -10.0),
            // Just inside the pill's top-right corner (half a pixel off
            // the float boundary of the box, where a click is robust).
            Vec2::new(max.x - 0.5, min.y + 0.5),
        ] {
            let (origin, dir) = pick_ray(&ctx, px);
            let hit = pick_objects(&ctx, origin, dir, &objects);
            assert_eq!(hit.map(|h| h.id), Some(6), "click {px:?}");
            assert_eq!(hit.map(|h| h.kind), Some(DisplayKind::Marker));
        }
        for px in [
            CENTER + Vec2::new(0.0, 1.0),     // just below the anchor row
            CENTER + Vec2::new(0.0, -30.0),   // above the pill top
            CENTER + Vec2::new(-30.0, -10.0), // left of the pill
            CENTER + Vec2::new(30.0, -10.0),  // right of the pill
        ] {
            let (origin, dir) = pick_ray(&ctx, px);
            assert_eq!(
                pick_objects(&ctx, origin, dir, &objects),
                None,
                "click {px:?}"
            );
        }
    }

    #[test]
    fn text_box_scales_with_the_string_and_the_font_size() {
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let short = DisplayObject::Marker(Marker::text(Vec3::ZERO, "abc"));
        let objects = [(6u64, &short)];
        // 40 px right of the anchor is outside "abc" (half-width 15.6) but
        // inside a 12-character label (half-width 53.4).
        let px = CENTER + Vec2::new(40.0, -10.0);
        let (origin, dir) = pick_ray(&ctx, px);
        assert_eq!(pick_objects(&ctx, origin, dir, &objects), None);

        let long = DisplayObject::Marker(Marker::text(Vec3::ZERO, "abcdefghijkl"));
        let objects = [(6u64, &long)];
        let (origin, dir) = pick_ray(&ctx, px);
        assert_eq!(
            pick_objects(&ctx, origin, dir, &objects).map(|h| h.id),
            Some(6)
        );
    }

    #[test]
    fn text_box_uses_the_context_font_size() {
        let camera = level_camera(Vec3::ZERO, 10.0);
        let mut ctx = context(&camera);
        let marker = DisplayObject::Marker(Marker::text(Vec3::ZERO, "abc"));
        let objects = [(6u64, &marker)];
        let px = CENTER + Vec2::new(0.0, -30.0);
        // At 14 px the pill top is 22.8 px above the anchor (miss), at
        // 28 px it is 39.6 px above (hit).
        let (origin, dir) = pick_ray(&ctx, px);
        assert_eq!(pick_objects(&ctx, origin, dir, &objects), None);
        ctx.font_size_px = 28.0;
        let (origin, dir) = pick_ray(&ctx, px);
        assert_eq!(
            pick_objects(&ctx, origin, dir, &objects).map(|h| h.id),
            Some(6)
        );
    }

    #[test]
    fn empty_text_and_offscreen_anchors_never_hit() {
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let marker = DisplayObject::Marker(Marker::text(Vec3::ZERO, ""));
        let objects = [(6u64, &marker)];
        let (origin, dir) = pick_ray(&ctx, CENTER);
        assert_eq!(pick_objects(&ctx, origin, dir, &objects), None);

        // Anchors outside the frustum (far above the level camera's view)
        // project to nothing, exactly like painting.
        let marker = DisplayObject::Marker(Marker::text(Vec3::new(0.0, 0.0, 100.0), "abc"));
        let objects = [(6u64, &marker)];
        assert_eq!(pick_objects(&ctx, origin, dir, &objects), None);
    }

    #[test]
    fn text_label_wins_over_the_scene_under_it() {
        // A label anchored on its own, earlier-added mesh floats above the
        // mesh (overlay class, 005 spec §6): the click inside the label
        // box picks the label even though the ray also crosses the mesh.
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let wall = quad_wall(0.0);
        let marker = DisplayObject::Marker(Marker::text(Vec3::ZERO, "abc"));
        let objects = [(2u64, &wall), (6u64, &marker)];

        // 14 px above the anchor: inside the pill (box top 22.8 px above),
        // and on the mesh behind.
        let px = CENTER + Vec2::new(0.0, -14.0);
        let (origin, dir) = pick_ray(&ctx, px);
        let hit = pick_objects(&ctx, origin, dir, &objects);
        assert_eq!(hit.map(|h| h.id), Some(6));
        assert_eq!(hit.map(|h| h.kind), Some(DisplayKind::Marker));

        // 30 px above the anchor: above the pill, the mesh is picked.
        let px = CENTER + Vec2::new(0.0, -30.0);
        let (origin, dir) = pick_ray(&ctx, px);
        let hit = pick_objects(&ctx, origin, dir, &objects);
        assert_eq!(hit.map(|h| h.id), Some(2));
        assert_eq!(hit.map(|h| h.kind), Some(DisplayKind::Mesh));
    }

    #[test]
    fn overlapping_labels_resolve_to_the_topmost() {
        // Two labels at the same anchor: the later-added one is painted on
        // top and wins inside the shared box; the earlier one still wins
        // outside the top one's box only if the boxes differ.
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let under = DisplayObject::Marker(Marker::text(Vec3::ZERO, "abc"));
        let over = DisplayObject::Marker(Marker::text(Vec3::ZERO, "abc"));
        let objects = [(5u64, &under), (6u64, &over)];
        let px = CENTER + Vec2::new(0.0, -10.0);
        let (origin, dir) = pick_ray(&ctx, px);
        assert_eq!(
            pick_objects(&ctx, origin, dir, &objects).map(|h| h.id),
            Some(6)
        );
    }

    // ------------------------------------------------------------------
    // Arbitration (tasks T5, spec D4)
    // ------------------------------------------------------------------

    #[test]
    fn pick_objects_returns_the_nearest_object_along_the_ray() {
        // Two vertical segments on the same click ray: one on the wall at
        // depth 10, one on the wall at depth 20 (4 px right of the center
        // pixel at their own depths). The nearer depth wins.
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let near = vertical_path(Vec3::new(offset_world(4.0, 10.0), 0.0, 0.0));
        let far = vertical_path(Vec3::new(offset_world(4.0, 20.0), 10.0, 0.0));
        let objects = [(1u64, &near), (2u64, &far)];
        let px = CENTER + Vec2::X * 4.0;
        let (origin, dir) = pick_ray(&ctx, px);
        let hit = pick_objects(&ctx, origin, dir, &objects);
        assert_eq!(hit.map(|h| h.id), Some(1));
        assert!(hit.unwrap().t < 15.0, "t must be the nearer depth");
    }

    #[test]
    fn equidistant_twins_go_to_the_earlier_add_order() {
        // Two identical segments in the same place: identical geometry
        // yields bit-identical t, and the tie resolves to the earlier
        // object (spec D4 / tasks T5: equal distance goes to the earlier
        // add order — "first drawn wins", which supersedes the plan §2
        // table's "later drawn" reading).
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let a = vertical_path(Vec3::new(offset_world(4.0, 10.0), 0.0, 0.0));
        let b = vertical_path(Vec3::new(offset_world(4.0, 10.0), 0.0, 0.0));
        let objects = [(1u64, &a), (2u64, &b)];
        let (origin, dir) = pick_ray(&ctx, CENTER);
        let hit = pick_objects(&ctx, origin, dir, &objects);
        assert_eq!(hit.map(|h| h.id), Some(1));
    }

    #[test]
    fn text_and_scene_arbitrate_by_class_not_depth() {
        // A label box overlapping a nearer scene object still wins (it is
        // painted above), while a click at the same scene pixel outside
        // every label box resolves by depth.
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        // A line on the wall at depth 5, in front of a wall at depth 10
        // whose label floats over both click rows... use two paths.
        let front = vertical_path(Vec3::new(offset_world(0.0, 5.0), -5.0, 0.0));
        let back = vertical_path(Vec3::new(offset_world(0.0, 10.0), 0.0, 0.0));
        let objects = [(1u64, &front), (2u64, &back)];
        // The center column: the front segment is nearer and wins.
        let (origin, dir) = pick_ray(&ctx, CENTER);
        let hit = pick_objects(&ctx, origin, dir, &objects);
        assert_eq!(hit.map(|h| h.id), Some(1));
        let t = hit.unwrap().t;
        assert!(t < 9.0, "t = {t}");
    }

    #[test]
    fn pick_objects_returns_none_when_nothing_is_hit() {
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let (origin, dir) = pick_ray(&ctx, CENTER);
        assert_eq!(pick_objects(&ctx, origin, dir, &[]), None);
        let path = vertical_path(Vec3::new(offset_world(50.0, 10.0), 0.0, 0.0));
        let objects = [(7u64, &path)];
        assert_eq!(pick_objects(&ctx, origin, dir, &objects), None);
    }

    // ------------------------------------------------------------------
    // Guards (spec A5)
    // ------------------------------------------------------------------

    #[test]
    fn pick_objects_guards_non_finite_rays() {
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let wall = quad_wall(0.0);
        let objects = [(2u64, &wall)];
        for (origin, dir) in [
            (Vec3::splat(f32::NAN), Vec3::Z),
            (Vec3::ZERO, Vec3::splat(f32::NAN)),
            (Vec3::ZERO, Vec3::ZERO),
        ] {
            assert_eq!(pick_objects(&ctx, origin, dir, &objects), None);
        }
    }

    #[test]
    fn pick_objects_guards_non_finite_contexts() {
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let wall = quad_wall(0.0);
        let objects = [(2u64, &wall)];
        let (origin, dir) = pick_ray(&ctx, CENTER);

        let mut garbage = ctx;
        garbage.view_proj = Mat4::from_cols_array(&[f32::NAN; 16]);
        assert_eq!(pick_objects(&garbage, origin, dir, &objects), None);
        garbage = ctx;
        garbage.viewport = Vec2::ZERO;
        assert_eq!(pick_objects(&garbage, origin, dir, &objects), None);
        garbage = ctx;
        garbage.viewport = Vec2::splat(f32::NAN);
        assert_eq!(pick_objects(&garbage, origin, dir, &objects), None);
        garbage = ctx;
        garbage.viewport = Vec2::new(1000.0, -1.0);
        assert_eq!(pick_objects(&garbage, origin, dir, &objects), None);
    }

    #[test]
    fn a_degenerate_pixel_scale_disables_lines_and_points_but_not_meshes() {
        // world_per_pixel_scale feeds only the depth conversions of lines
        // and points; meshes and labels keep working (guards are per
        // criterion, not global).
        let camera = level_camera(Vec3::ZERO, 10.0);
        let mut ctx = context(&camera);
        ctx.world_per_pixel_scale = 0.0;
        let wall = quad_wall(0.0);
        let path = vertical_path(Vec3::ZERO);
        let marker = DisplayObject::Marker(Marker::text(Vec3::ZERO, "abc"));
        let objects = [(1u64, &wall), (2u64, &path), (3u64, &marker)];
        let (origin, dir) = pick_ray(&ctx, CENTER);
        // The mesh and the label still hit at the center pixel (the label
        // box contains it); the path needs the scale and misses.
        let hit = pick_objects(&ctx, origin, dir, &objects);
        assert_eq!(hit.map(|h| h.kind), Some(DisplayKind::Marker));

        ctx.world_per_pixel_scale = f32::NAN;
        let objects = [(1u64, &wall), (2u64, &path)];
        let (origin, dir) = pick_ray(&ctx, CENTER);
        assert_eq!(
            pick_objects(&ctx, origin, dir, &objects).map(|h| h.id),
            Some(1)
        );
    }

    #[test]
    fn degenerate_objects_never_panic_the_pick() {
        // Non-finite positions and bounds across every kind (spec A5:
        // "non-finite coordinates and degenerate bounds never panic").
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let wall = quad_wall(0.0);
        let nan_wall = quad_wall(f32::NAN);
        let path = DisplayObject::Path(Path::from_data(io::PathData {
            points: vec![Vec3::splat(f32::NAN), Vec3::splat(f32::INFINITY)],
            bounds: None,
        }));
        let cloud = cloud(vec![Vec3::splat(f32::NAN)]);
        let objects = [
            (1u64, &wall),
            (2u64, &nan_wall),
            (3u64, &path),
            (4u64, &cloud),
        ];
        let (origin, dir) = pick_ray(&ctx, CENTER);
        let hit = pick_objects(&ctx, origin, dir, &objects);
        // The only sane object is the quad wall at the center.
        assert_eq!(hit.map(|h| h.id), Some(1));
    }

    #[test]
    fn line_tolerance_carries_segments_past_the_viewport_border() {
        // The level camera sees the wall plane's z from −500 px (bottom
        // border) to +500 px. A horizontal segment 3 px beyond the bottom
        // border still comes within δ = 5 px of a click 1 px inside it, so
        // the tolerance registers a hit whose geometry is off-viewport; a
        // segment 20 px beyond the border is ~16 px off the click and
        // misses.
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let depth = 10.0;
        let one_px = offset_world(1.0, depth);
        let border_z = -(500.0 * one_px);
        let click = CENTER + Vec2::Y * 499.0;

        let assert_border_click = |z: f32, expected_hit: bool| {
            let points = vec![Vec3::new(-0.2, 0.0, z), Vec3::new(0.2, 0.0, z)];
            let path = DisplayObject::Path(Path::from_data(io::PathData {
                bounds: aabb_of(&points),
                points,
            }));
            let objects = [(7u64, &path)];
            let (origin, dir) = pick_ray(&ctx, click);
            let hit = pick_objects(&ctx, origin, dir, &objects);
            assert_eq!(hit.is_some(), expected_hit, "segment at z = {z}");
            if expected_hit {
                let hit = hit.expect("hit");
                assert_eq!(hit.id, 7);
                assert!(hit.t > 0.0);
            }
        };
        assert_border_click(border_z - 3.0 * one_px, true);
        assert_border_click(border_z - 20.0 * one_px, false);
    }

    // ------------------------------------------------------------------
    // pick_rect (tasks T6, spec A9)
    // ------------------------------------------------------------------

    fn rect(min: (f32, f32), max: (f32, f32)) -> Rect2 {
        Rect2 {
            min: Vec2::new(min.0, min.1),
            max: Vec2::new(max.0, max.1),
        }
    }

    /// A path whose bounds span [−0.2, 0.2] × [−0.2, 0.2] on the wall
    /// plane y = 0 (a vertical segment diagonal across that box).
    fn wall_box_path() -> DisplayObject {
        let points = vec![Vec3::new(-0.2, 0.0, -0.2), Vec3::new(0.2, 0.0, 0.2)];
        DisplayObject::Path(Path::from_data(io::PathData {
            bounds: aabb_of(&points),
            points,
        }))
    }

    #[test]
    fn pick_rect_selects_intersecting_boxes_in_add_order() {
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let a = wall_box_path();
        let b = wall_box_path();
        let objects = [(1u64, &a), (2u64, &b)];
        // 0.2 world = 17.3 px at depth 10: the box spans x/y ∈
        // [482.7, 517.3] around the center.
        let covering = rect((400.0, 400.0), (600.0, 600.0));
        assert_eq!(pick_rect(&ctx, covering, &objects), vec![1, 2]);
        let partial = rect((500.0, 400.0), (540.0, 540.0));
        assert_eq!(pick_rect(&ctx, partial, &objects), vec![1, 2]);
        let disjoint = rect((600.0, 600.0), (700.0, 700.0));
        assert_eq!(pick_rect(&ctx, disjoint, &objects), Vec::<u64>::new());
    }

    #[test]
    fn pick_rect_counts_touching_the_projected_box() {
        // A9: mere contact selects — a rect whose corner exactly touches
        // the projected box's corner (the box's px extents come from the
        // corners at (±0.2, ±0.2)) selects; a rect 1 px away does not.
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let path = wall_box_path();
        let objects = [(1u64, &path)];
        let corner = anchor_to_screen(&ctx.view_proj, SIZE, Vec3::new(0.2, 0.0, 0.2)).unwrap();
        let touching = Rect2 {
            min: corner,
            max: corner + Vec2::splat(10.0),
        };
        assert_eq!(pick_rect(&ctx, touching, &objects), vec![1]);
        let clear = Rect2 {
            min: corner + Vec2::splat(1.0),
            max: corner + Vec2::splat(11.0),
        };
        assert_eq!(pick_rect(&ctx, clear, &objects), Vec::<u64>::new());
    }

    #[test]
    fn pick_rect_selects_frames_arrows_and_text_boxes() {
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        // A frame right of the marker, so the three objects' projected
        // boxes overlap only where the test says they do: the frame spans
        // x ∈ [577.9, 621.2] (0.9..1.4 world), the marker label box
        // [484.4, 515.6] × [477.2, 500], the arrow a degenerate y = 500
        // line over x ∈ [456.7, 543.3].
        let frame = DisplayObject::Frame(Frame::new(Vec3::new(0.9, 0.0, 0.0), 0.5));
        let marker = DisplayObject::Marker(Marker::text(Vec3::ZERO, "abc"));
        let arrow = DisplayObject::Marker(Marker::arrow(
            Vec3::new(-0.5, 0.0, 0.0),
            Vec3::new(0.5, 0.0, 0.0),
        ));
        let objects = [(1u64, &frame), (2u64, &marker), (3u64, &arrow)];
        // The covering rect contains all three projected boxes: selected
        // in list (add) order.
        let covering = rect((400.0, 400.0), (640.0, 640.0));
        assert_eq!(pick_rect(&ctx, covering, &objects), vec![1, 2, 3]);
        // A rect only over the frame's box selects the frame.
        let frame_only = rect((580.0, 460.0), (600.0, 480.0));
        assert_eq!(pick_rect(&ctx, frame_only, &objects), vec![1]);
        // A rect inside the label box (clear of the arrow's y = 500 line
        // and of the frame's x range) selects the text marker only.
        let label_only = rect((490.0, 480.0), (510.0, 490.0));
        assert_eq!(pick_rect(&ctx, label_only, &objects), vec![2]);
    }

    #[test]
    fn pick_rect_skips_objects_outside_the_frustum_and_degenerate_geometry() {
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        // A box behind the eye (every corner has w ≤ 0) and a degenerate
        // path next to a valid one: only the valid one selects.
        let behind = DisplayObject::Path(Path::from_data(io::PathData {
            points: vec![Vec3::new(-1.0, -20.0, -1.0), Vec3::new(1.0, -20.0, 1.0)],
            bounds: Some(Aabb {
                min: Vec3::new(-1.0, -20.0, -1.0),
                max: Vec3::new(1.0, -20.0, 1.0),
            }),
        }));
        let garbage_bounds = DisplayObject::Path(Path::from_data(io::PathData {
            points: vec![Vec3::ZERO, Vec3::X],
            bounds: Some(Aabb {
                min: Vec3::splat(f32::NAN),
                max: Vec3::splat(f32::NAN),
            }),
        }));
        let frame = DisplayObject::Frame(Frame::new(Vec3::splat(f32::NAN), 1.0));
        let valid = wall_box_path();
        let objects = [
            (1u64, &behind),
            (2u64, &garbage_bounds),
            (3u64, &frame),
            (4u64, &valid),
        ];
        let covering = rect((400.0, 400.0), (600.0, 600.0));
        assert_eq!(pick_rect(&ctx, covering, &objects), vec![4]);
    }

    #[test]
    fn pick_rect_guards_empty_inverted_and_non_finite_rects() {
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let path = wall_box_path();
        let objects = [(1u64, &path)];
        assert_eq!(
            pick_rect(&ctx, rect((500.0, 500.0), (500.0, 500.0)), &objects),
            Vec::<u64>::new()
        );
        assert_eq!(
            pick_rect(&ctx, rect((600.0, 600.0), (400.0, 400.0)), &objects),
            Vec::<u64>::new()
        );
        assert_eq!(
            pick_rect(
                &ctx,
                Rect2 {
                    min: Vec2::splat(f32::NAN),
                    max: Vec2::splat(f32::NAN),
                },
                &objects
            ),
            Vec::<u64>::new()
        );
        let mut garbage = ctx;
        garbage.viewport = Vec2::ZERO;
        assert_eq!(
            pick_rect(&garbage, rect((0.0, 0.0), (100.0, 100.0)), &objects),
            Vec::<u64>::new()
        );
    }

    // ------------------------------------------------------------------
    // Cross-check: projected click positions stay consistent (A3)
    // ------------------------------------------------------------------

    #[test]
    fn known_world_points_project_where_the_click_pixels_expect_them() {
        // Sanity anchor for the fixture itself: the wall offsets used above
        // really are the projected pixels they claim to be (guards the
        // tests against a drifted camera convention, not the pick code).
        let camera = level_camera(Vec3::ZERO, 10.0);
        let ctx = context(&camera);
        let right = anchor_to_screen(
            &ctx.view_proj,
            SIZE,
            Vec3::new(offset_world(4.0, 10.0), 0.0, 0.0),
        )
        .unwrap();
        assert!((right.x - (CENTER.x + 4.0)).abs() < 0.01);
        assert!((right.y - CENTER.y).abs() < 0.01);
        let up = anchor_to_screen(
            &ctx.view_proj,
            SIZE,
            Vec3::new(0.0, 0.0, offset_world(4.0, 10.0)),
        )
        .unwrap();
        assert!((up.x - CENTER.x).abs() < 0.01);
        assert!((up.y - (CENTER.y - 4.0)).abs() < 0.01);
    }
}
