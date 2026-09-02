//! Display types (display-types spec §7): the closed set of objects a scene
//! can hold — point clouds, meshes, paths, coordinate frames, and markers.
//!
//! The set is an enum, not a trait-object registry (plan §3.2): dispatch
//! over display kinds stays one compiler-checked `match`, and a trait-based
//! plugin registry is a future migration with a narrow surface.
//!
//! Each type lives in its own module with the shared shape of the point
//! cloud display (point_cloud.rs): CPU data plus an optional GPU handle
//! (`Option<Arc<…>>`). Uploading is the renderer's job, never the display's
//! — the host calls the pipeline upload entry points and stores the returned
//! handle in the display — and replacing a display drops its old handle,
//! freeing the buffers through wgpu's deferred destruction semantics. Every
//! display type implements `Drop` to report its removal to the render
//! handle ledger (spec §4 A6, `render/counters.rs`), gated on the object
//! actually carrying an uploaded handle so never-uploaded displays (and
//! overlay-only marker texts) leave the ledger untouched.
//!
//! Framing policy (spec §6): [`DisplayObject::bounds`] reports world-space
//! bounds for camera framing. Data-backed kinds report the bounds `io`
//! computed over their finite points; frames and markers report `None` —
//! the union framing (first-add and the Fit control) is driven by the
//! "data classes" (point clouds, meshes, paths) only. Frames and marker
//! arrows are anchored decorations whose extent is arbitrary UI input, not
//! scene content: letting a stray anchor pull the framing would fight the
//! camera policy, so they never participate in the union.

pub mod frame;
pub mod marker;
pub mod mesh;
pub mod path;
pub mod point_cloud;

use crate::io;
use crate::scene::HasBounds;

pub use frame::Frame;
pub use marker::{Marker, MarkerArrow, MarkerText};
pub use mesh::Mesh;
pub use path::Path;
pub use point_cloud::PointCloud;

/// The kind of a display object (spec §7): the type column of the object
/// list. The UI text for a kind lives in the app's `texts.rs`; this core
/// enum only distinguishes the closed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DisplayKind {
    /// A point cloud (PLY/PCD), the first feature's display type.
    PointCloud,
    /// A triangle mesh (OBJ); face-less files show as a scatter, but the
    /// kind stays a mesh.
    Mesh,
    /// One open polyline (CSV/XYZ).
    Path,
    /// A world-aligned XYZ coordinate frame (UI-added).
    Frame,
    /// A marker: an overlay text label or an arrow with a head (UI-added).
    Marker,
}

impl DisplayKind {
    /// Stable ASCII key of this kind: the handle-ledger row name (spec A6,
    /// `render/counters.rs`) and the log/debug label. Not UI text — display
    /// names are the app's job.
    pub fn as_str(self) -> &'static str {
        match self {
            DisplayKind::PointCloud => "point_cloud",
            DisplayKind::Mesh => "mesh",
            DisplayKind::Path => "path",
            DisplayKind::Frame => "frame",
            DisplayKind::Marker => "marker",
        }
    }
}

/// A display object of the scene: one of the closed set of display types
/// (plan §3.2), each wrapping its CPU data and optional GPU handle.
pub enum DisplayObject {
    /// A point cloud (spec §7 F0 lineage; first feature's type).
    PointCloud(PointCloud),
    /// A triangle mesh (spec §7 F1).
    Mesh(Mesh),
    /// An open polyline (spec §7 F2).
    Path(Path),
    /// A world-aligned coordinate frame (spec §7 F3).
    Frame(Frame),
    /// A marker: overlay text label or arrow (spec §7 F4).
    Marker(Marker),
}

impl DisplayObject {
    /// The kind of this object, for the object-list type column and the
    /// render dispatch.
    pub fn kind(&self) -> DisplayKind {
        match self {
            DisplayObject::PointCloud(_) => DisplayKind::PointCloud,
            DisplayObject::Mesh(_) => DisplayKind::Mesh,
            DisplayObject::Path(_) => DisplayKind::Path,
            DisplayObject::Frame(_) => DisplayKind::Frame,
            DisplayObject::Marker(_) => DisplayKind::Marker,
        }
    }

    /// World-space bounds of this object for camera framing (spec §6): the
    /// data-backed kinds report the bounds `io` computed over their finite
    /// points; frames and markers report `None` and never drive the union
    /// framing (see the module docs).
    pub fn bounds(&self) -> Option<io::Aabb> {
        match self {
            DisplayObject::PointCloud(cloud) => cloud.data.bounds,
            DisplayObject::Mesh(mesh) => mesh.data.bounds,
            DisplayObject::Path(path) => path.data.bounds,
            DisplayObject::Frame(_) | DisplayObject::Marker(_) => None,
        }
    }

    /// Wrap a loaded file (spec A1/A3 channel) into its display object. Only
    /// the three file-backed kinds exist here; frames and markers are added
    /// through UI parameters (spec §7 F3/F4).
    pub fn from_loaded(loaded: io::LoadedObject) -> Self {
        match loaded {
            io::LoadedObject::PointCloud(data) => {
                DisplayObject::PointCloud(PointCloud::from_data(data))
            }
            io::LoadedObject::Mesh(data) => DisplayObject::Mesh(Mesh::from_data(data)),
            io::LoadedObject::Path(data) => DisplayObject::Path(Path::from_data(data)),
        }
    }
}

impl From<io::LoadedObject> for DisplayObject {
    fn from(loaded: io::LoadedObject) -> Self {
        DisplayObject::from_loaded(loaded)
    }
}

/// The scene's payload capability (scene/mod.rs `HasBounds`): a display
/// object reports the box its kind frames by. Delegates to the inherent
/// [`DisplayObject::bounds`] — the fully qualified call below pins the
/// delegation so a future rename of either method cannot silently recurse.
impl HasBounds for DisplayObject {
    fn bounds(&self) -> Option<io::Aabb> {
        DisplayObject::bounds(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn cloud_data() -> io::PointCloudData {
        io::PointCloudData {
            positions: vec![Vec3::ZERO, Vec3::ONE],
            colors: None,
            bounds: Some(io::Aabb {
                min: Vec3::ZERO,
                max: Vec3::ONE,
            }),
            format: io::Format::Ply,
        }
    }

    fn mesh_data() -> io::MeshData {
        io::MeshData {
            positions: vec![Vec3::ZERO],
            normals: None,
            indices: None,
            bounds: None,
        }
    }

    fn path_data() -> io::PathData {
        io::PathData {
            points: vec![Vec3::ZERO, Vec3::X],
            bounds: Some(io::Aabb {
                min: Vec3::ZERO,
                max: Vec3::X,
            }),
        }
    }

    #[test]
    fn kinds_round_trip_through_as_str_without_aliases() {
        let kinds = [
            DisplayKind::PointCloud,
            DisplayKind::Mesh,
            DisplayKind::Path,
            DisplayKind::Frame,
            DisplayKind::Marker,
        ];
        let keys: Vec<&str> = kinds.iter().map(|kind| kind.as_str()).collect();
        assert_eq!(
            keys,
            ["point_cloud", "mesh", "path", "frame", "marker"],
            "ledger keys must stay stable (A6 rows)"
        );
        let unique: std::collections::HashSet<&str> = keys.iter().copied().collect();
        assert_eq!(
            unique.len(),
            kinds.len(),
            "each kind has its own ledger key"
        );
    }

    #[test]
    fn kind_matches_the_variant_constructed() {
        let objects = [
            DisplayObject::PointCloud(PointCloud::from_data(cloud_data())),
            DisplayObject::Mesh(Mesh::from_data(mesh_data())),
            DisplayObject::Path(Path::from_data(path_data())),
            DisplayObject::Frame(Frame::new(Vec3::ZERO, 1.0)),
            DisplayObject::Marker(Marker::arrow(Vec3::ZERO, Vec3::X)),
            DisplayObject::Marker(Marker::text(Vec3::ZERO, "label")),
        ];
        let kinds: Vec<DisplayKind> = objects.iter().map(DisplayObject::kind).collect();
        assert_eq!(
            kinds,
            [
                DisplayKind::PointCloud,
                DisplayKind::Mesh,
                DisplayKind::Path,
                DisplayKind::Frame,
                DisplayKind::Marker,
                DisplayKind::Marker,
            ]
        );
    }

    #[test]
    fn bounds_follow_the_data_kinds_and_frame_marker_report_none() {
        let cloud = DisplayObject::PointCloud(PointCloud::from_data(cloud_data()));
        assert_eq!(cloud.bounds().unwrap().max, Vec3::ONE);

        // No finite vertices → None, mirroring io (spec G1).
        let scatter = DisplayObject::Mesh(Mesh::from_data(mesh_data()));
        assert_eq!(scatter.bounds(), None);

        let path = DisplayObject::Path(Path::from_data(path_data()));
        assert_eq!(path.bounds().unwrap().max, Vec3::X);

        // Frames and markers never drive the union framing (module docs):
        // even fully finite geometry reports None.
        let frame = DisplayObject::Frame(Frame::new(Vec3::splat(100.0), 50.0));
        assert_eq!(frame.bounds(), None);
        let arrow = DisplayObject::Marker(Marker::arrow(Vec3::splat(100.0), Vec3::splat(200.0)));
        assert_eq!(arrow.bounds(), None);
        let text = DisplayObject::Marker(Marker::text(Vec3::splat(-50.0), "far away"));
        assert_eq!(text.bounds(), None);
    }

    #[test]
    fn from_loaded_wraps_each_file_kind() {
        let loaded = [
            io::LoadedObject::PointCloud(cloud_data()),
            io::LoadedObject::Mesh(mesh_data()),
            io::LoadedObject::Path(path_data()),
        ];
        let objects: Vec<DisplayObject> =
            loaded.into_iter().map(DisplayObject::from_loaded).collect();
        let kinds: Vec<DisplayKind> = objects.iter().map(DisplayObject::kind).collect();
        assert_eq!(
            kinds,
            [
                DisplayKind::PointCloud,
                DisplayKind::Mesh,
                DisplayKind::Path
            ]
        );
    }

    #[test]
    fn display_object_is_a_scene_bounds_payload() {
        // The scene's `bounds_union` needs `HasBounds` on the payload the
        // app stores — the delegation must behave like `bounds()`.
        fn via_scene_trait<H: HasBounds>(payload: &H) -> Option<io::Aabb> {
            HasBounds::bounds(payload)
        }
        let cloud = DisplayObject::PointCloud(PointCloud::from_data(cloud_data()));
        assert_eq!(via_scene_trait(&cloud), cloud.bounds());
        let frame = DisplayObject::Frame(Frame::new(Vec3::ZERO, 1.0));
        assert_eq!(via_scene_trait(&frame), None);
    }
}
