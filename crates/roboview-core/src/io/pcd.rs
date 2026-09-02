//! PCD parser (v0.7, ASCII and binary_little_endian subsets, see spec §7).
//!
//! Implemented in task T4; this file is a placeholder.

use std::path::Path;

use super::{PointCloudData, PointCloudError};

pub fn load(_path: &Path) -> Result<PointCloudData, PointCloudError> {
    Err(PointCloudError::Malformed {
        reason: "PCD parsing not implemented yet".to_string(),
    })
}
