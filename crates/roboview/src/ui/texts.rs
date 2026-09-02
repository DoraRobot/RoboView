//! All user-facing copy, centralized in one module so the entire UI surface
//! can be reviewed — and later localized — from a single place (spec §6.5:
//! UI copy in English; the strings are the i18n-ready boundary, the widgets
//! never inline their own copy).
//!
//! Keep every user-facing string here. Panels, dialogs, and error messages
//! must reference the constants and helpers below instead of formatting
//! their own text.

use std::fmt::Display;

use roboview_core::io::PointCloudError;

/// Native window title (also the `eframe::run_native` application name).
pub const WINDOW_TITLE: &str = "RoboView";

/// Label of the top menu bar's first menu.
pub const MENU_FILE: &str = "File";

/// File menu item that opens the platform file picker.
pub const MENU_OPEN_POINT_CLOUD: &str = "Open point cloud file…";

/// Hint shown in the center of the viewport while no cloud is loaded.
pub const VIEWPORT_EMPTY_HINT: &str = "Open a point cloud file to begin (File → Open…)";

/// Shown in the viewport while a file loads in the background.
pub const VIEWPORT_LOADING: &str = "Loading point cloud…";

/// Title of the non-modal error notification window.
pub const ERROR_WINDOW_TITLE: &str = "Error";

/// Title of the native open-file dialog.
pub const FILE_DIALOG_TITLE: &str = "Open point cloud file";

/// Filter label of the native open-file dialog. The extension list stays in
/// the caller as the dotless ["ply", "pcd"] pair on purpose (the repository
/// check script `scripts/check_data_paths.sh` requires it that way).
pub const FILE_DIALOG_FILTER_NAME: &str = "Point cloud";

/// Notification when a chosen file cannot be loaded. Spec A7 keeps the
/// previously loaded cloud on screen — the scene is untouched on failure,
/// this message is the only reaction.
pub fn load_failed(file_name: &str, error: &PointCloudError) -> String {
    format!("Could not open \"{file_name}\":\n{error}")
}

/// Notification when the background loader could not be started at all
/// (thread spawn failure; the cloud is left untouched).
pub fn loader_start_failed(error: &impl Display) -> String {
    format!("Could not start the background loader: {error}")
}

/// Notification when the loader thread ended without reporting an outcome
/// (defensive: the loader always sends its result before exiting).
pub const LOADER_ABORTED: &str =
    "The background loader ended unexpectedly. The point cloud was not loaded.";
