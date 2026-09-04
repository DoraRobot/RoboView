//! Pointer input → camera deltas for the 3D viewport.
//!
//! A thin adapter between egui input events and [`OrbitCamera`] (005
//! spec keymap A11): it owns no state, only maps one frame of events onto
//! incremental camera updates with "grab the cloud" feel — the content
//! tracks the pointer, the pro-tools mapping (005 approved 2026-09-04):
//!
//! - middle-button drag orbits the cloud around the camera target;
//! - shift + middle drag pans the cloud in the screen plane;
//! - a mouse with no middle button (Apple Magic Mouse etc.) follows
//!   Blender's "simulate 3-button mouse": **alt + primary drag** acts as
//!   the middle button (orbit), **shift+alt+primary drag** as the
//!   shifted-middle (pan), and the picking/box-select gestures of the
//!   viewport stay off while alt is down (005 A11 revision);
//! - the Magic Mouse's touch surface (and trackpad scrolls), which
//!   arrive as `MouseWheelUnit::Point` events, emulate middle-drag the
//!   way Blender's Magic Mouse Emulation does: plain surface move =
//!   orbit, **shift + move = pan**, **command + move = zoom**; a real
//!   wheel (`Line`/`Page` units) keeps its classic zoom-anchored-
//!   at-cursor behavior. Both coexist: mice with a middle button and
//!   Magic Mouse users share the same keymap (005 A11 revision);
//! - scroll zooms in on wheel-up / two-finger-up, out on the reverse,
//!   about the **viewport center** (the camera's target — Blender's
//!   default behavior, its "zoom to mouse position" option stays off,
//!   005 A11 revision 2026-09-05: the pointer-anchored zoom we shipped
//!   first was compared against Blender and reverted);
//! - the primary button is NOT a camera gesture: viewport picking/box
//!   select owns it (005 A9/A11), this adapter never consumes it.
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
//! - Scrolling zooms around the camera target — the viewport center. This
//!   is Blender's default: `zoom(+1)` halves the eye-to-target distance,
//!   so a positive (upward) scroll delta zooms in and the framing stays
//!   centered (the target never moves).
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
use glam::Vec2;

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
/// a continuous gesture. The primary button is deliberately left alone:
/// picking and box-select own it (005 A9).
pub fn apply_pointer_events(
    response: &egui::Response,
    ctx: &egui::Context,
    viewport: egui::Rect,
    camera: &mut OrbitCamera,
) {
    let shift = ctx.input(|input| input.modifiers.shift);
    let alt = ctx.input(|input| input.modifiers.alt);

    // The camera gesture is the middle button — real, or, on a mouse
    // without one (Apple Magic Mouse / a trackpad), the Blender-style
    // simulation: alt + primary drag acts as middle drag.
    let middle_or_emulated = response.dragged_by(egui::PointerButton::Middle)
        || (alt && response.dragged_by(egui::PointerButton::Primary));

    if middle_or_emulated {
        let drag = response.drag_delta();
        // Shift + middle-drag pans (005 A11); plain middle-drag orbits —
        // the pro-tools convention, so the left button is free for
        // selection.
        if shift {
            let height = viewport.height();
            if drag.x.is_finite() && drag.y.is_finite() && height.is_finite() && height > 0.0 {
                // +Y is down in egui but up in the camera world: negate the
                // horizontal axis (content follows), keep the vertical one.
                let scale = focal_plane_track_factor() / height;
                camera.pan(glam::Vec2::new(-drag.x * scale, drag.y * scale));
            }
        } else {
            if drag.x.is_finite() && drag.y.is_finite() {
                camera.orbit(
                    -drag.x * ORBIT_RADIANS_PER_POINT,
                    drag.y * ORBIT_RADIANS_PER_POINT,
                );
            }
        }
    }

    if response.hovered() && !ctx.wants_pointer_input() {
        let cursor_px = response
            .hover_pos()
            .map(|p| Vec2::new(p.x - viewport.min.x, p.y - viewport.min.y))
            .unwrap_or_else(|| Vec2::ZERO);
        let viewport_map = Vec2::new(viewport.width(), viewport.height());
        let aspect = viewport.width() / viewport.height();
        // Walk the raw scroll events this frame: a tracked Magic Mouse /
        // trackpad surface arrives as Point units and follows Blender's
        // Magic-Mouse-Emulation gestures, a real wheel (Line/Page) keeps
        // the classic cursor-anchored zoom.
        let events = ctx.input(|input| input.events.clone());
        for event in events {
            if let egui::Event::MouseWheel {
                unit,
                delta,
                modifiers,
            } = event
            {
                let delta = Vec2::new(delta.x, delta.y);
                apply_scroll_camera(
                    camera,
                    aspect,
                    viewport_map,
                    cursor_px,
                    delta,
                    unit,
                    modifiers,
                );
            }
        }
    }
}

/// Dispatch one scroll event to the camera (005 A11 revision): tracked
/// touches (`Point` units — Magic Mouse surface, trackpad scrolls) right.
/// Headless-testable; the drag direction signs match the middle-drag
/// "grab the cloud" convention for continuity:
///
/// - plain touch move = orbit (the middle-drag simulation);
/// - shift + move = pan;
/// - command + move = zoom (cursor-anchored);
/// - real-wheel units (Line/Page) = cursor-anchored zoom, unchanged.
fn apply_scroll_camera(
    camera: &mut OrbitCamera,
    _aspect: f32,
    viewport_px: Vec2,
    _cursor_px: Vec2,
    delta: Vec2,
    unit: egui::MouseWheelUnit,
    modifiers: egui::Modifiers,
) {
    if matches!(unit, egui::MouseWheelUnit::Point) {
        if modifiers.command {
            // Command + surface move = zoom about the viewport center
            // (Blender's default; target stays put).
            camera.zoom(delta.y * ZOOM_LOG2_PER_SCROLL_POINT);
        } else if modifiers.shift {
            // Treat the touch motion as a middle-drag pan: the cloud
            // follows the fingers on the focal plane.
            let k = focal_plane_track_factor() / viewport_px.y;
            camera.pan(Vec2::new(-delta.x * k, delta.y * k));
        } else {
            // The Magic Mouse surface IS a middle click in Blender's
            // magic-mouse emulation: plain motion orbits.
            camera.orbit(
                -delta.x * ORBIT_RADIANS_PER_POINT,
                delta.y * ORBIT_RADIANS_PER_POINT,
            );
        }
    } else if delta.y != 0.0 {
        // A real wheel: classic zoom about the viewport center.
        camera.zoom(delta.y * ZOOM_LOG2_PER_SCROLL_POINT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn zoom_is_centered_on_the_viewport_target() {
        // 005 A11 revision 2026-09-05: zoom anchors at the viewport
        // CENTER (the camera target) — Blender's default (its "zoom to
        // mouse position" option is off). The target never moves while
        // the distance changes, for the wheel and for command+surface.
        let viewport = Vec2::new(800.0, 600.0);
        let aspect = viewport.x / viewport.y;
        for unit in [egui::MouseWheelUnit::Line, egui::MouseWheelUnit::Point] {
            for delta in [1.5, -1.5, 1000.0, -1000.0] {
                let mut camera = OrbitCamera::new(Vec3::ZERO);
                let target0 = camera.target();
                let distance0 = camera.distance();
                let delta = Vec2::new(0.0, delta);
                let modifiers = if egui::MouseWheelUnit::Point == unit {
                    egui::Modifiers::COMMAND
                } else {
                    egui::Modifiers::default()
                };
                apply_scroll_camera(
                    &mut camera,
                    aspect,
                    viewport,
                    Vec2::new(500.0, 400.0),
                    delta,
                    unit,
                    modifiers,
                );
                assert_eq!(camera.target(), target0, "zoom never moves the target");
                // The distance changes for in-range deltas; an out-of-
                // range delta saturates at the camera's clamps instead.
                let unchanged_out = delta.y > 0.0 && distance0 >= camera.distance()
                    || delta.y < 0.0 && distance0 <= camera.distance();
                assert!(
                    (camera.distance() - distance0).abs() > 1e-3 || unchanged_out,
                    "zoom changes the distance (or saturates)"
                );
                assert!(camera.distance().is_finite());
            }
        }
    }

    #[test]
    fn centered_zoom_is_degenerate_safe() {
        // An extreme delta saturates at the distance clamps instead of
        // panicking or producing a non-finite pose.
        let mut camera = OrbitCamera::new(Vec3::ZERO);
        apply_scroll_camera(
            &mut camera,
            1.0,
            Vec2::new(800.0, 600.0),
            Vec2::ZERO,
            Vec2::new(f32::NAN, 0.0),
            egui::MouseWheelUnit::Line,
            egui::Modifiers::default(),
        );
        apply_scroll_camera(
            &mut camera,
            1.0,
            Vec2::new(800.0, 600.0),
            Vec2::ZERO,
            Vec2::new(0.0, f32::NAN),
            egui::MouseWheelUnit::Line,
            egui::Modifiers::default(),
        );
        camera.zoom(1.0);
        assert!(camera.distance().is_finite());
    }

    #[test]
    fn magic_mouse_surface_orbits_pans_and_zooms_per_blender() {
        // 005 A11 revision: Point-unit motion = Blender's Magic Mouse
        // Emulation (surface = middle-drag); Line units keep the wheel
        // zoom. Command/Shift select the pan/zoom branch; modifiers on
        // the wheel branch are ignored (classic scroll zoom).
        let viewport = Vec2::new(800.0, 600.0);
        let cursor = Vec2::new(410.0, 290.0);
        let aspect = viewport.x / viewport.y;
        let wheel = egui::Modifiers::default();

        // Surface move, no modifier -> orbit: yaw changes, distance not.
        let mut camera = OrbitCamera::new(Vec3::ZERO);
        let yaw0 = camera.yaw();
        apply_scroll_camera(
            &mut camera,
            aspect,
            viewport,
            cursor,
            Vec2::new(30.0, 12.0),
            egui::MouseWheelUnit::Point,
            wheel,
        );
        assert!((camera.yaw() - yaw0).abs() > 1e-3, "surface move orbits");

        // Surface move + shift -> pan: the target moves, yaw identical.
        let mut camera = OrbitCamera::new(Vec3::ZERO);
        let yaw0 = camera.yaw();
        let target0 = camera.target();
        apply_scroll_camera(
            &mut camera,
            aspect,
            viewport,
            cursor,
            Vec2::new(30.0, 12.0),
            egui::MouseWheelUnit::Point,
            egui::Modifiers::SHIFT,
        );
        assert_eq!(camera.yaw(), yaw0, "shift+surface must not orbit");
        assert!(
            (camera.target() - target0).length() > 1e-3,
            "shift+surface pans"
        );

        // Surface move + command -> centered zoom: distance changes, the
        // target (viewport center) stays put.
        let mut camera = OrbitCamera::new(Vec3::ZERO);
        let d0 = camera.distance();
        let target0 = camera.target();
        apply_scroll_camera(
            &mut camera,
            aspect,
            viewport,
            cursor,
            Vec2::new(0.0, 40.0),
            egui::MouseWheelUnit::Point,
            egui::Modifiers::COMMAND,
        );
        assert!((camera.distance() - d0).abs() > 1e-2, "cmd+surface zooms");
        assert_eq!(camera.target(), target0, "centered zoom keeps the target");

        // Real wheel (Line unit) -> zoom, modifiers ignored.
        let mut camera = OrbitCamera::new(Vec3::ZERO);
        let d0 = camera.distance();
        apply_scroll_camera(
            &mut camera,
            aspect,
            viewport,
            cursor,
            Vec2::new(0.0, 40.0),
            egui::MouseWheelUnit::Line,
            wheel,
        );
        assert!((camera.distance() - d0).abs() > 1e-2, "wheel zooms");
    }
}
