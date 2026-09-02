//! Frame display type (display-types spec §7 F3): a world-aligned XYZ
//! coordinate frame, UI-added.
//!
//! A frame stores only its geometry — origin and axis length. Orientation
//! is fixed to the world axes (spec §5: no frame pose editing) and axis
//! labels are an overlay the app paints (spec A4); core's part is the three
//! colored axis segments, which the renderer builds at upload time
//! ([`render::LinePipeline::upload_frame`]).

use std::sync::Arc;

use glam::Vec3;

use crate::render;

use super::DisplayKind;

/// A world-aligned coordinate frame: three axis segments from `origin`
/// along +X (red), +Y (green), and +Z (blue), each `length` long, plus the
/// optional GPU line mesh.
///
/// Upload is the renderer's job — the host calls
/// [`render::LinePipeline::upload_frame`] with this frame's origin and
/// length and stores the returned handle in [`Frame::gpu`]. Non-finite or
/// non-positive parameters produce empty geometry at upload (render/line.rs
/// module docs), so a frame the UI built with garbage draws nothing.
pub struct Frame {
    /// The shared corner of the three axes, in world space.
    pub origin: Vec3,
    /// Length of each axis segment, in world units.
    pub length: f32,
    /// GPU representation of the axes, present once the renderer has
    /// uploaded them. `None` before the first upload or while the frame
    /// parameters are being edited.
    pub gpu: Option<Arc<render::LineMesh>>,
}

impl Frame {
    /// A frame at `origin` with axis segments of `length` world units. The
    /// GPU handle starts empty; the host uploads it through
    /// [`render::LinePipeline::upload_frame`] and stores the returned
    /// handle in [`Frame::gpu`].
    pub fn new(origin: Vec3, length: f32) -> Self {
        Frame {
            origin,
            length,
            gpu: None,
        }
    }
}

/// Report removals to the render handle ledger (spec A6): a frame display
/// that held an uploaded handle counts one destroyed event for the frame
/// kind when it leaves the scene; never-uploaded frames leave the ledger
/// untouched.
impl Drop for Frame {
    fn drop(&mut self) {
        if self.gpu.is_some() {
            render::counters::note_object_dropped(DisplayKind::Frame);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_stores_the_geometry_and_starts_without_a_gpu_handle() {
        let frame = Frame::new(Vec3::new(1.0, -2.0, 3.0), 0.5);
        assert!(frame.gpu.is_none());
        assert_eq!(frame.origin, Vec3::new(1.0, -2.0, 3.0));
        assert_eq!(frame.length, 0.5);
    }
}
