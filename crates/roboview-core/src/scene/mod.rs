//! Scene: the camera and the currently active display.
//!
//! A [`Scene`] owns the view state ([`camera::OrbitCamera`]) and at most one
//! display instance. The display type `D` is an opaque handle chosen by the
//! caller — the app instantiates `Scene` with its concrete display type —
//! which keeps this module free of any coupling to `displays`.
//!
//! Display replacement is a plain atomic swap: the previous instance is
//! dropped as the new one is stored. The failure path of loading (keep the
//! old display when a new file fails, spec point-cloud-viewport A7) is
//! orchestrated by the app, which simply does not call
//! [`Scene::set_display`] until the new data is ready.

pub mod camera;

use camera::OrbitCamera;

/// A scene: one camera plus at most one display instance.
///
/// `camera` is public so the app can swap in a freshly framed camera when a
/// new data set loads; `display` is `None` until the first successful load
/// (an empty viewport, spec A1).
#[derive(Debug, Clone)]
pub struct Scene<D> {
    /// Camera used to view the scene.
    pub camera: OrbitCamera,
    /// The active display instance, if any.
    pub display: Option<D>,
}

impl<D> Scene<D> {
    /// Create an empty scene (no display) viewed through `camera`.
    pub fn new(camera: OrbitCamera) -> Self {
        Self {
            camera,
            display: None,
        }
    }

    /// Make `display` the active instance, atomically replacing and dropping
    /// any previous one. Callers wanting keep-old-on-failure semantics
    /// (spec A7) must only call this after the new data is ready.
    pub fn set_display(&mut self, display: D) {
        self.display = Some(display);
    }

    /// Remove and drop the active display instance, leaving the scene empty.
    pub fn clear_display(&mut self) {
        self.display = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_swaps_and_clears_the_display() {
        let mut scene: Scene<u32> = Scene::new(OrbitCamera::new(glam::Vec3::ZERO));
        assert!(scene.display.is_none());

        scene.set_display(7);
        assert_eq!(scene.display, Some(7));

        // Replacing drops the previous instance (atomic swap, success path).
        scene.set_display(11);
        assert_eq!(scene.display, Some(11));

        scene.clear_display();
        assert!(scene.display.is_none());
    }
}
