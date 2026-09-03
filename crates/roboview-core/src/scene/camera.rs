//! Orbit camera: view state and the view/projection math built from it.
//!
//! The camera is a spherical-coordinate orbit rig — a fixed `target` point,
//! a `yaw` around the world +Y axis, a `pitch` elevation, and an eye-to-target
//! `distance` — with hard guards against degenerate output:
//!
//! - The elevation is clamped away from the poles so the eye-to-target
//!   direction never becomes parallel to world up, which keeps the view
//!   basis orthonormal and well conditioned at every pose.
//! - The distance is clamped to a positive range, and the near/far planes
//!   are derived from it (plus the extent of the framed content), so the
//!   target and the framed content stay inside the depth range at every
//!   zoom level (spec A6: focus always valid, no NaN degradation).
//! - Every interaction method applies its delta as a pure function of the
//!   current state and rolls the state back when the result would not be
//!   finite: no input event can corrupt the camera.
//!
//! Convention: right-handed coordinates, **Z up** — the mature-3D-tool
//! convention (Blender, glTF, ROS: the world Z axis is "up", the ground
//! plane is Z=0, axes letters X red / Y green / Z blue). The eye position
//! is derived from the spherical offset around `target`:
//!
//! ```text
//! eye = target + distance · (cos(pitch)·sin(yaw), −cos(pitch)·cos(yaw), sin(pitch))
//! ```
//!
//! `yaw = 0` puts the eye on the −Y side of the target; positive `pitch`
//! raises the eye above the Z=0 ground plane. The projection maps depth to
//! the wgpu/WebGPU NDC range `z ∈ [0, 1]` (near plane = 0, far plane = 1).

use std::f32::consts::{FRAC_PI_2, FRAC_PI_3};

use glam::{Mat4, Vec2, Vec3};

use crate::io::Aabb;

/// Default elevation of the eye above the target, in radians (~34°): high
/// enough to read the third dimension of a point cloud at first glance.
const DEFAULT_PITCH: f32 = 0.6;

/// Default eye-to-target distance (world units) used whenever no bounds are
/// available to frame (empty scene, all-invalid points, single-point cloud).
const DEFAULT_DISTANCE: f32 = 10.0;

/// Clamp limits for the eye-to-target distance (world units). The lower
/// clamp doubles as the near-plane floor — the eye may approach but never
/// pass the near plane — and the upper clamp bounds the far plane, keeping
/// every projection entry finite.
const MIN_DISTANCE: f32 = 1e-3;
const MAX_DISTANCE: f32 = 1e5;

/// Elevation clamp, applied symmetrically: the eye may approach but never
/// reach the poles. The 0.01 rad (~0.57°) margin keeps the angle between
/// the view direction and world up at least 0.01 rad, so the cross products
/// that build the view basis never collapse.
const PITCH_LIMIT: f32 = FRAC_PI_2 - 0.01;

/// Vertical field of view (60°), a typical interactive-viewer choice.
const FOV_Y: f32 = FRAC_PI_3;

/// The near plane sits this fraction of the camera distance in front of the
/// eye (see [`OrbitCamera::view_proj`] for the formula and rationale).
const NEAR_FRACTION: f32 = 0.01;

/// The far plane sits this multiple of the camera distance beyond the eye
/// (see [`OrbitCamera::view_proj`] for the formula and rationale).
const FAR_MULTIPLE: f32 = 100.0;

/// Margin added when framing content: `distance = 1.5·extent + margin`.
/// Keeps the framing distance well above the near-plane floor even for
/// microscopic extents (0.01 = 10× [`MIN_DISTANCE`]).
const FRAMING_MARGIN: f32 = 0.01;

/// A spherical-coordinates orbit camera (spec A6: focus always valid, no
/// NaN degradation — see the module docs for the full guarantee).
///
/// Interaction is incremental: [`Self::orbit`], [`Self::zoom`], and
/// [`Self::pan`] only ever transform the current pose and clamp the result;
/// construct a fresh pose for a new data set with [`Self::framing`] or
/// [`Self::new`].
#[derive(Debug, Clone, PartialEq)]
pub struct OrbitCamera {
    /// The point the camera orbits and always looks at. Only `pan` moves it;
    /// `orbit`/`zoom` keep it fixed.
    target: Vec3,
    /// Rotation around the world +Y axis, in radians. Unbounded (periodic).
    yaw: f32,
    /// Elevation of the eye above the target, in radians, clamped to
    /// `±PITCH_LIMIT` (never at the poles).
    pitch: f32,
    /// Eye-to-target distance, clamped to `[MIN_DISTANCE, MAX_DISTANCE]`.
    distance: f32,
    /// Largest dimension of the bounds used by the most recent
    /// [`Self::framing`] call; 0 when nothing (or only degenerate content)
    /// was framed. Interactions never change it, so the far plane keeps
    /// clearing the framed content at any zoom level (see
    /// [`Self::view_proj`] for why the projection needs it).
    framed_extent: f32,
}

impl OrbitCamera {
    /// A camera with the default pose (yaw 0, pitch `DEFAULT_PITCH`,
    /// distance `DEFAULT_DISTANCE`) aimed at `target`.
    pub fn new(target: Vec3) -> Self {
        Self {
            target,
            yaw: 0.0,
            pitch: DEFAULT_PITCH,
            distance: DEFAULT_DISTANCE,
            framed_extent: 0.0,
        }
    }

    /// Frame `bounds`, the box reported by `io::Aabb::from_points` (which
    /// already excludes non-finite points; `None` means no finite point,
    /// spec G1).
    ///
    /// - Some bounds: target = box center; distance = `1.5·largest_dimension`
    ///   plus `FRAMING_MARGIN`, so the eye sits comfortably outside the
    ///   content and a degenerate or all-identical box (extent 0, single
    ///   point) frames its center at `DEFAULT_DISTANCE` instead of
    ///   deriving a zero distance.
    /// - `None`, or bounds that are not finite (hand-built garbage): origin
    ///   and `DEFAULT_DISTANCE`.
    pub fn framing(bounds: Option<&Aabb>) -> Self {
        let Some(bounds) = bounds else {
            return Self::new(Vec3::ZERO);
        };
        if !bounds.min.is_finite() || !bounds.max.is_finite() {
            // Defensive: non-finite bounds are treated like missing bounds.
            return Self::new(Vec3::ZERO);
        }
        let extent = bounds.largest_dimension();
        let center = bounds.center();
        if !center.is_finite() {
            return Self::new(Vec3::ZERO);
        }
        if !extent.is_finite() || extent <= 0.0 {
            // Zero-size bounds (single point) or a non-finite derived extent
            // (overflow of the min/max difference): frame the valid center
            // at the default distance instead of deriving one from zero or
            // garbage.
            return Self::new(center);
        }
        let distance = (1.5 * extent + FRAMING_MARGIN).clamp(MIN_DISTANCE, MAX_DISTANCE);
        Self {
            target: center,
            yaw: 0.0,
            pitch: DEFAULT_PITCH,
            distance,
            framed_extent: extent,
        }
    }

    /// The point the camera orbits and looks at.
    pub fn target(&self) -> Vec3 {
        self.target
    }

    /// Yaw around the world +Z axis, in radians.
    pub fn yaw(&self) -> f32 {
        self.yaw
    }

    /// Elevation of the eye above the target, in radians.
    pub fn pitch(&self) -> f32 {
        self.pitch
    }

    /// Eye-to-target distance.
    pub fn distance(&self) -> f32 {
        self.distance
    }

    /// Eye position in world space for the current pose.
    fn eye(&self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        let eye_offset = Vec3::new(cos_pitch * sin_yaw, -cos_pitch * cos_yaw, sin_pitch);
        self.target + eye_offset * self.distance
    }

    /// Unit view axes in world space: `forward` points from the eye toward
    /// the target, `right` and `up` span the screen plane. The triple is the
    /// orthonormal, right-handed frame `up = right × forward` — the same
    /// basis `glam::camera::rh::view::look_at_mat4` builds internally — so
    /// pan directions and the view matrix always agree.
    ///
    /// Non-degenerate for every pose reachable through the public API: the
    /// pitch clamp keeps `|forward · Z| ≤ cos(0.01)`.
    fn view_axes(&self) -> (Vec3, Vec3, Vec3) {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        let forward = -Vec3::new(cos_pitch * sin_yaw, -cos_pitch * cos_yaw, sin_pitch);
        // World up is +Z (Z-up convention): the view basis spans the screen
        // plane with `up = right × forward`, right-handed.
        let right = forward.cross(Vec3::Z).normalize();
        let up = right.cross(forward);
        (forward, right, up)
    }

    /// Distance from the eye to the near plane for the current pose.
    ///
    /// `max(0.01·distance, MIN_DISTANCE)`: 1% of the camera distance in
    /// front of the eye, floored at the zoom clamp so the projection entries
    /// stay finite. The target sits ≥ 100× beyond the near plane at every
    /// zoom; when the eye is clamped to exactly `MIN_DISTANCE` the target
    /// lies on the near plane (depth 0), which is the designed stop — zoom
    /// cannot bring the eye closer.
    fn near_plane(&self) -> f32 {
        (self.distance * NEAR_FRACTION).max(MIN_DISTANCE)
    }

    /// Distance from the eye to the far plane for the current pose.
    ///
    /// `100·distance + framed_extent`: the 100× term alone keeps everything
    /// around the target far inside the frustum at typical zoom levels; the
    /// `framed_extent` term (set by the most recent [`Self::framing`])
    /// guarantees the far plane reaches at least `distance + extent/2`
    /// behind the eye at *any* zoom, so content the user framed is never
    /// clipped from behind even when the eye is clamped to its minimum
    /// distance. Content never framed (`framed_extent = 0`) has nothing to
    /// protect, so the plain 100× term suffices.
    ///
    /// The wgpu depth convention used here has no depth attachment in the
    /// current feature, so the depth range only affects clipping, not
    /// precision; a future depth-buffer pipeline must revisit the range.
    fn far_plane(&self) -> f32 {
        self.distance * FAR_MULTIPLE + self.framed_extent
    }

    /// Full view-projection matrix for the current pose and viewport
    /// `aspect` (width / height).
    ///
    /// The view matrix is a right-handed, y-up `look_at` built from an
    /// orthonormal basis ([`glam::camera::rh::view::look_at_mat4`]); the
    /// projection maps the right-handed view space to the wgpu/WebGPU NDC
    /// convention, depth `z ∈ [0, 1]` with `near → 0`, `far → 1`
    /// ([`glam::camera::rh::proj::directx::perspective`]). Combined, they
    /// project the target exactly onto the principal axis of the frustum for
    /// every pose.
    ///
    /// A non-finite or non-positive `aspect` (e.g. a minimized window) falls
    /// back to 1.0; together with the pose clamps and rollback guards this
    /// method always returns a finite matrix.
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let aspect = if aspect.is_finite() && aspect > 0.0 {
            aspect
        } else {
            1.0
        };
        // Z-up world: the view basis is built around world up = +Z.
        let view = glam::camera::rh::view::look_at_mat4(self.eye(), self.target, Vec3::Z);
        let proj = glam::camera::rh::proj::directx::perspective(
            FOV_Y,
            aspect,
            self.near_plane(),
            self.far_plane(),
        );
        proj * view
    }

    /// Orbit the camera by `delta_yaw` around the target (radians) and
    /// `delta_pitch` in elevation.
    ///
    /// Yaw is unbounded (it is periodic); pitch is clamped to
    /// `±(π/2 − 0.01)`. Non-finite deltas — or a finite delta that would
    /// overflow — leave the pose unchanged (rollback).
    pub fn orbit(&mut self, delta_yaw: f32, delta_pitch: f32) {
        let (old_yaw, old_pitch) = (self.yaw, self.pitch);
        self.yaw += delta_yaw;
        self.pitch += delta_pitch;
        if !self.yaw.is_finite() || !self.pitch.is_finite() {
            self.yaw = old_yaw;
            self.pitch = old_pitch;
            return;
        }
        self.pitch = self.pitch.clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// Zoom by a multiplicative step: `delta` is a log2 factor — each +1
    /// halves the eye-to-target distance (zoom in), each −1 doubles it.
    ///
    /// The distance is clamped to `[MIN_DISTANCE, MAX_DISTANCE]`; out-zoom
    /// saturates exactly at the upper clamp, so the exponential is never
    /// evaluated for a delta that could overflow. Non-finite deltas leave
    /// the distance unchanged (rollback).
    pub fn zoom(&mut self, delta: f32) {
        if !delta.is_finite() {
            return;
        }
        // `min_delta ≤ 0` is the out-zoom delta that would reach exactly
        // MAX_DISTANCE; anything below it saturates at the clamp.
        let min_delta = (self.distance / MAX_DISTANCE).log2();
        if delta <= min_delta {
            self.distance = MAX_DISTANCE;
            return;
        }
        let candidate = self.distance * 2f32.powf(-delta);
        self.distance = candidate.clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    /// Pan the target in the screen plane. `delta.x` pans along the
    /// camera-right axis and `delta.y` along the screen-up axis; one unit
    /// pans by a full eye-to-target distance (the pan rate therefore scales
    /// with the zoom, a fixed screen-space drag).
    ///
    /// The target is restored when the step would push it out of the finite
    /// range, and non-finite deltas are ignored entirely (rollback).
    pub fn pan(&mut self, delta: Vec2) {
        if !delta.is_finite() {
            return;
        }
        let (_, right, up) = self.view_axes();
        let step = (right * delta.x + up * delta.y) * self.distance;
        let old_target = self.target;
        self.target += step;
        if !self.target.is_finite() {
            self.target = old_target;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic approximate equality with a small absolute allowance,
    /// scaled by the magnitude of the expected value.
    fn close(actual: f32, expected: f32) -> bool {
        (actual - expected).abs() <= 1e-5 * expected.abs().max(1.0)
    }

    fn box_from(center: Vec3, half: f32) -> Aabb {
        Aabb {
            min: center - Vec3::splat(half),
            max: center + Vec3::splat(half),
        }
    }

    #[test]
    fn default_pose_aims_at_the_given_target() {
        let target = Vec3::new(1.0, -2.0, 3.0);
        let cam = OrbitCamera::new(target);
        assert_eq!(cam.target(), target);
        assert_eq!(cam.yaw(), 0.0);
        assert_eq!(cam.pitch(), DEFAULT_PITCH);
        assert_eq!(cam.distance(), DEFAULT_DISTANCE);
        assert!(cam.view_proj(1.0).is_finite());
    }

    #[test]
    fn framing_centers_and_fits_bounds() {
        let bounds = Aabb {
            min: Vec3::new(-2.0, -1.0, -3.0),
            max: Vec3::new(4.0, 5.0, 1.0),
        };
        let cam = OrbitCamera::framing(Some(&bounds));
        assert_eq!(cam.target(), Vec3::new(1.0, 2.0, -1.0));
        // Largest dimension is 6 (x); distance = 1.5·6 + FRAMING_MARGIN.
        assert!(close(cam.distance(), 1.5 * 6.0 + FRAMING_MARGIN));
    }

    #[test]
    fn framing_falls_back_for_degenerate_or_missing_bounds() {
        // Zero-size bounds (a single point): frame the point at the default
        // distance instead of deriving a zero distance.
        let point = Aabb {
            min: Vec3::splat(2.0),
            max: Vec3::splat(2.0),
        };
        let cam = OrbitCamera::framing(Some(&point));
        assert_eq!(cam.target(), Vec3::splat(2.0));
        assert_eq!(cam.distance(), DEFAULT_DISTANCE);

        // All-invalid points: no bounds at all (spec G1) -> origin + default.
        let cam = OrbitCamera::framing(None);
        assert_eq!(cam.target(), Vec3::ZERO);
        assert_eq!(cam.distance(), DEFAULT_DISTANCE);

        // Hand-built non-finite bounds are treated like missing bounds.
        let garbage = Aabb {
            min: Vec3::splat(f32::NEG_INFINITY),
            max: Vec3::splat(f32::INFINITY),
        };
        let cam = OrbitCamera::framing(Some(&garbage));
        assert_eq!(cam.target(), Vec3::ZERO);
        assert_eq!(cam.distance(), DEFAULT_DISTANCE);
    }

    #[test]
    fn pitch_is_clamped_before_the_poles() {
        let mut cam = OrbitCamera::new(Vec3::ZERO);
        cam.orbit(0.0, 100.0);
        assert!(close(cam.pitch(), PITCH_LIMIT));
        cam.orbit(0.0, 100.0);
        assert!(close(cam.pitch(), PITCH_LIMIT));
        cam.orbit(0.0, -500.0);
        assert!(close(cam.pitch(), -PITCH_LIMIT));
        // Interior motion between the limits is unclamped.
        cam.orbit(0.0, 0.3);
        assert!(close(cam.pitch(), -PITCH_LIMIT + 0.3));
    }

    #[test]
    fn zoom_is_exponential_and_clamped() {
        let mut cam = OrbitCamera::new(Vec3::ZERO);
        cam.zoom(3.0);
        assert!(close(cam.distance(), DEFAULT_DISTANCE / 8.0));
        cam.zoom(100.0);
        assert_eq!(cam.distance(), MIN_DISTANCE);
        cam.zoom(-1e30);
        assert_eq!(cam.distance(), MAX_DISTANCE);
        cam.zoom(0.0);
        assert_eq!(cam.distance(), MAX_DISTANCE);
    }

    #[test]
    fn pan_moves_the_target_in_the_screen_plane_scaled_by_distance() {
        let mut cam = OrbitCamera::new(Vec3::ZERO);
        cam.orbit(0.0, -DEFAULT_PITCH); // level view (pitch 0): right = +X, up = +Z
        let distance = cam.distance();
        cam.pan(Vec2::new(0.25, -0.5));
        assert!(
            cam.target()
                .abs_diff_eq(Vec3::new(0.25, 0.0, -0.5) * distance, 1e-6)
        );
        // Pan never changes zoom or orientation.
        assert_eq!(cam.distance(), distance);
        assert_eq!(cam.yaw(), 0.0);
    }

    #[test]
    fn pan_restores_the_target_when_the_step_would_overflow() {
        let mut cam = OrbitCamera::new(Vec3::ZERO);
        cam.zoom(-1000.0); // distance = MAX_DISTANCE
        let before = cam.clone();
        // 1e34 · MAX_DISTANCE exceeds f32::MAX.
        cam.pan(Vec2::new(1e34, 0.0));
        assert_eq!(cam, before);
    }

    #[test]
    fn non_finite_deltas_leave_the_pose_unchanged() {
        let mut cam = OrbitCamera::framing(Some(&box_from(Vec3::ZERO, 2.0)));
        cam.zoom(2.0); // give it some state beyond the defaults
        let before = cam.clone();

        cam.orbit(f32::NAN, 0.0);
        assert_eq!(cam, before);
        cam.orbit(0.0, f32::NAN);
        assert_eq!(cam, before);
        cam.orbit(f32::NAN, f32::INFINITY);
        assert_eq!(cam, before);
        cam.zoom(f32::NAN);
        assert_eq!(cam, before);
        cam.zoom(f32::INFINITY);
        assert_eq!(cam, before);
        cam.zoom(f32::NEG_INFINITY);
        assert_eq!(cam, before);
        cam.pan(Vec2::splat(f32::NAN));
        assert_eq!(cam, before);
        cam.pan(Vec2::new(0.0, f32::NEG_INFINITY));
        assert_eq!(cam, before);
    }

    #[test]
    fn view_axes_form_an_orthonormal_right_handed_frame() {
        let mut cam = OrbitCamera::framing(Some(&box_from(Vec3::ZERO, 2.0)));
        cam.orbit(0.7, 0.3);
        let (forward, right, up) = cam.view_axes();
        assert!(forward.abs_diff_eq(forward.normalize(), 1e-6));
        assert!(right.abs_diff_eq(right.normalize(), 1e-6));
        assert!(up.abs_diff_eq(up.normalize(), 1e-6));
        assert!(forward.dot(right).abs() < 1e-6);
        assert!(forward.dot(up).abs() < 1e-6);
        assert!(right.dot(up).abs() < 1e-6);
        // Right-handed y-up: up = right × forward, and the eye sits on the
        // forward axis at `distance`.
        assert!(up.abs_diff_eq(right.cross(forward), 1e-6));
        assert!(
            cam.eye()
                .abs_diff_eq(cam.target() - forward * cam.distance(), 1e-4)
        );
    }

    #[test]
    fn the_target_stays_centered_in_the_frustum_through_interactions() {
        let mut cam = OrbitCamera::framing(Some(&box_from(Vec3::ZERO, 10.0)));
        // A long interaction run: yaw sweeps several full turns and pitch
        // pins against both clamps.
        for _ in 0..48 {
            cam.orbit(std::f32::consts::FRAC_PI_6, 0.15);
            let vp = cam.view_proj(4.0 / 3.0);
            assert!(vp.is_finite());
            let ndc = vp.project_point3(cam.target());
            assert!(ndc.x.abs() < 1e-4, "target must stay on the principal axis");
            assert!(ndc.y.abs() < 1e-4, "target must stay on the principal axis");
            assert!(
                ndc.z > 0.0 && ndc.z < 1.0,
                "target must stay inside the depth range"
            );
        }
        // Both zoom clamps keep the camera finite and the target valid.
        cam.zoom(100.0); // clamps at MIN_DISTANCE: the target reaches depth 0
        let ndc = cam.view_proj(1.0).project_point3(cam.target());
        assert!(ndc.is_finite());
        assert!(ndc.z >= -1e-4 && ndc.z <= 1e-4);
        cam.zoom(-100.0); // clamps at MAX_DISTANCE
        let ndc = cam.view_proj(1.0).project_point3(cam.target());
        assert!(ndc.is_finite());
        assert!(ndc.z > 0.0 && ndc.z < 1.0);
    }

    #[test]
    fn view_is_never_flipped_vertically_or_horizontally() {
        // World +Z of the target must map to NDC up (Z-up convention) and
        // world +X (camera right at yaw = 0) to NDC right at every
        // elevation: any handedness or vertical-flip regression in the
        // view/projection pair fails here.
        let bounds = box_from(Vec3::ZERO, 1.0);
        for pitch_delta in [-1.5, -0.8, 0.0, 0.8, 1.5] {
            let mut cam = OrbitCamera::framing(Some(&bounds));
            cam.orbit(0.0, pitch_delta);
            let vp = cam.view_proj(1.0);
            let above = vp.project_point3(Vec3::Z * 0.1);
            assert!(above.y > 0.0, "above-target must stay above center");
            let right = vp.project_point3(Vec3::X * 0.1);
            assert!(right.x > 0.0, "right-of-target must stay right of center");
        }
    }

    #[test]
    fn view_proj_guards_non_finite_and_non_positive_aspect() {
        let cam = OrbitCamera::framing(Some(&box_from(Vec3::ZERO, 1.0)));
        for aspect in [0.0, -2.0, f32::NAN, f32::INFINITY, 0.75] {
            let vp = cam.view_proj(aspect);
            assert!(vp.is_finite());
            let ndc = vp.project_point3(cam.target());
            assert!(ndc.x.abs() < 1e-4 && ndc.y.abs() < 1e-4);
            assert!(ndc.z > 0.0 && ndc.z < 1.0);
        }
    }

    #[test]
    fn near_and_far_planes_follow_the_pose() {
        let mut cam = OrbitCamera::new(Vec3::ZERO);
        cam.zoom(5.0); // distance = 10 / 32
        assert!(close(cam.near_plane(), cam.distance() * 0.01));
        assert!(close(cam.far_plane(), cam.distance() * FAR_MULTIPLE));
        // Framed content extends the far plane (framed_extent = 20 here).
        let cam = OrbitCamera::framing(Some(&box_from(Vec3::ZERO, 10.0)));
        assert!(close(cam.far_plane(), cam.distance() * FAR_MULTIPLE + 20.0));
    }

    #[test]
    fn far_plane_keeps_framed_content_visible_at_full_zoom_in() {
        // A large box, fully zoomed in: the half-extent behind the target
        // must still clear the far plane (framed_extent is remembered).
        let mut cam = OrbitCamera::framing(Some(&box_from(Vec3::ZERO, 50.0)));
        cam.zoom(1000.0); // clamps at MIN_DISTANCE
        assert_eq!(cam.distance(), MIN_DISTANCE);
        assert!(cam.far_plane() >= cam.distance() + 50.0);
        // Without the remembered extent the far plane would only reach 0.1.
        assert!(cam.far_plane() > 100.0);
    }
}
