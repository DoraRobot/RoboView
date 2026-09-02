//! Pointer input → camera deltas for the 3D viewport.
//!
//! A thin adapter between egui input events and [`OrbitCamera`] (plan §4,
//! §6): it owns no state, only maps one frame of events onto incremental
//! camera updates with "grab the cloud" feel — the content tracks the
//! pointer, the mapping used by OrbitControls-style viewers:
//!
//! - primary-button drag orbits the cloud around the camera target;
//! - scroll zooms in on wheel-up / two-finger-up, out on the reverse;
//! - middle-button drag pans the cloud in the screen plane.
//!
//! Sign conventions, from the camera's point of view (right-handed, +Y up,
//! screen right == camera right at rest; see the core camera docs):
//!
//! - Dragging the pointer right by `dx` (egui reports deltas in points, its
//!   +Y axis pointing down the screen) must rotate the grabbed surface to
//!   the right: the eye orbits left, i.e. the yaw delta is `-dx`. Dragging
//!   down (`dy > 0` in egui) must tip the cloud's top toward the viewer:
//!   the eye rises, i.e. the pitch delta is `+dy`. The vertical axis is
//!   flipped on the way in because egui's +Y points down while the camera's
//!   pitch grows upward.
//! - Scrolling zooms around the target: `zoom(+1)` halves the eye-to-target
//!   distance, so a positive (upward) scroll delta zooms in.
//! - Panning translates the camera target; to make the content follow a
//!   drag to the right the target moves one screen step to the left, and a
//!   downward drag lifts the target. [`OrbitCamera::pan`] already maps
//!   `delta.x` along the camera-right and `delta.y` along the screen-up
//!   axis, so the adapter only negates the horizontal axis and flips egui's
//!   vertical axis back to world-up.
//!
//! All deltas are guarded against non-finite values before reaching the
//! camera (the core rolls back internally as well, spec A6).

use eframe::egui;

use roboview_core::scene::camera::OrbitCamera;

/// Orbit sensitivity: radians of yaw/pitch per pointer point.
///
/// 0.01 rad ≈ 0.57° per point; a full-height drag on a 800 pt viewport
/// sweeps ≈ 8 rad — slightly over one full turn feels natural for a
/// grab-and-spin orbit, close to common three.js-style defaults.
pub const ORBIT_RADIANS_PER_POINT: f32 = 0.01;

/// Zoom sensitivity: log2 distance steps per scroll point.
///
/// One mouse-wheel notch is about 40 scroll points in egui (its native
/// `line_scroll_speed`), so this constant makes one notch exactly one log2
/// step: the eye-to-target distance halves (zoom in) or doubles (out) per
/// notch, a rate that is comfortable both for fine trackpad flicks and for
/// wheel users.
pub const ZOOM_LOG2_PER_SCROLL_POINT: f32 = 1.0 / 40.0;

/// Pan track factor: `2·tan(60°/2) = 2/√3 ≈ 1.1547`.
///
/// The core camera has a 60° vertical field of view, so the target plane
/// spans `2·tan(30°)·distance` world units across one viewport height; a
/// drag across the full viewport height therefore pans by exactly one
/// eye-to-target distance and the cloud tracks the pointer 1:1 on the
/// focal plane ([`OrbitCamera::pan`] pans one full distance per delta unit).
fn focal_plane_track_factor() -> f32 {
    2.0 / 3f32.sqrt()
}

/// Apply one frame of pointer events over the viewport rect to `camera`.
///
/// Call once per frame while the viewport is the panel under the pointer.
/// Drag deltas arrive per frame, so repeated calls integrate smoothly into
/// a continuous gesture.
pub fn apply_pointer_events(
    response: &egui::Response,
    ctx: &egui::Context,
    viewport: egui::Rect,
    camera: &mut OrbitCamera,
) {
    if response.dragged_by(egui::PointerButton::Primary) {
        let drag = response.drag_delta();
        if drag.x.is_finite() && drag.y.is_finite() {
            camera.orbit(
                -drag.x * ORBIT_RADIANS_PER_POINT,
                drag.y * ORBIT_RADIANS_PER_POINT,
            );
        }
    }

    if response.dragged_by(egui::PointerButton::Middle) {
        let drag = response.drag_delta();
        let height = viewport.height();
        if drag.x.is_finite() && drag.y.is_finite() && height.is_finite() && height > 0.0 {
            // +Y is down in egui but up in the camera world: negate the
            // horizontal axis (content follows), keep the vertical one.
            let scale = focal_plane_track_factor() / height;
            camera.pan(glam::Vec2::new(-drag.x * scale, drag.y * scale));
        }
    }

    if response.hovered() && !ctx.wants_pointer_input() {
        let scroll = ctx.input(|input| input.raw_scroll_delta.y);
        if scroll.is_finite() && scroll != 0.0 {
            camera.zoom(scroll * ZOOM_LOG2_PER_SCROLL_POINT);
        }
    }
}
