//! Point cloud display type: CPU data plus the optional GPU mesh handle.

use std::sync::Arc;

use crate::io;
use crate::render;

/// A point cloud display: the loaded CPU-side data together with the GPU
/// mesh uploaded for it, if any.
///
/// Upload is the renderer's job, not the display's: the display keeps the
/// data (for statistics, picking, and bounds) and the handle produced by
/// [`render::Renderer::upload`], so replacing a cloud is one assignment of
/// both fields; the old mesh drops and wgpu destroys its buffers after the
/// frame using them has finished.
pub struct PointCloud {
    /// CPU-side point data as loaded by `io`.
    pub data: io::PointCloudData,
    /// GPU representation of `data`, present once the renderer has uploaded
    /// it. `None` until the first upload and while a replacement is being
    /// loaded.
    pub mesh: Option<Arc<render::PointCloudMesh>>,
}

impl PointCloud {
    /// Wrap freshly loaded data. The mesh starts empty; the host uploads it
    /// through [`render::Renderer::upload`] and stores the returned handle
    /// in [`PointCloud::mesh`].
    pub fn from_data(data: io::PointCloudData) -> Self {
        PointCloud { data, mesh: None }
    }
}
