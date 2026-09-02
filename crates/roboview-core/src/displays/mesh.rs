//! Mesh display type (display-types spec §7 F1): OBJ data plus the optional
//! GPU mesh.
//!
//! The GPU side has two shapes ([`render::MeshGpu`]): a triangle mesh when
//! the file had `f` records, or — for face-less files, which the spec shows
//! as a scatter of points — the point cloud geometry shape drawn through
//! the point pipeline. Both are provisioned by [`render::MeshPipeline`],
//! never by this type.

use crate::io;
use crate::render;

use super::DisplayKind;

/// A mesh display: the loaded CPU-side data together with the GPU handle
/// uploaded for it, if any.
///
/// `gpu` holds whichever shape the file's faces imply ([`render::MeshGpu`]):
/// `Surfaces` for triangle data with CPU-computed face normals, `Scatter`
/// for face-less files (spec §7 F1). Upload is the renderer's job — the
/// host calls [`render::MeshPipeline::upload`] and stores the returned
/// handle here; replacing a mesh is one assignment of both fields, and the
/// old handle drops and frees its buffers through wgpu's deferred
/// destruction semantics.
pub struct Mesh {
    /// CPU-side mesh data as loaded by `io` (OBJ).
    pub data: io::MeshData,
    /// GPU representation of `data`, present once the renderer has uploaded
    /// it. `None` until the first upload and while a replacement is being
    /// loaded.
    pub gpu: Option<render::MeshGpu>,
}

impl Mesh {
    /// Wrap freshly loaded data. The GPU handle starts empty; the host
    /// uploads it through [`render::MeshPipeline::upload`] and stores the
    /// returned handle in [`Mesh::gpu`].
    pub fn from_data(data: io::MeshData) -> Self {
        Mesh { data, gpu: None }
    }
}

/// Report removals to the render handle ledger (spec A6): a mesh display
/// that held an uploaded handle counts one destroyed event for the mesh
/// kind when it leaves the scene; never-uploaded meshes leave the ledger
/// untouched (the created event was only ever recorded by the upload).
impl Drop for Mesh {
    fn drop(&mut self) {
        if self.gpu.is_some() {
            render::counters::note_object_dropped(DisplayKind::Mesh);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn data() -> io::MeshData {
        io::MeshData {
            positions: vec![Vec3::ZERO, Vec3::X, Vec3::Y],
            normals: None,
            indices: Some(vec![0, 1, 2]),
            bounds: Some(io::Aabb {
                min: Vec3::ZERO,
                max: Vec3::ONE,
            }),
        }
    }

    #[test]
    fn from_data_stores_the_data_and_starts_without_a_gpu_handle() {
        let mesh = Mesh::from_data(data());
        assert!(mesh.gpu.is_none());
        assert_eq!(mesh.data.face_count(), 1);
        assert_eq!(mesh.data.vertex_count(), 3);
        assert_eq!(mesh.data.bounds.unwrap().max, Vec3::ONE);
    }
}
