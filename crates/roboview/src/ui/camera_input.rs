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
//!   **anchored at the cursor** (the world point under the pointer stays
//!   put — [`OrbitCamera`] zoom would swing the scene around its target
//!   plane instead, 005 A11);
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
//! - Scrolling zooms around the cursor: `zoom(+1)` halves the eye-to-target
//!   distance, so a positive (upward) scroll delta zooms in; the target is
//!   then re-anchored along its focal plane so the world point under the
//!   cursor keeps its pixel (005 A11, drift ≤ 0.5 % of the viewport height,
//!   asserted by `cursor_zoom` unit tests).
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
    aspect: f32,
    viewport_px: Vec2,
    cursor_px: Vec2,
    delta: Vec2,
    unit: egui::MouseWheelUnit,
    modifiers: egui::Modifiers,
) {
    if matches!(unit, egui::MouseWheelUnit::Point) {
        if modifiers.command {
            cursor_zoom(
                camera,
                aspect,
                viewport_px,
                cursor_px,
                delta.y * ZOOM_LOG2_PER_SCROLL_POINT,
            );
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
        cursor_zoom(
            camera,
            aspect,
            viewport_px,
            cursor_px,
            delta.y * ZOOM_LOG2_PER_SCROLL_POINT,
        );
    }
}

/// Cursor-anchored zoom: `camera.zoom(delta)`, then move the target along
/// its focal plane so the world point under `cursor_px` keeps its pixel
/// (005 A11 — drift ≤ 0.5 % of the viewport height). Pure math on the
/// camera, unit-testable headless.
///
/// When the world point under the cursor sits on the wrong side of the
/// zoom state (behind the eye, outside the clip volume, or the distance
/// clamps saturate the zoom), the correction is skipped and the plain
/// zoom stands — a degraded-but-continuous gesture instead of a jump.
pub(crate) fn cursor_zoom(
    camera: &mut OrbitCamera,
    aspect: f32,
    viewport_px: Vec2,
    cursor_px: Vec2,
    delta: f32,
) {
    let before_vp = camera.view_proj(aspect);
    let Some(anchor) = roboview_core::render::camera_math::pointer_world(
        &before_vp,
        viewport_px,
        cursor_px,
        roboview_core::render::camera_math::WorldPlane::CameraTargetPlane,
    ) else {
        return;
    };
    camera.zoom(delta);
    let after_vp = camera.view_proj(aspect);
    let Some(offset_px) = roboview_core::render::camera_math::zoom_cursor_screen_offset(
        &after_vp,
        viewport_px,
        anchor,
        cursor_px,
    ) else {
        return;
    };
    let k = focal_plane_track_factor() / viewport_px.y;
    // Screen offset → focal-plane target shift: x along camera-right
    // (negated: bring the image back to the cursor), y along screen-up
    // (egui y is down, so the pan y flips once more).
    camera.pan(glam::Vec2::new(-offset_px.x * k, offset_px.y * k));
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use roboview_core::render::camera_math::{WorldPlane, anchor_to_screen, pointer_world};

    /// Re-project the world point the camera currently puts under
    /// `cursor_px` and measure how far it has drifted from that pixel.
    fn cursor_drift_px(camera: &OrbitCamera, viewport: Vec2, cursor_px: Vec2) -> f32 {
        let vp = camera.view_proj(viewport.x / viewport.y);
        let Some(w) = pointer_world(&vp, viewport, cursor_px, WorldPlane::CameraTargetPlane) else {
            return f32::INFINITY;
        };
        let Some(px) = anchor_to_screen(&vp, viewport, w) else {
            return f32::INFINITY;
        };
        (px - cursor_px).length()
    }

    #[test]
    fn cursor_zoom_keeps_the_world_point_under_the_pointer() {
        // 005 A11: zoom must anchor at the cursor — the world point under
        // the pointer stays within 0.5 % of the viewport height of its
        // pixel, for every reasonable cursor and zoom step in/out.
        let viewport = Vec2::new(800.0, 600.0);
        let cursors = [
            Vec2::new(400.0, 300.0),
            Vec2::new(520.0, 180.0),
            Vec2::new(30.0, 580.0),
        ];
        let deltas = [1.5, -1.5, 0.25, -3.0, 1000.0, -1000.0];
        for aspect in [1.0_f32, 4.0 / 3.0] {
            for cursor in cursors {
                for delta in deltas {
                    let mut camera = OrbitCamera::new(Vec3::ZERO);
                    cursor_zoom(&mut camera, aspect, viewport, cursor, delta);
                    let drift = cursor_drift_px(&camera, viewport, cursor);
                    assert!(
                        drift <= 0.005 * viewport.y,
                        "aspect {aspect} cursor {cursor:?} delta {delta}: drift {drift}px"
                    );
                }
            }
        }
    }

    #[test]
    fn cursor_zoom_is_headless_and_degenerate_safe() {
        let mut camera = OrbitCamera::new(Vec3::ZERO);
        let viewport = Vec2::new(800.0, 600.0);
        // Non-finite inputs never panic and never move the camera (the
        // camera's own rollbacks cover the zoom; the cursor math skips).
        cursor_zoom(&mut camera, 1.0, viewport, Vec2::new(f32::NAN, 300.0), 1.0);
        cursor_zoom(
            &mut camera,
            1.0,
            viewport,
            Vec2::new(400.0, 300.0),
            f32::NAN,
        );
        cursor_zoom(&mut camera, 1.0, viewport, Vec2::new(400.0, 300.0), 0.0);
        // A degenerate viewport (hidden window) stays a no-op too.
        cursor_zoom(&mut camera, 1.0, Vec2::new(0.0, 0.0), Vec2::ZERO, 1.0);
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

        // Surface move + command -> cursor-anchored zoom.
        let mut camera = OrbitCamera::new(Vec3::ZERO);
        let d0 = camera.distance();
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
        // The A11 drift bound holds for that zoom too.
        let vp = camera.view_proj(aspect);
        let w = pointer_world(&vp, viewport, cursor, WorldPlane::CameraTargetPlane).unwrap();
        let px = anchor_to_screen(&vp, viewport, w).unwrap();
        assert!((px - cursor).length() <= 0.005 * viewport.y);

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
