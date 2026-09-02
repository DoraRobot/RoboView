//! Path display type (display-types spec §7 F2): one open polyline (CSV/XYZ)
//! plus the optional GPU line mesh.

use std::sync::Arc;

use crate::io;
use crate::render;

use super::DisplayKind;

/// A path display: the loaded CPU-side polyline together with the GPU line
/// mesh uploaded for it, if any.
///
/// Upload is the renderer's job — the host calls
/// [`render::LinePipeline::upload_path`] and stores the returned handle
/// here. The upload splits the polyline into finite runs of at least two
/// points at the geometry level (render/line.rs), so non-finite file points
/// (spec G1) stay in [`Path::data`] while the drawn strips never touch them.
pub struct Path {
    /// CPU-side polyline as loaded by `io` (CSV/XYZ), in file order.
    pub data: io::PathData,
    /// GPU representation of `data`, present once the renderer has uploaded
    /// it. `None` until the first upload and while a replacement is being
    /// loaded.
    pub gpu: Option<Arc<render::LineMesh>>,
}

impl Path {
    /// Wrap freshly loaded data. The GPU handle starts empty; the host
    /// uploads it through [`render::LinePipeline::upload_path`] and stores
    /// the returned handle in [`Path::gpu`].
    pub fn from_data(data: io::PathData) -> Self {
        Path { data, gpu: None }
    }
}

/// Report removals to the render handle ledger (spec A6): a path display
/// that held an uploaded handle counts one destroyed event for the path
/// kind when it leaves the scene; never-uploaded paths leave the ledger
/// untouched.
impl Drop for Path {
    fn drop(&mut self) {
        if self.gpu.is_some() {
            render::counters::note_object_dropped(DisplayKind::Path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn from_data_stores_the_data_and_starts_without_a_gpu_handle() {
        let data = io::PathData {
            points: vec![Vec3::ZERO, Vec3::X, Vec3::Y],
            bounds: Some(io::Aabb {
                min: Vec3::ZERO,
                max: Vec3::ONE,
            }),
        };
        let path = Path::from_data(data);
        assert!(path.gpu.is_none());
        assert_eq!(path.data.point_count(), 3);
    }
}
