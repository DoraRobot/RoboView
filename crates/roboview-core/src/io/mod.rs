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

/// Loaded point cloud data: CPU side, renderer-independent.
#[derive(Debug, Clone)]
pub struct PointCloudData {
    pub positions: Vec<glam::Vec3>,
    /// Optional per-point color, same length as `positions`; sRGB 8-bit.
    pub colors: Option<Vec<Color>>,
    /// Bounding box of the valid (finite) points; `None` when no finite
    /// point exists (spec G1: non-finite points are kept, defended against).
    pub bounds: Option<glam::Aabb3>,
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
