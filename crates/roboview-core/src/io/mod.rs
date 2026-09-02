//! Point cloud loading: format dispatch, shared data model, error types.
//!
//! This module has no GUI or renderer dependencies (§2.4.1) and is fully
//! testable headless.

pub mod pcd;
pub mod ply;

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
