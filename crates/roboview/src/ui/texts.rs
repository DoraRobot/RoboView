//! All user-facing copy, centralized in one module so the entire UI surface
//! can be reviewed — and later localized — from a single place (display-types
//! spec §6.5: UI copy in English; the strings are the i18n-ready boundary,
//! the widgets never inline their own copy).
//!
//! Keep every user-facing string here. Panels, dialogs, and error messages
//! must reference the constants and helpers below instead of formatting
//! their own text. Copy that interpolates runtime values (a file name, a
//! sequence number) lives in the helper functions next to the constants.

use std::fmt::Display;

use roboview_core::displays::DisplayKind;
use roboview_core::io::{LoadError, ObjError, PathError, PointCloudError};

/// Native window title (also the `eframe::run_native` application name).
pub const WINDOW_TITLE: &str = "RoboView";

/// Label of the top menu bar's first menu.
pub const MENU_FILE: &str = "File";

/// File menu item opening the point cloud picker (PLY/PCD).
pub const MENU_OPEN_POINT_CLOUD: &str = "Open point cloud…";

/// File menu item opening the mesh picker (OBJ).
pub const MENU_OPEN_MESH: &str = "Open mesh (OBJ)…";

/// File menu item opening the path picker (CSV/XYZ).
pub const MENU_OPEN_PATH: &str = "Open path (CSV/XYZ)…";

/// Titles of the native open-file dialogs, one per file family.
pub const FILE_DIALOG_TITLE_POINT_CLOUD: &str = "Open point cloud file";
pub const FILE_DIALOG_TITLE_MESH: &str = "Open mesh file";
pub const FILE_DIALOG_TITLE_PATH: &str = "Open path file";

/// Filter labels of the native open-file dialogs. The dotless extension
/// lists stay in the caller (main.rs, next to `OpenKind`) on purpose: the
/// repository check script `scripts/check_data_paths.sh` (spec A9/A12)
/// treats a quoted `.<ext>` in production code as a hardcoded data path.
pub const FILE_DIALOG_FILTER_POINT_CLOUD: &str = "Point cloud";
pub const FILE_DIALOG_FILTER_MESH: &str = "Mesh (OBJ)";
pub const FILE_DIALOG_FILTER_PATH: &str = "Path (CSV/XYZ)";

/// Hint shown in the center of the viewport while the scene holds no object.
pub const VIEWPORT_EMPTY_HINT: &str =
    "The scene is empty. Open a file, or add a frame or marker to begin.";

/// Shown in the viewport while a file loads in the background.
pub const VIEWPORT_LOADING: &str = "Loading…";

/// Heading of the fixed objects panel (the left sidebar list).
pub const OBJECTS_PANEL_TITLE: &str = "Objects";

/// Fit button of the objects panel: reframes the camera to the union of
/// the visible objects (spec §6).
pub const OBJECTS_FIT: &str = "Fit";

/// Tooltip of the Fit button (enabled state).
pub const OBJECTS_FIT_TOOLTIP: &str = "Frame the camera to the visible objects";

/// Tooltip of the disabled Fit button: nothing measurable is visible
/// (frames and markers never participate in the bounds union).
pub const OBJECTS_FIT_TOOLTIP_DISABLED: &str =
    "Nothing measurable is visible yet — open a file first";

/// Objects panel entry opening the Add frame dialog (spec §7 F3, A4).
pub const OBJECTS_ADD_FRAME: &str = "Add frame…";

/// Objects panel entry opening the Add marker dialog (spec §7 F4, A5).
pub const OBJECTS_ADD_MARKER: &str = "Add marker…";

/// Hint of an empty objects list (the scene holds no object yet).
pub const OBJECTS_EMPTY_HINT: &str = "No objects yet. Open a file, or add a frame or marker above.";

/// Icon of the remove button of one object list row.
pub const OBJECTS_REMOVE: &str = "🗑";

/// Tooltip of the remove button.
pub const OBJECTS_REMOVE_TOOLTIP: &str = "Remove this object from the scene";

/// Kind column labels, one per display kind (spec §4 type column). The
/// core's own `DisplayKind::as_str` is a handle-ledger key, not UI text;
/// this mapping is the only place a kind becomes copy.
pub const KIND_POINT_CLOUD: &str = "Point cloud";
pub const KIND_MESH: &str = "Mesh";
pub const KIND_PATH: &str = "Path";
pub const KIND_FRAME: &str = "Frame";
pub const KIND_MARKER: &str = "Marker";

/// The object-list label of `kind`.
pub fn object_kind_label(kind: DisplayKind) -> &'static str {
    match kind {
        DisplayKind::PointCloud => KIND_POINT_CLOUD,
        DisplayKind::Mesh => KIND_MESH,
        DisplayKind::Path => KIND_PATH,
        DisplayKind::Frame => KIND_FRAME,
        DisplayKind::Marker => KIND_MARKER,
    }
}

/// Title of the non-modal Add frame window.
pub const ADD_FRAME_WINDOW_TITLE: &str = "Add frame";

/// Title of the non-modal Add marker window.
pub const ADD_MARKER_WINDOW_TITLE: &str = "Add marker";

/// Confirm button of both add-object windows.
pub const ADD_OBJECT_BUTTON: &str = "Add";

/// Label of the frame dialog's origin row (three XYZ drag values).
pub const ADD_FRAME_ORIGIN: &str = "Origin";

/// Label of the frame dialog's axis-length row.
pub const ADD_FRAME_LENGTH: &str = "Axis length";

/// Marker shape radio choices of the Add marker window (spec §7 F4).
pub const MARKER_SHAPE_TEXT: &str = "Text label";
pub const MARKER_SHAPE_ARROW: &str = "Arrow";

/// Label of the text marker's anchor row (three XYZ drag values).
pub const MARKER_ANCHOR: &str = "Anchor";

/// Label of the text marker's label-text field.
pub const MARKER_TEXT: &str = "Text";

/// Placeholder of the label-text field (the text itself is user data).
pub const MARKER_TEXT_HINT: &str = "Label text";

/// Labels of the arrow marker's endpoint rows.
pub const MARKER_START: &str = "Start";
pub const MARKER_END: &str = "End";

/// Coordinate axis letters: the per-axis labels painted at a frame's axis
/// tips (spec §7 F3 / A4) and the prefixes of the XYZ drag values.
pub const AXIS_X: &str = "X";
pub const AXIS_Y: &str = "Y";
pub const AXIS_Z: &str = "Z";

/// Generated display names of UI-added objects; `sequence` is the per-kind
/// add counter, so every name is unique within a session (the file-backed
/// objects are named by their file stem instead).
pub fn default_frame_name(sequence: u64) -> String {
    format!("Frame {sequence}")
}

/// Generated display name of a UI-added marker; see [`default_frame_name`].
pub fn default_marker_name(sequence: u64) -> String {
    format!("Marker {sequence}")
}

/// Title of the non-modal error notification window.
pub const ERROR_WINDOW_TITLE: &str = "Error";

/// Notification when a chosen file cannot be loaded. Spec A10 keeps every
/// previously loaded object on screen — the scene is untouched on failure,
/// this message is the only reaction. The detail line is a per-variant
/// mapping of the typed `io::LoadError` tree, so core's own error strings
/// never reach the UI.
pub fn load_failed(file_name: &str, error: &LoadError) -> String {
    format!(
        "Could not open \"{file_name}\":\n{}",
        describe_load_error(error)
    )
}

/// One readable line per `io::LoadError` variant family.
fn describe_load_error(error: &LoadError) -> String {
    match error {
        LoadError::UnsupportedFormat { extension } => unsupported_format(extension),
        LoadError::PointCloud(error) => describe_point_cloud_error(error),
        LoadError::Obj(error) => describe_obj_error(error),
        LoadError::Path(error) => describe_path_error(error),
    }
}

fn describe_point_cloud_error(error: &PointCloudError) -> String {
    match error {
        PointCloudError::UnsupportedFormat { extension } => unsupported_format(extension),
        PointCloudError::Malformed { reason } => format!("Malformed point cloud: {reason}"),
        PointCloudError::Io(error) => read_error(error),
    }
}

fn describe_obj_error(error: &ObjError) -> String {
    match error {
        ObjError::Malformed { line, reason } => {
            format!("Malformed OBJ file at line {line}: {reason}")
        }
        ObjError::Limit { reason } => {
            format!("The OBJ file exceeds the supported load limits: {reason}")
        }
        ObjError::Io(error) => read_error(error),
    }
}

fn describe_path_error(error: &PathError) -> String {
    match error {
        PathError::Malformed { line, reason } => {
            format!("Malformed path file at line {line}: {reason}")
        }
        PathError::TooFewPoints { points } => {
            format!("Path file has too few points ({points}): a polyline needs at least 2 points")
        }
        PathError::Limit { reason } => {
            format!("The path file exceeds the supported load limits: {reason}")
        }
        PathError::Io(error) => read_error(error),
    }
}

/// Shared copy of the three formats' unsupported-extension leaf.
fn unsupported_format(extension: &str) -> String {
    format!("Unsupported file format: \"{extension}\"")
}

/// Shared copy of the leaf that reports an OS-level file error.
fn read_error(error: &std::io::Error) -> String {
    format!("Could not read the file: {error}")
}

/// Notification when the background loader could not be started at all
/// (thread spawn failure; the scene is left untouched).
pub fn loader_start_failed(error: &impl Display) -> String {
    format!("Could not start the background loader: {error}")
}

/// Notification when the loader thread ended without reporting an outcome
/// (defensive: the loader always sends its result before exiting).
pub const LOADER_ABORTED: &str =
    "The background loader ended unexpectedly. The object was not added.";
