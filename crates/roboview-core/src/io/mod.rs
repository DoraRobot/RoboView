//! Data loading: format dispatch, shared data models, error types.
//!
//! Point clouds (PLY/PCD) keep the first-feature `load_point_cloud` entry;
//! display-type files (display-types spec §7) load through [`load_object`],
//! which dispatches `.obj` to the mesh parser and `.csv`/`.xyz` to the path
//! parser. Every parser is byte-level ASCII with no magic byte, so the
//! extension double check is the documented, weakened smoke level (spec §6).
//!
//! This module has no GUI or renderer dependencies (§2.4.1) and is fully
//! testable headless.

pub mod obj;
pub mod path_xyz;
pub mod pcd;
pub mod ply;

pub(crate) mod ascii_text;

use std::path::Path;

use thiserror::Error;

/// Point cloud file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Ply,
    Pcd,
}

/// Per-point color as stored in the file (sRGB bytes; conversion is the
/// renderer's job).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// Axis-aligned bounding box on the CPU side. Custom (not taken from the
/// math crate) so the spec G1 policy — non-finite coordinates are kept in
/// the data but excluded from bounds — is exactly one implementation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: glam::Vec3,
    pub max: glam::Vec3,
}

impl Aabb {
    /// Build the bounds of finite points only. Returns `None` when the
    /// slice contains no finite point (spec G1: camera falls back to a
    /// default framing).
    pub fn from_points(points: &[glam::Vec3]) -> Option<Aabb> {
        let mut min = glam::Vec3::splat(f32::INFINITY);
        let mut max = glam::Vec3::splat(f32::NEG_INFINITY);
        let mut finite = false;
        for p in points {
            if p.is_finite() {
                finite = true;
                min = min.min(*p);
                max = max.max(*p);
            }
        }
        finite.then_some(Aabb { min, max })
    }

    pub fn center(&self) -> glam::Vec3 {
        (self.min + self.max) * 0.5
    }

    /// Largest dimension; 0 for degenerate boxes (single point / plane).
    pub fn largest_dimension(&self) -> f32 {
        let extent = self.max - self.min;
        extent.x.max(extent.y).max(extent.z)
    }
}

/// Loaded point cloud data: CPU side, renderer-independent.
#[derive(Debug, Clone)]
pub struct PointCloudData {
    pub positions: Vec<glam::Vec3>,
    /// Optional per-point color, same length as `positions`; sRGB 8-bit.
    pub colors: Option<Vec<Color>>,
    /// Bounding box of the valid (finite) points; `None` when no finite
    /// point exists (spec G1: non-finite points are kept, defended against).
    pub bounds: Option<Aabb>,
    pub format: Format,
}

impl PointCloudData {
    pub fn point_count(&self) -> usize {
        self.positions.len()
    }
}

/// Loaded triangle-mesh data (OBJ, display-types spec §7 F1): CPU side,
/// renderer-independent.
#[derive(Debug, Clone)]
pub struct MeshData {
    /// Vertices from the `v` records, in file order.
    pub positions: Vec<glam::Vec3>,
    /// The file's `vn` records, in file order, when the file has any.
    /// Parsed and range-validated by the parser but deliberately not used
    /// in shading (spec §6: face normals are computed CPU-side).
    pub normals: Option<Vec<glam::Vec3>>,
    /// Triangle corner indices into `positions` (0-based, 1-based OBJ
    /// references minus one), `3 * face_count` entries. `None` when the
    /// file has no `f` records: the whole file is displayed as a scatter
    /// of points (spec §7 F1).
    pub indices: Option<Vec<u32>>,
    /// Bounding box of the valid (finite) vertices; `None` when no finite
    /// vertex exists (spec G1: non-finite vertices are kept, defended
    /// against).
    pub bounds: Option<Aabb>,
}

impl MeshData {
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn face_count(&self) -> usize {
        self.indices.as_ref().map_or(0, |indices| indices.len() / 3)
    }
}

/// Loaded path data (CSV/XYZ, display-types spec §7 F2): one open polyline
/// through the file's points, in file order — no closing or multi-segment
/// semantics (a first/last coincidence is not a closure).
#[derive(Debug, Clone)]
pub struct PathData {
    /// Polyline vertices in file order.
    pub points: Vec<glam::Vec3>,
    /// Bounding box of the valid (finite) points; `None` when no finite
    /// point exists (spec G1: non-finite points are kept, defended
    /// against).
    pub bounds: Option<Aabb>,
}

impl PathData {
    pub fn point_count(&self) -> usize {
        self.points.len()
    }
}

/// Typed loading errors (CONSTITUTION §2.5; never swallowed silently).
#[derive(Debug, Error)]
pub enum PointCloudError {
    #[error("unsupported file format: {extension}")]
    UnsupportedFormat { extension: String },
    #[error("malformed point cloud: {reason}")]
    Malformed { reason: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// OBJ loading errors (display-types spec §7 F1). Line numbers in
/// [`ObjError::Malformed`] are 1-based physical file lines.
#[derive(Debug, Error)]
pub enum ObjError {
    #[error("malformed OBJ at line {line}: {reason}")]
    Malformed { line: usize, reason: String },
    /// A record count or index space that cannot be represented on this
    /// platform; raised by the counting pre-validation (spec §6) before
    /// any allocation happens.
    #[error("OBJ exceeds the supported load limits: {reason}")]
    Limit { reason: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Path file (CSV/XYZ) loading errors (display-types spec §7 F2). Line
/// numbers in [`PathError::Malformed`] are 1-based physical file lines.
#[derive(Debug, Error)]
pub enum PathError {
    #[error("malformed path file at line {line}: {reason}")]
    Malformed { line: usize, reason: String },
    #[error("path file has too few points ({points}): a polyline needs at least 2 points")]
    TooFewPoints { points: usize },
    /// Raised by the counting pre-validation (spec §6) before any
    /// allocation happens.
    #[error("path file exceeds the supported load limits: {reason}")]
    Limit { reason: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Load a point cloud: extension + header double check before parsing
/// (spec §7, acceptance A8).
pub fn load_point_cloud(path: &Path) -> Result<PointCloudData, PointCloudError> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "ply" => ply::load(path),
        "pcd" => pcd::load(path),
        other => Err(PointCloudError::UnsupportedFormat {
            extension: other.to_string(),
        }),
    }
}

/// A loaded display object (display-types spec §7): one file, one object.
#[derive(Debug, Clone)]
pub enum LoadedObject {
    /// A point cloud (PLY/PCD), via [`load_point_cloud`].
    PointCloud(PointCloudData),
    /// A mesh (OBJ). When the file has no faces the mesh data is a scatter:
    /// `MeshData::indices` is `None` (spec §7 F1).
    Mesh(MeshData),
    /// A path as one open polyline (CSV/XYZ).
    Path(PathData),
}

impl LoadedObject {
    /// Bounding box over the object's finite data points (spec G1), for
    /// framing; `None` when nothing finite is present.
    pub fn bounds(&self) -> Option<Aabb> {
        match self {
            LoadedObject::PointCloud(data) => data.bounds,
            LoadedObject::Mesh(data) => data.bounds,
            LoadedObject::Path(data) => data.bounds,
        }
    }
}

/// Error of the unified [`load_object`] dispatch: one variant per format
/// family, converted with `From` from the family error types.
#[derive(Debug, Error)]
pub enum LoadError {
    #[error("unsupported file format: {extension}")]
    UnsupportedFormat { extension: String },
    #[error(transparent)]
    PointCloud(#[from] PointCloudError),
    #[error(transparent)]
    Obj(#[from] ObjError),
    #[error(transparent)]
    Path(#[from] PathError),
}

/// Load one display object, dispatching on the file extension (spec A10/A12
/// guard: `.ply`/`.pcd`/`.obj`/`.csv`/`.xyz`). Formats without a magic byte
/// rely on this extension check plus the per-format smoke rules (spec §6).
pub fn load_object(path: &Path) -> Result<LoadedObject, LoadError> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "ply" | "pcd" => Ok(LoadedObject::PointCloud(load_point_cloud(path)?)),
        "obj" => Ok(LoadedObject::Mesh(obj::load(path)?)),
        "csv" | "xyz" => Ok(LoadedObject::Path(path_xyz::load(path)?)),
        other => Err(LoadError::UnsupportedFormat {
            extension: other.to_string(),
        }),
    }
}
