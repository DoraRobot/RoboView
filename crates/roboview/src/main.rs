//! RoboView desktop shell: an eframe/egui application hosting the 3D
//! scene viewport.
//!
//! Opening files appends to the scene (display-types spec §1 replaces the
//! first feature's single-slot swap, `docs/specs/point-cloud-viewport/`):
//! each successful load adds one object named by its file stem, the first
//! object of an empty scene frames the scene bounds, later adds never move
//! the camera (spec §6). Failure keeps every existing object and surfaces
//! a readable error (spec A10). Frames and markers are added through the
//! objects panel dialogs instead (spec §7 F3/F4).
//!
//! Layout of the frame (`App::update`, display-types plan §3.4):
//!
//! 1. Align the renderer and the scene pipeline family with eframe's wgpu
//!    render state (created once it exists, rebuilt when the target format
//!    changes).
//! 2. Poll the background loader channel; a finished file becomes pending
//!    data, a failure keeps the current objects and shows an error window.
//! 3. Install pending data once a renderer exists (single-flight; upload
//!    and scene add happen here, on the UI thread).
//! 4. Draw the menu bar (File → Open point cloud / mesh / path), the left
//!    objects panel (Fit, Add frame / marker, the object list), and the
//!    central viewport panel filling the rest.
//! 5. Draw the non-modal error window and the two add dialogs on top.
//!
//! Loading never blocks the UI thread: the dialog choice spawns a worker
//! thread that parses the file and reports back over an mpsc channel; the
//! worker asks egui for a repaint when it finishes.
//!
//! The three open entries of the File menu are one channel into the same
//! `io::load_object` dispatch, distinguished only by the dialog filter
//! (spec A12 guard: the dotless extension lists stay here, next to
//! `OpenKind`, so the repository check script finds them).

mod ui;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use eframe::egui;

use roboview_core::io::{self, LoadError, LoadedObject};

use ui::objects_panel::{self, AddFrameDialog, AddMarkerDialog};
use ui::texts;
use ui::viewport::{self, ViewportState};

/// The three file families the File menu opens. Each maps to the dialog
/// spec of one `io::load_object` extension family (spec §7): point clouds
/// (PLY/PCD), meshes (OBJ), paths (CSV/XYZ).
#[derive(Debug, Clone, Copy)]
enum OpenKind {
    PointCloud,
    Mesh,
    Path,
}

impl OpenKind {
    /// The File menu label of this family.
    fn menu_label(self) -> &'static str {
        match self {
            OpenKind::PointCloud => texts::MENU_OPEN_POINT_CLOUD,
            OpenKind::Mesh => texts::MENU_OPEN_MESH,
            OpenKind::Path => texts::MENU_OPEN_PATH,
        }
    }

    /// Native dialog spec of this family: (title, filter label, dotless
    /// extension list). The lists never carry a dot: the repository check
    /// script `scripts/check_data_paths.sh` (spec A12) treats a quoted
    /// `.<ext>` in production code as a hardcoded data path.
    fn dialog_spec(self) -> (&'static str, &'static str, &'static [&'static str]) {
        match self {
            OpenKind::PointCloud => (
                texts::FILE_DIALOG_TITLE_POINT_CLOUD,
                texts::FILE_DIALOG_FILTER_POINT_CLOUD,
                &["ply", "pcd"],
            ),
            OpenKind::Mesh => (
                texts::FILE_DIALOG_TITLE_MESH,
                texts::FILE_DIALOG_FILTER_MESH,
                &["obj"],
            ),
            OpenKind::Path => (
                texts::FILE_DIALOG_TITLE_PATH,
                texts::FILE_DIALOG_FILTER_PATH,
                &["csv", "xyz"],
            ),
        }
    }
}

/// Result of one background load, sent over the loader channel.
enum LoadOutcome {
    /// The file parsed; the display object and its display name (file
    /// stem) are ready for the scene add on the UI thread.
    Loaded {
        /// Display name of the new scene object (file stem, display-types
        /// plan §3.1).
        name: String,
        object: LoadedObject,
    },
    /// The file could not be loaded; the scene stays untouched (spec A10).
    Failed {
        /// File name for the human-readable error window.
        file: String,
        error: LoadError,
    },
}

/// A successfully parsed file waiting for renderer-side install.
struct PendingObject {
    /// Display name of the object to add (file stem).
    name: String,
    /// Parsed display data, ready to upload.
    object: LoadedObject,
}

/// The RoboView application: window shell + viewport state.
struct RoboViewApp {
    /// Scene, renderer, and per-frame rect, shared with the wgpu paint
    /// callback registered by the viewport panel each frame.
    viewport: Arc<Mutex<ViewportState>>,
    /// Successfully loaded data waiting for a renderer. Renderer creation
    /// depends on eframe's wgpu render state being available, which it is
    /// from the first frame on native; the fallback keeps this slot for a
    /// frame where it is not.
    pending_object: Option<PendingObject>,
    /// In-flight background load. `Some` also drives the single-flight
    /// guard that disables the Open menu items while a load runs.
    load: Option<Receiver<LoadOutcome>>,
    /// Message of the non-modal error window; `None` hides the window.
    error_message: Option<String>,
    /// The Add frame dialog of the objects panel (spec §7 F3).
    add_frame_dialog: AddFrameDialog,
    /// The Add marker dialog of the objects panel (spec §7 F4).
    add_marker_dialog: AddMarkerDialog,
}

impl RoboViewApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Dark theme by default: point colors read best on a dark canvas.
        // Theme switching is out of scope (spec §5 non-goals).
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        Self {
            viewport: Arc::new(Mutex::new(ViewportState::new())),
            pending_object: None,
            load: None,
            error_message: None,
            add_frame_dialog: AddFrameDialog::new(),
            add_marker_dialog: AddMarkerDialog::new(),
        }
    }

    /// Open the native file dialog for one file family and start a
    /// background load of the chosen file. Blocking by design (rfd's
    /// modal dialog); the menu disables itself while a load is in flight,
    /// so at most one worker exists at a time.
    fn open_file_dialog(&mut self, ctx: &egui::Context, kind: OpenKind) {
        let (title, filter, extensions) = kind.dialog_spec();
        let Some(path) = rfd::FileDialog::new()
            .set_title(title)
            .add_filter(filter, extensions)
            .pick_file()
        else {
            return; // Cancelled: nothing to do.
        };
        self.start_background_load(ctx, path, kind);
    }

    /// Spawn the parse worker for `path`. On success the receiver replaces
    /// any finished slot; on spawn failure the user gets the error window.
    fn start_background_load(&mut self, ctx: &egui::Context, path: PathBuf, kind: OpenKind) {
        tracing::info!(file = %path.display(), ?kind, "starting background load");
        let (sender, receiver) = mpsc::channel::<LoadOutcome>();
        let context = ctx.clone();
        let file = path.to_string_lossy().into_owned();
        // Display name of the new scene object: the file stem ("scene.ply"
        // -> "scene"). Fall back to the file name for files without a stem
        // (e.g. dotfiles), which the open dialog may still pick.
        let name = path
            .file_stem()
            .or_else(|| path.file_name())
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        match std::thread::Builder::new()
            .name("object-loader".to_owned())
            .spawn(move || {
                // One dispatch for the whole family (spec §7 F1–F3): the
                // extension decides the parser inside io.
                let outcome = match io::load_object(&path) {
                    Ok(object) => {
                        log_loaded(&path, kind, &object);
                        LoadOutcome::Loaded { name, object }
                    }
                    Err(error) => {
                        tracing::warn!(file = %path.display(), %error, "object load failed");
                        LoadOutcome::Failed { file, error }
                    }
                };
                // Wake the UI so it polls the channel promptly; losing the
                // send is fine (the UI polls once per frame anyway).
                if sender.send(outcome).is_ok() {
                    context.request_repaint();
                }
            }) {
            Ok(_) => {
                self.load = Some(receiver);
            }
            Err(error) => {
                tracing::warn!(%error, "could not start the background loader");
                self.error_message = Some(texts::loader_start_failed(&error));
            }
        }
    }

    /// Poll the loader channel and turn its outcome into state: loaded
    /// data moves to the pending slot (installed later in the same frame,
    /// after this function returns), a failure keeps the current objects
    /// and surfaces the error window.
    fn poll_background_load(&mut self) {
        let Some(receiver) = &self.load else {
            return;
        };
        match receiver.try_recv() {
            Ok(LoadOutcome::Loaded { name, object }) => {
                self.load = None;
                self.pending_object = Some(PendingObject { name, object });
            }
            Ok(LoadOutcome::Failed { file, error }) => {
                self.load = None;
                self.error_message = Some(texts::load_failed(&file, &error));
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.load = None;
                self.error_message = Some(texts::LOADER_ABORTED.to_owned());
            }
        }
    }

    /// Install the pending object once the renderer exists (see
    /// [`ViewportState::install_object`]); a successful install clears any
    /// stale error from an earlier failed load. The pending slot is taken
    /// only when the renderer is ready, so no loaded data is ever cloned or
    /// dropped by the install fallback.
    fn install_pending_object(&mut self) {
        if self.pending_object.is_none() || !viewport::lock_state(&self.viewport).renderer_ready() {
            return; // Nothing pending, or retry on a later frame.
        }
        let pending = self
            .pending_object
            .take()
            .expect("pending_object checked above");
        let installed =
            viewport::lock_state(&self.viewport).install_object(pending.object, &pending.name);
        if installed {
            tracing::info!(name = %pending.name, "object added to the scene");
            self.error_message = None;
        } else {
            // Invariant violation (renderer_ready just said yes); the
            // loaded data is lost — log loudly rather than loop.
            tracing::error!(name = %pending.name, "pending object dropped: renderer vanished");
            self.pending_object = None;
            self.error_message = None;
        }
    }

    /// The top menu bar. The open entries are disabled while a background
    /// load runs (single-flight loading).
    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let loading = self.load.is_some();
        ui.menu_button(texts::MENU_FILE, |ui| {
            for kind in [OpenKind::PointCloud, OpenKind::Mesh, OpenKind::Path] {
                let open = ui.add_enabled(!loading, egui::Button::new(kind.menu_label()));
                if open.clicked() {
                    ui.close();
                    self.open_file_dialog(ui.ctx(), kind);
                }
            }
        });
    }

    /// The left objects panel: Fit, Add frame/marker entries, and the
    /// per-object list. Its requests open the add dialogs with defaults
    /// derived from the visible scene.
    fn objects_panel(&mut self, ctx: &egui::Context) {
        let state = Arc::clone(&self.viewport);
        let requests = egui::SidePanel::left(egui::Id::new("objects_panel"))
            .resizable(true)
            .default_width(230.0)
            .show(ctx, |ui| objects_panel::show_objects_panel(ui, &state))
            .inner;
        if requests.open_add_frame {
            let (center, scale) = viewport::lock_state(&self.viewport).ui_defaults();
            self.add_frame_dialog.open(center, scale);
        }
        if requests.open_add_marker {
            let (center, scale) = viewport::lock_state(&self.viewport).ui_defaults();
            self.add_marker_dialog.open(center, scale);
        }
    }

    /// Non-modal error notification: a closable window anchored top-right.
    /// Closing it clears the message, so a dismissed error stays dismissed
    /// (it only reappears for a *new* failure).
    fn error_window(&mut self, ctx: &egui::Context) {
        let mut dismissed = false;
        if let Some(message) = &self.error_message {
            let mut open = true;
            egui::Window::new(texts::ERROR_WINDOW_TITLE)
                .id(egui::Id::new("roboview_error"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-8.0, 8.0))
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label(message);
                });
            dismissed = !open;
        }
        if dismissed {
            self.error_message = None;
        }
    }
}

impl eframe::App for RoboViewApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // 1. Renderer lifecycle: create the renderer and the scene pipeline
        // family from eframe's wgpu render state and rebuild them when the
        // surface target format changes.
        if let Some(render_state) = frame.wgpu_render_state() {
            viewport::lock_state(&self.viewport).sync_renderer(
                &render_state.device,
                &render_state.queue,
                render_state.target_format,
            );
        }

        // 2. + 3. Load outcomes → pending data → install (see struct docs).
        self.poll_background_load();
        self.install_pending_object();

        // 4. Panels: menu bar on top, then the objects panel on the left
        // and the viewport filling the rest (plan §3.4).
        egui::TopBottomPanel::top(egui::Id::new("menu_bar")).show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                self.menu_bar(ui);
            });
        });

        self.objects_panel(ctx);

        let panel_fill = ctx.style().visuals.panel_fill;
        let viewport_state = Arc::clone(&self.viewport);
        let loading = self.load.is_some();
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(panel_fill))
            .show(ctx, |ui| {
                viewport::show_viewport(ui, &viewport_state, loading);
            });

        // 5. Floating windows on top of the panels.
        self.error_window(ctx);
        self.add_frame_dialog.show(ctx, &self.viewport);
        self.add_marker_dialog.show(ctx, &self.viewport);
    }
}

/// Load-succeeded log line, one per file family (the worker thread).
fn log_loaded(path: &Path, kind: OpenKind, object: &LoadedObject) {
    match (kind, object) {
        (OpenKind::PointCloud, LoadedObject::PointCloud(data)) => {
            tracing::info!(
                file = %path.display(),
                points = data.point_count(),
                "point cloud file loaded"
            );
        }
        (OpenKind::Mesh, LoadedObject::Mesh(data)) => {
            tracing::info!(
                file = %path.display(),
                vertices = data.vertex_count(),
                faces = data.face_count(),
                "mesh file loaded"
            );
        }
        (OpenKind::Path, LoadedObject::Path(data)) => {
            tracing::info!(
                file = %path.display(),
                points = data.point_count(),
                "path file loaded"
            );
        }
        // The dispatch pairs kind and data, so a mismatch is unreachable;
        // log it loudly if the extension family ever disagrees with the
        // parse result.
        (kind, object) => tracing::error!(?kind, ?object, "loader kind/data mismatch"),
    }
}

fn main() {
    tracing_subscriber::fmt().init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(texts::WINDOW_TITLE)
            .with_inner_size(egui::vec2(1280.0, 800.0))
            .with_min_inner_size(egui::vec2(480.0, 360.0)),
        renderer: eframe::Renderer::Wgpu,
        // Shared depth (display-types spec §6): egui-wgpu attaches a
        // Depth24Plus attachment to its pass — all scene pipelines must be
        // built with the same format and sample count.
        depth_buffer: 24,
        ..Default::default()
    };

    let result = eframe::run_native(
        texts::WINDOW_TITLE,
        options,
        Box::new(|cc| Ok(Box::new(RoboViewApp::new(cc)))),
    );
    if let Err(error) = result {
        tracing::error!(%error, "RoboView terminated with an error");
    }
}
