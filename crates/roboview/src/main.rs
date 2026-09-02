//! RoboView desktop shell: an eframe/egui application hosting the 3D point
//! cloud viewport.
//!
//! Opening files appends to the scene (display-types spec §1 replaces the
//! first feature's single-slot swap, `docs/specs/point-cloud-viewport/`):
//! each successful load adds one object named by its file stem, the first
//! object of an empty scene frames the scene bounds, later adds never move
//! the camera (spec §6). Failure keeps every existing object and surfaces
//! a readable error (spec A10).
//!
//! Layout of the frame (`App::update`, display-types plan §3.4):
//!
//! 1. Align the point cloud renderer with eframe's wgpu render state
//!    (created once it exists, rebuilt when the target format changes).
//! 2. Poll the background loader channel; a finished file becomes pending
//!    data, a failure keeps the current objects and shows an error window.
//! 3. Install pending data once a renderer exists (single-flight; upload
//!    and scene add happen here, on the UI thread).
//! 4. Draw the menu bar (File → Open point cloud file…), the central
//!    viewport panel, and the non-modal error notification.
//!
//! Loading never blocks the UI thread: the dialog choice spawns a worker
//! thread that parses the file and reports back over an mpsc channel; the
//! worker asks egui for a repaint when it finishes.

mod ui;

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use eframe::egui;

use roboview_core::io::{self, PointCloudError};

use ui::texts;
use ui::viewport::{self, ViewportState};

/// Result of one background load, sent over the loader channel.
enum LoadOutcome {
    /// The file parsed; the data and its display name (file stem) are
    /// ready for the scene add on the UI thread.
    Loaded {
        /// Display name of the new scene object (file stem, display-types
        /// plan §3.1).
        name: String,
        data: io::PointCloudData,
    },
    /// The file could not be loaded; the scene stays untouched (spec A10).
    Failed {
        /// File name for the human-readable error window.
        file: String,
        error: PointCloudError,
    },
}

/// A successfully parsed file waiting for renderer-side install.
struct PendingCloud {
    /// Display name of the object to add (file stem).
    name: String,
    /// Parsed point data, ready to upload.
    data: io::PointCloudData,
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
    pending_cloud: Option<PendingCloud>,
    /// In-flight background load. `Some` also drives the single-flight
    /// guard that disables the Open menu item while a load runs.
    load: Option<Receiver<LoadOutcome>>,
    /// Message of the non-modal error window; `None` hides the window.
    error_message: Option<String>,
}

impl RoboViewApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Dark theme by default: point colors read best on a dark canvas.
        // Theme switching is out of scope (spec §5 non-goals).
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        Self {
            viewport: Arc::new(Mutex::new(ViewportState::new())),
            pending_cloud: None,
            load: None,
            error_message: None,
        }
    }

    /// Open the native file dialog and start a background load of the
    /// chosen file. Blocking by design (rfd's modal dialog); the menu
    /// disables itself while a load is in flight, so at most one worker
    /// exists at a time.
    fn open_file_dialog(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .set_title(texts::FILE_DIALOG_TITLE)
            .add_filter(texts::FILE_DIALOG_FILTER_NAME, &["ply", "pcd"])
            .pick_file()
        else {
            return; // Cancelled: nothing to do.
        };
        self.start_background_load(ctx, path);
    }

    /// Spawn the parse worker for `path`. On success the receiver replaces
    /// any finished slot; on spawn failure the user gets the error window.
    fn start_background_load(&mut self, ctx: &egui::Context, path: PathBuf) {
        tracing::info!(file = %path.display(), "starting background load");
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
            .name("point-cloud-loader".to_owned())
            .spawn(move || {
                let outcome = match io::load_point_cloud(&path) {
                    Ok(data) => {
                        tracing::info!(
                            file = %path.display(),
                            points = data.point_count(),
                            "point cloud file loaded"
                        );
                        LoadOutcome::Loaded { name, data }
                    }
                    Err(error) => {
                        tracing::warn!(file = %path.display(), %error, "point cloud load failed");
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
            Ok(LoadOutcome::Loaded { name, data }) => {
                self.load = None;
                self.pending_cloud = Some(PendingCloud { name, data });
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

    /// Install the pending cloud once the renderer exists (see
    /// [`ViewportState::install_cloud`]); a successful install clears any
    /// stale error from an earlier failed load.
    fn install_pending_cloud(&mut self) {
        let Some(pending) = &self.pending_cloud else {
            return;
        };
        let installed =
            viewport::lock_state(&self.viewport).install_cloud(&pending.data, &pending.name);
        if installed {
            tracing::info!(
                name = %pending.name,
                points = pending.data.point_count(),
                "point cloud added to the scene"
            );
            self.pending_cloud = None;
            self.error_message = None;
        }
    }

    /// The top menu bar. The Open item is disabled while a background load
    /// runs (single-flight loading).
    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let loading = self.load.is_some();
        ui.menu_button(texts::MENU_FILE, |ui| {
            let open = ui.add_enabled(!loading, egui::Button::new(texts::MENU_OPEN_POINT_CLOUD));
            if open.clicked() {
                ui.close();
                self.open_file_dialog(ui.ctx());
            }
        });
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
        // 1. Renderer lifecycle: create it from eframe's wgpu render state
        // and rebuild it when the surface target format changes.
        if let Some(render_state) = frame.wgpu_render_state() {
            viewport::lock_state(&self.viewport).sync_renderer(
                &render_state.device,
                &render_state.queue,
                render_state.target_format,
            );
        }

        // 2. + 3. Load outcomes → pending data → install (see struct docs).
        self.poll_background_load();
        self.install_pending_cloud();

        // 4. Panels: menu bar, then the viewport filling the rest.
        egui::TopBottomPanel::top(egui::Id::new("menu_bar")).show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                self.menu_bar(ui);
            });
        });

        let panel_fill = ctx.style().visuals.panel_fill;
        let viewport_state = Arc::clone(&self.viewport);
        let loading = self.load.is_some();
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(panel_fill))
            .show(ctx, |ui| {
                viewport::show_viewport(ui, &viewport_state, loading);
            });

        // 5. Error notifications on top of the panels.
        self.error_window(ctx);
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
