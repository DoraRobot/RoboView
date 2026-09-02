//! PLY parser (ASCII and binary_little_endian subsets, see spec §7).
//!
//! Implemented in tasks T2/T3; this file is a placeholder.

use std::path::Path;

use super::{PointCloudData, PointCloudError};

pub fn load(_path: &Path) -> Result<PointCloudData, PointCloudError> {
    Err(PointCloudError::Malformed {
        reason: "PLY parsing not implemented yet".to_string(),
    })
}
