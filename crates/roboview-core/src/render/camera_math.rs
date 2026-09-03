//! Screen-space projection for viewport overlays.
//!
//! Text labels (display-types spec §7 F4) are an overlay: the app draws them
//! with its egui painter on top of the viewport, and they never participate
//! in the shared-depth protocol (spec §6). Core's only part is the pure
//! projection that turns a world-space anchor into a screen position for the
//! painter — [`anchor_to_screen`] — so the whole path is unit-testable
//! headless (plan §3.3: "core 提供 `anchor_to_screen(view_proj, viewport) ->
//! ScreenPos` 纯函数，可单测").
//!
//! The 004 ui-blueprint work extends the same pure-function family (plan
//! §3.2): [`screen_to_ray`] unprojects a viewport pixel into a world ray,
//! [`pointer_world`] intersects that ray with the reference plane the grid
//! overlay lives on, and [`orientation_gizmo_dirs`] derives the orientation
//! indicator's axis directions. They share `anchor_to_screen`'s conventions
//! — pure and stateless, egui y-down pixels over the scene's view-projection
//! matrix — and stay headless-testable with it.
//!
//! This family lives here in `render` rather than next to the camera in
//! `scene`: it consumes the same view-projection matrix the scene's shared
//! uniform carries (render/mod.rs), and `scene/` is owned by the scene
//! container tasks. The functions take the matrix as an argument and never
//! touch GPU state, so they stay pure either way.

use glam::{Mat4, Vec2, Vec3, Vec4};

/// Project a world-space `anchor` through `view_proj` onto a viewport of
/// `viewport_size` pixels, returning the screen position in egui painter
/// coordinates (origin at the top-left of the viewport, y pointing down).
///
/// The projection follows the scene's camera convention (scene/camera.rs):
/// right-handed view space with the wgpu NDC range `z ∈ [0, 1]`. The anchor
/// is reported as `None` when it cannot be placed on the screen:
///
/// - the viewport size is not finite or not positive (minimized/hidden
///   window — there is no screen to place anything on);
/// - `anchor` is not finite (spec G1 data is kept but defended against);
/// - the view-projection matrix is not finite (hand-built garbage);
/// - the anchor projects at or behind the eye (`w ≤ 0`), outside the clip
///   volume (`|ndc.x| > 1`, `|ndc.y| > 1`), or outside the depth range
///   (`ndc.z ≤ 0` or `> 1`) — such anchors are culled, not clamped, so a
///   label near the frustum edge appears and disappears cleanly instead of
///   sliding along the viewport border.
///
/// Screen conversion: NDC `(-1, -1)` is the bottom-left pixel corner and
/// `(1, 1)` the top-right, so `x` maps directly while `y` flips — matching
/// the top-left origin of egui painter coordinates.
pub fn anchor_to_screen(view_proj: &Mat4, viewport_size: Vec2, anchor: Vec3) -> Option<Vec2> {
    if !viewport_size.is_finite() || viewport_size.x <= 0.0 || viewport_size.y <= 0.0 {
        return None;
    }
    if !anchor.is_finite() {
        return None;
    }
    let clip = view_proj * anchor.extend(1.0);
    if !clip.is_finite() {
        return None;
    }
    // For the right-handed view convention, w = −z_view: non-positive w is
    // at or behind the eye, where the division below would flip the image.
    if clip.w <= 0.0 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    if !ndc.is_finite() {
        return None;
    }
    if ndc.x < -1.0 || ndc.x > 1.0 || ndc.y < -1.0 || ndc.y > 1.0 {
        return None;
    }
    if ndc.z <= 0.0 || ndc.z > 1.0 {
        return None;
    }
    Some(Vec2::new(
        (ndc.x + 1.0) * 0.5 * viewport_size.x,
        (1.0 - ndc.y) * 0.5 * viewport_size.y,
    ))
}

/// Reference plane the viewport pointer is resolved against, for
/// [`pointer_world`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldPlane {
    /// The world plane `z = 0` — the plane the viewport grid overlay is
    /// drawn on (004 spec M5: the grid sits on the world Z=0 plane).
    GroundZ0,
    /// The camera's target plane: the plane through the orbit target,
    /// perpendicular to the view direction — the coordinate basis the app
    /// reports against while the grid is hidden (spec M5, "无网格时").
    ///
    /// This module only sees `view_proj`, and the true orbit target is not
    /// recoverable from it, so the plane is resolved as the plane through
    /// the **world origin** perpendicular to the view direction (the
    /// screen-center ray). That equals the true target plane exactly when
    /// the orbit target is the world origin — the default pose and the
    /// empty-scene case, which is the intended reading of M5.
    CameraTargetPlane,
}

/// Unproject a viewport pixel into a world-space ray.
///
/// `pos` is in egui painter coordinates (pixels, origin at the top-left of
/// the viewport, y pointing down) on a viewport of `viewport_size`;
/// `view_proj` follows the scene's camera convention (scene/camera.rs):
/// right-handed view space with the wgpu NDC range `z ∈ [0, 1]`.
///
/// The pixel is unprojected twice through the inverse of `view_proj` — once
/// on the near plane (`z_ndc = 0`) and once on the far plane (`z_ndc = 1`).
/// The returned pair is `(origin, direction)`, where `origin` is the
/// near-plane world point and `direction` the unit vector toward the
/// far-plane point. This two-point form needs no camera access (unlike an
/// `eye()`-based construction) and stays well scaled at every zoom: the two
/// points are always `far − near` world units apart along the ray, so the
/// direction never degenerates into the difference of two huge coordinates.
///
/// `None` when the ray cannot be formed: a non-finite or non-positive
/// `viewport_size` (a minimized or hidden window), a non-finite `pos` or
/// `view_proj`, or an unprojection landing at or behind the eye (`w ≤ 0`).
/// For a real perspective camera every finite pixel ray points forward, so
/// the `w ≤ 0` case only guards hand-built garbage matrices. Never panics.
pub fn screen_to_ray(view_proj: &Mat4, viewport_size: Vec2, pos: Vec2) -> Option<(Vec3, Vec3)> {
    if !viewport_size.is_finite() || viewport_size.x <= 0.0 || viewport_size.y <= 0.0 {
        return None;
    }
    if !pos.is_finite() {
        return None;
    }
    if !view_proj.is_finite() {
        return None;
    }
    let ndc = Vec2::new(
        pos.x / viewport_size.x * 2.0 - 1.0,
        1.0 - pos.y / viewport_size.y * 2.0,
    );
    // Clip points with w = 1 on the near (z = 0) and far (z = 1) NDC
    // depths: any scale is equivalent, the homogeneous divide below undoes
    // it. For the right-handed convention the homogeneous w is proportional
    // to the view depth, so w ≤ 0 means at or behind the eye.
    let inv = view_proj.inverse();
    let near_h = inv * Vec4::new(ndc.x, ndc.y, 0.0, 1.0);
    let far_h = inv * Vec4::new(ndc.x, ndc.y, 1.0, 1.0);
    if !near_h.is_finite() || !far_h.is_finite() {
        return None;
    }
    if near_h.w <= 0.0 || far_h.w <= 0.0 {
        return None;
    }
    let near = near_h.truncate() / near_h.w;
    let far = far_h.truncate() / far_h.w;
    let dir = far - near;
    if !near.is_finite() || !far.is_finite() || !dir.is_finite() {
        return None;
    }
    let dir = dir.normalize();
    if !dir.is_finite() || dir == Vec3::ZERO {
        return None;
    }
    Some((near, dir))
}

/// Intersect the ray through viewport pixel `pos` with the reference
/// `plane`, returning the world-space hit point.
///
/// Inputs follow [`screen_to_ray`] (egui pixels on `viewport_size`, the
/// scene's camera convention). The hit is validated against the view
/// frustum and reported as `None` when it does not exist or is not visible:
///
/// - no ray exists for the pixel (see [`screen_to_ray`]);
/// - the ray is parallel to the plane, or meets it at or behind the ray's
///   near-plane origin (`t ≤ 0` — e.g. a camera sitting on or below the
///   ground plane can never hit it in front);
/// - the hit lies outside the frustum: re-projecting it through
///   `view_proj` must put it in front of the eye (`w > 0`), laterally
///   inside the viewport (`|ndc.x| ≤ 1`, `|ndc.y| ≤ 1`) and within the
///   depth range (`0 < ndc.z ≤ 1`) — this rejects pointers just outside
///   the viewport and hits beyond the far plane.
///
/// See [`WorldPlane`] for the two reference planes and the
/// `CameraTargetPlane` approximation. Never panics.
pub fn pointer_world(
    view_proj: &Mat4,
    viewport_size: Vec2,
    pos: Vec2,
    plane: WorldPlane,
) -> Option<Vec3> {
    let (origin, dir) = screen_to_ray(view_proj, viewport_size, pos)?;
    let t = match plane {
        WorldPlane::GroundZ0 => {
            // Plane z = 0, normal (0, 0, 1): origin.z + t·dir.z = 0. The
            // zero-denominator case is a ray parallel to the plane.
            if dir.z == 0.0 {
                return None;
            }
            -origin.z / dir.z
        }
        WorldPlane::CameraTargetPlane => {
            // Plane through the world origin, normal = the view direction
            // (the screen-center ray — see WorldPlane::CameraTargetPlane).
            let view_dir = screen_to_ray(view_proj, viewport_size, viewport_size * 0.5)?.1;
            let denom = view_dir.dot(dir);
            if denom == 0.0 {
                return None;
            }
            -view_dir.dot(origin) / denom
        }
    };
    if !t.is_finite() || t <= 0.0 {
        return None;
    }
    let hit = origin + dir * t;
    if !hit.is_finite() {
        return None;
    }
    // Frustum validation of the hit (see the doc comment).
    let clip = view_proj * hit.extend(1.0);
    if !clip.is_finite() || clip.w <= 0.0 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    if !ndc.is_finite() {
        return None;
    }
    if ndc.x < -1.0 || ndc.x > 1.0 || ndc.y < -1.0 || ndc.y > 1.0 {
        return None;
    }
    if ndc.z <= 0.0 || ndc.z > 1.0 {
        return None;
    }
    Some(hit)
}

/// An axis-aligned rectangle in egui painter coordinates (pixels, origin at
/// the top-left of the viewport, y pointing down), used by viewport overlay
/// drawing to place content in a corner. Deliberately minimal: this module
/// only computes directions, it never lays content out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect2 {
    /// Top-left corner.
    pub min: Vec2,
    /// Bottom-right corner.
    pub max: Vec2,
}

/// Screen directions of the world axes for the orientation indicator.
///
/// Returns, for the world axes +X, +Y and +Z in that order, the on-screen
/// direction each axis runs in at its anchor — egui painter pixel space
/// (origin at the top-left of the viewport, y pointing down) — plus a
/// visibility flag:
///
/// `result[0] = (+X dir, +X visible)`, `result[1] = (+Y dir, +Y visible)`,
/// `result[2] = (+Z dir, +Z visible)`.
///
/// Each direction is read from the corresponding linear column of
/// `view_proj`, the image of the world axis as a clip-space direction
/// (`c_i = view_proj · e_i`), scaled into pixels and y-flipped:
///
/// ```text
/// dir_i = normalize((c_i.x · viewport_size.x, −c_i.y · viewport_size.y))
/// ```
///
/// No divide by `c_i.w` is needed: the columns of the 3×3 linear part are
/// pure direction images, and the pixel-space scaling (not plain NDC) keeps
/// the aspect correct, so a level camera maps +X exactly to screen right
/// and +Y exactly to screen up. The caller places the arms itself at pixel
/// length `len` inside `rect`; both are reserved for that placement step
/// and unused here — unit directions are position- and scale-free.
///
/// Visibility: an axis exactly parallel to the view direction has a zero
/// xy column — it projects to a point at its anchor (e.g. the Z axis of a
/// level camera, which the eye sits on) — and is reported as
/// `(Vec2::ZERO, false)`. Every other axis is visible.
///
/// Eye-behind note (spec §6 "w≤0 端点取反"): `c_i.w` is the axis's view
/// depth, negative when the axis's far end lies at or behind the eye. Such
/// axes are deliberately **not** negated: the arm follows the screen trend
/// of the axis's *positive* half at the anchor, which is exactly `dir_i`
/// above — an axis whose far end hides behind the eye still shows its near
/// half running along that trend (an axis passing near the eye sweeps
/// across the view toward its eye-side end, the direction the column's xy
/// points). Negating would mirror the arm onto the axis's negative half
/// and mislabel −e_i as +e_i; the `anchor_to_screen` `w ≤ 0` culling is
/// therefore not reused here by design.
///
/// A non-finite or non-positive `viewport_size`, or a non-finite
/// `view_proj`, reports all three axes as invisible. Never panics.
pub fn orientation_gizmo_dirs(
    view_proj: &Mat4,
    viewport_size: Vec2,
    rect: Rect2,
    len: f32,
) -> [(Vec2, bool); 3] {
    let invisible = (Vec2::ZERO, false);
    if !viewport_size.is_finite() || viewport_size.x <= 0.0 || viewport_size.y <= 0.0 {
        return [invisible; 3];
    }
    if !view_proj.is_finite() {
        return [invisible; 3];
    }
    // `rect` and `len` belong to the caller's placement step (where to draw
    // the gizmo and at what pixel length); unit directions are position-
    // and scale-free, so they are unused here.
    let _ = (rect, len);
    let arm = |axis: Vec4| -> (Vec2, bool) {
        let d = Vec2::new(axis.x * viewport_size.x, -axis.y * viewport_size.y);
        if d == Vec2::ZERO {
            invisible
        } else {
            // d is finite here (both factors are), and nonzero — normalize
            // is safe and the result is exactly a unit vector.
            (d.normalize(), true)
        }
    };
    [
        arm(view_proj.x_axis),
        arm(view_proj.y_axis),
        arm(view_proj.z_axis),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::camera::OrbitCamera;
    use std::f32::consts::{FRAC_PI_3, FRAC_PI_4, FRAC_PI_6};

    const SIZE: Vec2 = Vec2::new(1920.0, 1080.0);

    /// A level camera (pitch 0) looking at the origin from `distance` along
    /// −Z-ish, whose screen axes are exactly world X (right) and Y (up) —
    /// see camera.rs `view_axes`. `OrbitCamera::new` aims at the target with
    /// a default pose; `orbit` then levels the pitch.
    fn level_camera(distance: f32) -> OrbitCamera {
        let mut camera = OrbitCamera::new(Vec3::ZERO);
        // pitch default is 0.6 rad; orbit to exactly 0 to get a level view.
        camera.orbit(0.0, -0.6);
        // The default eye-to-target distance is 10: zoom(delta) multiplies
        // the distance by 2^−delta, so delta = log2(10/distance) sets it.
        camera.zoom((10.0 / distance).log2());
        camera
    }

    #[test]
    fn frustum_center_projects_to_the_viewport_center() {
        // The target sits on the principal axis of the frustum for every pose
        // (camera.rs), so any camera looking at the origin must place the
        // origin at the exact pixel center — aspect independent.
        let camera = OrbitCamera::new(Vec3::ZERO);
        let view_proj = camera.view_proj(SIZE.x / SIZE.y);
        let screen = anchor_to_screen(&view_proj, SIZE, Vec3::ZERO).unwrap();
        assert!(
            screen.abs_diff_eq(SIZE * 0.5, 1e-3),
            "origin must project to the center, got {screen:?}"
        );
    }

    #[test]
    fn world_axes_map_to_screen_right_and_up() {
        let camera = level_camera(10.0);
        let view_proj = camera.view_proj(SIZE.x / SIZE.y);

        let right = anchor_to_screen(&view_proj, SIZE, Vec3::X).unwrap();
        let center = SIZE * 0.5;
        assert!(
            right.x > center.x + 50.0,
            "world +X must sit right of center"
        );
        assert!(
            (right.y - center.y).abs() < 1e-2,
            "+X stays on the horizontal centerline"
        );

        let up = anchor_to_screen(&view_proj, SIZE, Vec3::Y).unwrap();
        assert!(
            up.y < center.y - 20.0,
            "world +Y (screen up) must sit above center"
        );
        assert!(
            (up.x - center.x).abs() < 1e-2,
            "+Y stays on the vertical centerline"
        );
    }

    #[test]
    fn anchors_at_or_behind_the_eye_project_to_none() {
        let camera = level_camera(10.0);
        let view_proj = camera.view_proj(SIZE.x / SIZE.y);
        let eye = Vec3::new(0.0, 0.0, 10.0); // level camera at +Z looks at the origin

        // Exactly at the eye: w = 0, division is undefined → None.
        assert_eq!(anchor_to_screen(&view_proj, SIZE, eye), None);
        // Behind the eye: w < 0 → None.
        assert_eq!(anchor_to_screen(&view_proj, SIZE, eye * 2.0), None);
        // Non-finite anchors (spec G1: kept in data, defended against here).
        assert_eq!(
            anchor_to_screen(&view_proj, SIZE, Vec3::splat(f32::NAN)),
            None
        );
        assert_eq!(
            anchor_to_screen(&view_proj, SIZE, Vec3::new(f32::INFINITY, 0.0, 0.0)),
            None
        );
    }

    #[test]
    fn anchors_outside_the_frustum_project_to_none() {
        let camera = level_camera(10.0);
        let view_proj = camera.view_proj(SIZE.x / SIZE.y);

        // Far to the side of a 60°-fov level camera at distance 10: the
        // frustum half-width at the target plane is tan(30°)·10 ≈ 5.77.
        assert_eq!(anchor_to_screen(&view_proj, SIZE, Vec3::X * 100.0), None);
        assert_eq!(anchor_to_screen(&view_proj, SIZE, Vec3::Y * 100.0), None);

        // Deep behind the target: beyond the far plane (depth > 1) → None.
        assert_eq!(anchor_to_screen(&view_proj, SIZE, -Vec3::Z * 1.0e5), None);

        // A point just inside the viewport still projects (frustum edge
        // sanity): world X at the exact half-width maps to ndc x = 1.
        let half_width = FRAC_PI_6.tan() * 10.0 * (SIZE.x / SIZE.y); // tan(fov_y/2)·d·aspect
        let edge = anchor_to_screen(&view_proj, SIZE, Vec3::X * (half_width - 0.01)).unwrap();
        assert!(
            edge.x > SIZE.x - 10.0,
            "edge anchor lands near the right border"
        );
    }

    #[test]
    fn degenerate_inputs_project_to_none() {
        let camera = OrbitCamera::new(Vec3::ZERO);
        let view_proj = camera.view_proj(SIZE.x / SIZE.y);

        // Non-positive or non-finite viewport sizes: no screen to place on.
        for size in [
            Vec2::ZERO,
            Vec2::new(-1.0, 100.0),
            Vec2::new(f32::NAN, 100.0),
            Vec2::new(100.0, f32::INFINITY),
        ] {
            assert_eq!(anchor_to_screen(&view_proj, size, Vec3::ZERO), None);
        }

        // Non-finite view-projection matrices (hand-built garbage): the
        // anchor cannot be placed.
        let garbage = Mat4::from_cols_array(&[f32::NAN; 16]);
        assert_eq!(anchor_to_screen(&garbage, SIZE, Vec3::ZERO), None);
    }

    #[test]
    fn projection_matches_glams_reference_transform() {
        // Parity with glam's own perspective divide (project_point3) for
        // anchors both inside and outside the frustum, under a tilted camera
        // pose: wherever the reference lands inside the clip volume, we must
        // agree; everywhere else we must report None.
        let mut camera = level_camera(20.0);
        camera.orbit(0.5, FRAC_PI_4 * 0.5);
        let view_proj = camera.view_proj(SIZE.x / SIZE.y);

        for x in [-30.0, -3.0, -0.5, 0.0, 0.5, 3.0, 30.0] {
            for y in [-20.0, -2.0, 0.0, 2.0, 20.0] {
                for z in [-40.0, -4.0, 0.0, 4.0, 40.0] {
                    let anchor = Vec3::new(x, y, z);
                    let screen = anchor_to_screen(&view_proj, SIZE, anchor);
                    let clip = view_proj * anchor.extend(1.0);
                    let expected = if clip.is_finite() && clip.w > 0.0 {
                        let ndc = clip.truncate() / clip.w;
                        ndc.is_finite()
                            && (-1.0..=1.0).contains(&ndc.x)
                            && (-1.0..=1.0).contains(&ndc.y)
                            && ndc.z > 0.0
                            && ndc.z <= 1.0
                    } else {
                        false
                    };
                    match (screen, expected) {
                        (Some(screen), true) => {
                            let ndc = clip.truncate() / clip.w;
                            let ref_screen = Vec2::new(
                                (ndc.x + 1.0) * 0.5 * SIZE.x,
                                (1.0 - ndc.y) * 0.5 * SIZE.y,
                            );
                            assert!(
                                screen.abs_diff_eq(ref_screen, 1e-2),
                                "screen mismatch for {anchor:?}: {screen:?} vs {ref_screen:?}"
                            );
                        }
                        (None, false) => {}
                        (Some(_), false) => panic!("outside anchor {anchor:?} must be None"),
                        (None, true) => panic!("inside anchor {anchor:?} must project"),
                    }
                }
            }
        }
    }

    /// A hand-built y-up `look_at` × wgpu-depth perspective matrix (the
    /// same formulas as camera.rs), used to pin poses and depth ranges the
    /// orbit camera cannot express exactly: an eye exactly on a reference
    /// plane or exactly behind the scene, and a far plane closer than the
    /// orbit far plane (orbit yaw angles never land exactly on π/2 or π in
    /// f32, which would leave a microscopic tilt in the pose).
    fn hand_view_proj(eye: Vec3, target: Vec3, near: f32, far: f32) -> Mat4 {
        let view = glam::camera::rh::view::look_at_mat4(eye, target, Vec3::Y);
        let proj =
            glam::camera::rh::proj::directx::perspective(FRAC_PI_3, SIZE.x / SIZE.y, near, far);
        proj * view
    }

    /// Re-project a world point through `view_proj` to its NDC coordinates
    /// (test helper: no culling, plain homogeneous divide).
    fn reproject_ndc(view_proj: &Mat4, point: Vec3) -> Vec3 {
        let clip = view_proj * point.extend(1.0);
        clip.truncate() / clip.w
    }

    #[test]
    fn center_pixel_ray_starts_on_the_near_plane_and_points_at_the_origin() {
        let camera = level_camera(10.0);
        let view_proj = camera.view_proj(SIZE.x / SIZE.y);

        // Level camera at (0, 0, 10) looking at the origin: the center ray
        // runs along the view axis, from the near plane (0.1 in front of
        // the eye) toward the far plane (1000 out — camera.rs formulas).
        let (near, dir) = screen_to_ray(&view_proj, SIZE, SIZE * 0.5).unwrap();
        assert!(
            near.abs_diff_eq(Vec3::new(0.0, 0.0, 9.9), 1e-3),
            "near point must sit on the near plane, got {near:?}"
        );
        assert!(
            dir.abs_diff_eq(Vec3::new(0.0, 0.0, -1.0), 1e-4),
            "center ray must point at the origin, got {dir:?}"
        );
        // Near/far normalization sanity: re-projecting the near point must
        // land on the near depth (z_ndc ≈ 0) and the far point (0.1 + 999.9
        // along the ray) on the far depth (z_ndc ≈ 1), with |dir| = 1.
        assert!((dir.length() - 1.0).abs() < 1e-4, "dir must be unit");
        let far = near + dir * 999.9;
        let near_ndc = reproject_ndc(&view_proj, near);
        let far_ndc = reproject_ndc(&view_proj, far);
        assert!(
            near_ndc.z.abs() < 1e-3,
            "near point must re-project to z_ndc = 0, got {near_ndc:?}"
        );
        assert!(
            (far_ndc.z - 1.0).abs() < 1e-3,
            "far point must re-project to z_ndc = 1, got {far_ndc:?}"
        );
    }

    #[test]
    fn edge_and_corner_pixel_rays_reproject_through_their_pixels() {
        let camera = level_camera(10.0);
        let view_proj = camera.view_proj(SIZE.x / SIZE.y);

        // Every pixel of the level camera sees the scene ahead: ray corners
        // and edges (the "viewport edge" degenerate cases) still form
        // forward, unit rays that pass through their own pixel.
        for pos in [
            Vec2::new(0.0, 0.0),
            Vec2::new(1920.0, 0.0),
            Vec2::new(0.0, 1080.0),
            Vec2::new(1920.0, 1080.0),
            Vec2::new(480.0, 300.0),
        ] {
            let (near, dir) = screen_to_ray(&view_proj, SIZE, pos).unwrap();
            assert!(
                (dir.length() - 1.0).abs() < 1e-4,
                "dir must be unit at {pos:?}, got {dir:?}"
            );
            assert!(dir.z < 0.0, "level-camera rays point forward, got {dir:?}");
            // The whole ray re-projects to its own pixel: check a point
            // mid-frustum (depth ~500) rather than the near/far endpoints,
            // which sit exactly on the depth-cull boundaries.
            let mid = near + dir * 500.0;
            let ndc = reproject_ndc(&view_proj, mid);
            let pixel = Vec2::new((ndc.x + 1.0) * 0.5 * SIZE.x, (1.0 - ndc.y) * 0.5 * SIZE.y);
            assert!(
                pixel.abs_diff_eq(pos, 0.05),
                "ray through {pos:?} must re-project to its own pixel, got {pixel:?}"
            );
            let near_ndc = reproject_ndc(&view_proj, near);
            assert!(
                near_ndc.z.abs() < 1e-3,
                "near z_ndc ≈ 0 at {pos:?}, got {near_ndc:?}"
            );
        }
    }

    #[test]
    fn rays_behind_the_eye_or_from_degenerate_inputs_are_none() {
        let camera = level_camera(10.0);
        let view_proj = camera.view_proj(SIZE.x / SIZE.y);

        // Hand-built finite matrix scaled by −1 (a negative-determinant
        // "camera"): every unprojection lands at or behind the eye (w ≤ 0).
        // No real perspective camera produces this — the check is
        // defensive, but the function must answer None, not panic.
        let mirrored = view_proj * Mat4::from_diagonal(Vec4::splat(-1.0));
        for pos in [Vec2::ZERO, SIZE * 0.5, SIZE] {
            assert_eq!(
                screen_to_ray(&mirrored, SIZE, pos),
                None,
                "behind-the-eye unprojection at {pos:?} must be None"
            );
        }

        // Non-finite positions and matrices (hand-built garbage).
        assert_eq!(
            screen_to_ray(&view_proj, SIZE, Vec2::new(f32::NAN, 10.0)),
            None
        );
        assert_eq!(
            screen_to_ray(&view_proj, SIZE, Vec2::new(f32::INFINITY, 10.0)),
            None
        );
        let garbage = Mat4::from_cols_array(&[f32::NAN; 16]);
        assert_eq!(screen_to_ray(&garbage, SIZE, SIZE * 0.5), None);
        // A zero matrix is finite but singular: its inverse is not.
        assert_eq!(screen_to_ray(&Mat4::ZERO, SIZE, SIZE * 0.5), None);

        // Non-positive or non-finite viewport sizes (hidden window).
        for size in [
            Vec2::ZERO,
            Vec2::new(-1.0, 100.0),
            Vec2::new(f32::NAN, 100.0),
            Vec2::new(100.0, f32::INFINITY),
        ] {
            assert_eq!(screen_to_ray(&view_proj, size, SIZE * 0.5), None);
        }
    }

    #[test]
    fn center_pixel_hits_the_origin_on_both_reference_planes() {
        let camera = level_camera(10.0);
        let view_proj = camera.view_proj(SIZE.x / SIZE.y);
        let center = SIZE * 0.5;

        // The level camera looks straight at the origin, so the center ray
        // hits z = 0 at the origin — and the camera-target plane coincides
        // with z = 0 here (both pass through the origin perpendicular to
        // the view direction), so both planes report the same hit.
        let ground = pointer_world(&view_proj, SIZE, center, WorldPlane::GroundZ0).unwrap();
        assert!(ground.abs_diff_eq(Vec3::ZERO, 1e-3), "got {ground:?}");
        let target =
            pointer_world(&view_proj, SIZE, center, WorldPlane::CameraTargetPlane).unwrap();
        assert!(target.abs_diff_eq(Vec3::ZERO, 1e-3), "got {target:?}");
    }

    #[test]
    fn viewport_pixels_hit_the_hand_derived_ground_points() {
        let camera = level_camera(10.0);
        let view_proj = camera.view_proj(SIZE.x / SIZE.y);

        // Pixels (1728, 972) and (192, 108) map to the exact NDC points
        // (0.8, −0.8) and (−0.8, 0.8) — clean interior positions (pixels on
        // the |ndc| = 1 boundary are avoided: their hits re-project back
        // onto the cull boundary, where f32 rounding decides inclusion).
        // The ground (z = 0) is 10 m in front of the eye, so the hit sits
        // at x = ndc.x·10·aspect·tan(30°) = ±0.8·10.264 ≈ ±8.211 and
        // y = ndc.y·10·tan(30°) = ∓0.8·5.774 ≈ ∓4.619
        // (m00 = 1/(aspect·tan 30°), m11 = 1/tan 30°).
        let corner = pointer_world(
            &view_proj,
            SIZE,
            Vec2::new(1728.0, 972.0),
            WorldPlane::GroundZ0,
        )
        .unwrap();
        assert!(
            corner.abs_diff_eq(Vec3::new(8.2112, -4.6188, 0.0), 1e-2),
            "ndc (0.8, −0.8) pixel must hit (8.2112, −4.6188, 0), got {corner:?}"
        );
        let top_left = pointer_world(
            &view_proj,
            SIZE,
            Vec2::new(192.0, 108.0),
            WorldPlane::GroundZ0,
        )
        .unwrap();
        assert!(
            top_left.abs_diff_eq(Vec3::new(-8.2112, 4.6188, 0.0), 1e-2),
            "ndc (−0.8, 0.8) pixel must hit (−8.2112, 4.6188, 0), got {top_left:?}"
        );
    }

    #[test]
    fn pointers_outside_the_viewport_are_rejected() {
        let camera = level_camera(10.0);
        let view_proj = camera.view_proj(SIZE.x / SIZE.y);

        // One pixel past the right edge (and one past the bottom): the ray
        // through the pointer exists, but its ground hit lies laterally
        // outside the frustum — the hit must be None. The edges also probe
        // the depth validation: both hits sit at depth ≈ 10, inside the
        // depth range, so only the lateral containment rejects them.
        assert_eq!(
            pointer_world(
                &view_proj,
                SIZE,
                Vec2::new(1921.0, 540.0),
                WorldPlane::GroundZ0
            ),
            None
        );
        assert_eq!(
            pointer_world(
                &view_proj,
                SIZE,
                Vec2::new(960.0, 1081.0),
                WorldPlane::GroundZ0
            ),
            None
        );
        // Non-finite pointer input.
        assert_eq!(
            pointer_world(
                &view_proj,
                SIZE,
                Vec2::new(f32::NAN, 540.0),
                WorldPlane::GroundZ0
            ),
            None
        );
    }

    #[test]
    fn ground_hits_beyond_the_far_plane_are_rejected() {
        // A level camera at distance 10 whose far plane sits at 5 (the
        // orbit far plane, 100·distance, always swallows the z = 0
        // crossing, so the hand-built range is needed): the origin ground
        // hit is 10 m out — beyond far — and must be rejected by the depth
        // check (re-projected z_ndc > 1) on both planes.
        let view_proj = hand_view_proj(Vec3::new(0.0, 0.0, 10.0), Vec3::ZERO, 0.1, 5.0);
        let center = SIZE * 0.5;
        assert_eq!(
            pointer_world(&view_proj, SIZE, center, WorldPlane::GroundZ0),
            None
        );
        assert_eq!(
            pointer_world(&view_proj, SIZE, center, WorldPlane::CameraTargetPlane),
            None
        );
        // Control: the same pose with a full-depth range does hit.
        let view_proj = hand_view_proj(Vec3::new(0.0, 0.0, 10.0), Vec3::ZERO, 0.1, 1000.0);
        let hit = pointer_world(&view_proj, SIZE, center, WorldPlane::GroundZ0).unwrap();
        assert!(hit.abs_diff_eq(Vec3::ZERO, 1e-3), "got {hit:?}");
    }

    #[test]
    fn cameras_on_or_below_the_ground_plane_never_hit_it() {
        // Yaw-90 pose (hand built — orbit yaw cannot land exactly on π/2 in
        // f32): the eye sits exactly on z = 0 and looks along −X, so the
        // center ray runs parallel to the ground plane (guarded against the
        // degenerate divide) and every other ray starts on the plane and
        // leaves it immediately — all ground hits are None.
        let view_proj = hand_view_proj(Vec3::new(10.0, 0.0, 0.0), Vec3::ZERO, 0.1, 1000.0);
        for pos in [SIZE * 0.5, Vec2::new(1920.0, 540.0), Vec2::ZERO, SIZE] {
            assert_eq!(
                pointer_world(&view_proj, SIZE, pos, WorldPlane::GroundZ0),
                None,
                "camera on the ground plane must not hit it at {pos:?}"
            );
        }

        // A camera below the ground plane looking away from it (hand built
        // target at z = −10) can never meet z = 0 in front either.
        let view_proj = hand_view_proj(
            Vec3::new(0.0, 0.0, -5.0),
            Vec3::new(0.0, 0.0, -10.0),
            0.1,
            1000.0,
        );
        for pos in [SIZE * 0.5, Vec2::new(960.0, 0.0), Vec2::new(0.0, 540.0)] {
            assert_eq!(
                pointer_world(&view_proj, SIZE, pos, WorldPlane::GroundZ0),
                None,
                "camera under the ground must not hit it at {pos:?}"
            );
        }

        // The camera-target plane is unaffected: it passes through the
        // origin, so the yaw-90 center pixel hits the origin, and pixel
        // (1728, 540) — exact ndc.x = 0.8, an interior pixel so the hit
        // re-projects cleanly inside the frustum — hits 0.8·10.264 ≈ 8.211
        // out on the world −Z axis (the view's right is world −Z for this
        // pose; hits scale linearly with ndc.x along the horizontal).
        let center = pointer_world(&view_proj, SIZE, SIZE * 0.5, WorldPlane::CameraTargetPlane);
        assert!(center.is_none(), "target plane lies behind this camera");
        let view_proj = hand_view_proj(Vec3::new(10.0, 0.0, 0.0), Vec3::ZERO, 0.1, 1000.0);
        let center =
            pointer_world(&view_proj, SIZE, SIZE * 0.5, WorldPlane::CameraTargetPlane).unwrap();
        assert!(center.abs_diff_eq(Vec3::ZERO, 1e-3), "got {center:?}");
        let right = pointer_world(
            &view_proj,
            SIZE,
            Vec2::new(1728.0, 540.0),
            WorldPlane::CameraTargetPlane,
        )
        .unwrap();
        assert!(
            right.abs_diff_eq(Vec3::new(0.0, 0.0, -8.2112), 1e-2),
            "ndc.x = 0.8 pixel must hit (0, 0, −8.2112), got {right:?}"
        );
    }

    /// Assert that a gizmo arm is visible and points along `(x, y)`.
    ///
    /// The normalized directions land within one ulp of the unit axes (glam
    /// normalizes as `v · length_recip`, which is not bit-exact), so the
    /// components are compared with a tiny tolerance rather than `==`.
    fn assert_arm(dir: (Vec2, bool), x: f32, y: f32) {
        assert!(dir.1, "axis must be visible");
        assert!(
            dir.0.abs_diff_eq(Vec2::new(x, y), 1e-6),
            "axis direction must be ({x}, {y}), got {:?}",
            dir.0
        );
    }

    #[test]
    fn level_camera_axes_point_right_and_up_and_the_z_axis_is_invisible() {
        let camera = level_camera(10.0);
        let view_proj = camera.view_proj(SIZE.x / SIZE.y);

        // +X maps to clip (+m00, 0, ·, 0): screen right. +Y to (0, m11, ·):
        // the y flip turns that into screen up. +Z is parallel to the view
        // direction — its direction column has zero xy (the axis projects
        // to a point at its anchor) — so it is reported invisible.
        let corner = Rect2 {
            min: Vec2::ZERO,
            max: SIZE,
        };
        let dirs = orientation_gizmo_dirs(&view_proj, SIZE, corner, 20.0);
        assert_arm(dirs[0], 1.0, 0.0);
        assert_arm(dirs[1], 0.0, -1.0);
        assert_eq!(dirs[2], (Vec2::ZERO, false));
        // `rect` and `len` are placement hints for the caller: the computed
        // directions must not depend on them.
        let other = orientation_gizmo_dirs(&view_proj, SIZE, corner, 40.0);
        assert_eq!(dirs, other);
    }

    #[test]
    fn default_pose_keeps_y_up_and_runs_z_down_the_screen() {
        // The default orbit pose (pitch 0.6 — the eye sits 10 m out on the
        // +Z side, 34° above the origin) is the pose the app opens with.
        // The +Y axis rises toward the eye: arm up. The +Z ground axis
        // runs toward the camera's shadow under the eye: arm down. Both
        // direction columns have w < 0 here (their far ends lie behind the
        // eye) and yet keep their visible trend — the pin for the rule that
        // the w sign never negates an arm (a blanket negation would swap
        // the Y and Z arms of this very pose).
        let camera = OrbitCamera::new(Vec3::ZERO);
        let view_proj = camera.view_proj(SIZE.x / SIZE.y);
        let corner = Rect2 {
            min: Vec2::ZERO,
            max: SIZE,
        };
        let dirs = orientation_gizmo_dirs(&view_proj, SIZE, corner, 20.0);
        assert_arm(dirs[0], 1.0, 0.0);
        assert_arm(dirs[1], 0.0, -1.0);
        assert_arm(dirs[2], 0.0, 1.0);
    }

    #[test]
    fn camera_behind_the_scene_mirrors_the_x_axis() {
        // Hand-built level camera at (0, 0, −10) looking at the origin
        // (yaw 180 — orbit yaw cannot land exactly on π in f32): the world
        // is seen from behind, so +X runs left on screen, +Y stays up, and
        // +Z recedes straight down the barrel of the view (invisible).
        let view_proj = hand_view_proj(Vec3::new(0.0, 0.0, -10.0), Vec3::ZERO, 0.1, 1000.0);
        let corner = Rect2 {
            min: Vec2::ZERO,
            max: SIZE,
        };
        let dirs = orientation_gizmo_dirs(&view_proj, SIZE, corner, 20.0);
        assert_arm(dirs[0], -1.0, 0.0);
        assert_arm(dirs[1], 0.0, -1.0);
        assert_eq!(dirs[2], (Vec2::ZERO, false));
    }

    #[test]
    fn axis_passing_near_the_eye_keeps_its_screen_trend() {
        // Yaw-45 level camera at 10·(sin 45°, 0, cos 45°): the +X axis
        // passes 7.07 m from the eye and its visible half runs from the
        // origin toward the eye's side — on screen to the right, exactly
        // along the +X column's xy. The column's w is negative here (the
        // axis's far end lies behind the eye), yet the arm must NOT be
        // negated: a flip would point it along the hidden −X half, against
        // what the scene shows. +Z runs to the view's left for this pose
        // (the view right is the +X/+Z bisector).
        let mut camera = level_camera(10.0);
        camera.orbit(FRAC_PI_4, 0.0);
        let view_proj = camera.view_proj(SIZE.x / SIZE.y);
        let corner = Rect2 {
            min: Vec2::ZERO,
            max: SIZE,
        };
        let dirs = orientation_gizmo_dirs(&view_proj, SIZE, corner, 20.0);
        assert_arm(dirs[0], 1.0, 0.0);
        assert_arm(dirs[1], 0.0, -1.0);
        assert_arm(dirs[2], -1.0, 0.0);
    }

    #[test]
    fn degenerate_inputs_report_all_axes_invisible() {
        let camera = level_camera(10.0);
        let view_proj = camera.view_proj(SIZE.x / SIZE.y);
        let corner = Rect2 {
            min: Vec2::ZERO,
            max: SIZE,
        };
        let none = (Vec2::ZERO, false);

        // Non-finite view-projection matrices (hand-built garbage).
        let garbage = Mat4::from_cols_array(&[f32::NAN; 16]);
        assert_eq!(
            orientation_gizmo_dirs(&garbage, SIZE, corner, 20.0),
            [none; 3]
        );
        // Non-positive or non-finite viewport sizes (hidden window).
        for size in [
            Vec2::ZERO,
            Vec2::new(-1.0, 100.0),
            Vec2::new(f32::NAN, 100.0),
        ] {
            assert_eq!(
                orientation_gizmo_dirs(&view_proj, size, corner, 20.0),
                [none; 3]
            );
        }
    }
}
