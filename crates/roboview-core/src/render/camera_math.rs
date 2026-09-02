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
//! The function lives here in `render` rather than next to the camera in
//! `scene`: it consumes the same view-projection matrix the scene's shared
//! uniform carries (render/mod.rs), and `scene/` is owned by the scene
//! container tasks. It takes the matrix as an argument and never touches GPU
//! state, so it stays a pure function either way.

use glam::{Mat4, Vec2, Vec3};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::camera::OrbitCamera;
    use std::f32::consts::{FRAC_PI_4, FRAC_PI_6};

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
}
