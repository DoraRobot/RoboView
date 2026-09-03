//! RoboView desktop shell: an eframe/egui application hosting the 3D
//! scene viewport.
//!
//! Opening files appends to the scene (display-types spec §1 replaces the
//! first feature's single-slot swap, `docs/specs/001-point-cloud-viewport/`):
//! each successful load adds one object named by its file stem, the first
//! object of an empty scene frames the scene bounds, later adds never move
//! the camera (spec §6). Failure keeps every existing object and surfaces
//! a readable error (spec A10). Frames and markers are added through the
//! objects panel dialogs instead (spec §7 F3/F4).
//!
//! Layout of the frame (`App::update`, ui-blueprint plan A5 — the fixed
//! layout period of 004 spec §6, before the 006 dock):
//!
//! 1. Align the renderer and the scene pipeline family with eframe's wgpu
//!    render state (created once it exists, rebuilt when the target format
//!    changes).
//! 2. Poll the background loader channel; a finished file becomes pending
//!    data, a failure keeps the current objects and shows an error window.
//! 3. Install pending data once a renderer exists (single-flight; upload
//!    and scene add happen here, on the UI thread).
//! 4. Draw the four-region fixed skeleton (spec §6): the top region —
//!    on Windows/Linux one panel holds the in-window menu bar and the
//!    toolbar as two rows; on macOS the same region holds only the
//!    toolbar, because the menu lives in the native system bar (decision
//!    D5: the toolbar is a button row, not a menu, so it stays in-window).
//!    The toolbar is the D3 button set: the Open dropdown (the per-family
//!    dialogs), Fit, the Add dropdown (frame / marker), and the Grid /
//!    Axes toggles, whose session state lives in `ViewportState` (T13
//!    subject) — every door of a toggle (toolbar buttons, HUD badges, menu
//!    items) funnels through the same `AppAction`. Below the top region:
//!    the full-width bottom status band — the readouts and the
//!    lightweight message strip of task T15's `ui/status_bar.rs` (D2: the
//!    strip and the error window coexist until the 007 message center) —
//!    the left objects panel (Fit, Add frame / marker, the object list),
//!    the right properties panel (task T14's `ui/properties_panel.rs`),
//!    and the central viewport filling the rest — the
//!    central panel always comes last. The side regions are
//!    width-constrained (left 180–360, right 200–360) so the 480×360
//!    minimum window keeps a viewport sliver (spec A13): egui clamps every
//!    panel to the space that remains, and the viewport guards degenerate
//!    rects (ui/viewport.rs), so squeezing the window never panics.
//! 5. Draw the non-modal error window and the two add dialogs on top.
//!
//! Loading never blocks the UI thread: the dialog choice spawns a worker
//! thread that parses the file and reports back over an mpsc channel; the
//! worker asks egui for a repaint when it finishes.
//!
//! The open doors — the menu's single combined "Open…" and the toolbar's
//! three per-family entries — are one channel into the same
//! `io::load_object` dispatch, distinguished only by the dialog filter
//! (spec A12 guard: the dotless extension lists stay here, next to
//! `OpenKind`, so the repository check script finds them).

mod ui;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use eframe::egui;

use roboview_core::io::{self, LoadError, LoadedObject};
use roboview_core::scene::camera::OrbitCamera;

use ui::menu::AppAction;
use ui::objects_panel::{self, ObjectsPanelState};
use ui::properties_panel;
use ui::status_bar::{MessageItem, MessageLevel, StatusBar, StatusInfo, TOOL_NAVIGATE};
use ui::texts;
use ui::theme;
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
    /// The per-family label of the toolbar Open dropdown entries (T20).
    fn menu_label(self, locale: texts::Locale) -> &'static str {
        match self {
            OpenKind::PointCloud => texts::menu_open_point_cloud(locale),
            OpenKind::Mesh => texts::menu_open_mesh(locale),
            OpenKind::Path => texts::menu_open_path(locale),
        }
    }

    /// Native dialog spec of this family: (title, filter label, dotless
    /// extension list). The lists never carry a dot: the repository check
    /// script `scripts/check_data_paths.sh` (spec A12) treats a quoted
    /// `.<ext>` in production code as a hardcoded data path.
    fn dialog_spec(
        self,
        locale: texts::Locale,
    ) -> (&'static str, &'static str, &'static [&'static str]) {
        match self {
            OpenKind::PointCloud => (
                texts::file_dialog_title_point_cloud(locale),
                texts::file_dialog_filter_point_cloud(locale),
                &["ply", "pcd"],
            ),
            OpenKind::Mesh => (
                texts::file_dialog_title_mesh(locale),
                texts::file_dialog_filter_mesh(locale),
                &["obj"],
            ),
            OpenKind::Path => (
                texts::file_dialog_title_path(locale),
                texts::file_dialog_filter_path(locale),
                &["csv", "xyz"],
            ),
        }
    }
}

/// One click of the toolbar row, queued while the row is drawn: the row's
/// closures borrow only the queue and this frame's snapshots, while the
/// handlers need `&mut self` — so the queue is drained after the row
/// returns.
#[derive(Debug, Clone, Copy)]
enum ToolbarEvent {
    /// The Open dropdown → the native dialog of one file family (the menu
    /// keeps its single combined door, [`RoboViewApp::open_any_dialog`]).
    OpenFamily(OpenKind),
    /// Fit / the Add dropdown / the Grid and Axes toggles → an action of
    /// the shared vocabulary, through the single dispatch point
    /// ([`RoboViewApp::dispatch_action`]).
    Action(AppAction),
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

/// Structured error state of the non-modal notification window (003 spec
/// §6.4): the event is stored, the message is assembled per frame in the
/// *current* locale — an open window follows a language switch.
enum ErrorEvent {
    /// A chosen file could not be loaded; the scene stays untouched (A10).
    Failed { file: String, error: LoadError },
    /// The background loader could not even be spawned.
    StartFailed(std::io::Error),
    /// The loader channel disconnected without an outcome.
    Aborted,
}

/// The RoboView application: window shell + viewport state.
struct RoboViewApp {
    /// Native macOS menu bridge (004 spike T2): keeps the muda menu tree
    /// alive for the whole process lifetime and drains its event queue once
    /// per frame. Always `Some` after a macOS launch; `None` only if a
    /// bridge was already registered.
    #[cfg(target_os = "macos")]
    native_menu_bridge: Option<ui::menu_bridge::BridgeCtx>,

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
    /// Active UI locale; flows down to every copy consumer (003 spec §6.2 —
    /// explicit injection, no global mutable state).
    locale: texts::Locale,
    /// Structured error event of the non-modal error window; `None` hides it.
    error: Option<ErrorEvent>,
    /// Tree-side state of the objects panel (search filter, group
    /// collapse/colors, selection label) — 004 app-owned subject (T12).
    objects_state: ObjectsPanelState,
    /// Status bar of the bottom band (004 T15): owns the frame-time
    /// samples of the FPS readout across frames ([`StatusBar::record`]
    /// once per `update`).
    status_bar: StatusBar,
    /// Session message log of the bottom strip (004 spec D2), oldest
    /// first: every recorded load failure lands here as an error item, and
    /// the non-modal error window keeps showing the event until dismissed
    /// — the two coexist until the 007 message center replaces the window.
    messages: Vec<MessageItem>,
    /// A12/M9 perf-protocol hooks (004 T18, env-gated debug affordances):
    /// the procedurally generated demo scene (ROBOVIEW_DEMO_SCENE=1), the
    /// ids it produced (selection sweep), and the last-second sampling
    /// stamp.
    demo_install: Option<Vec<PendingObject>>,
    demo_ids: Vec<u64>,
    demo_pickups_added: bool,
    demo_sweep_next: f64,
    last_fps_at: f64,
}

impl RoboViewApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Native macOS menu bar (004 spec §6, wiring spike T2): install the
        // muda bridge as early as possible in App::new — winit has already
        // installed its default menu at `applicationDidFinishLaunching`, so
        // `init_for_nsapp` replaces it before the first frame. The app
        // struct keeps the bridge (and thus the menu tree) alive.
        // The locale is resolved once, before the native menu tree is built:
        // its labels are baked at install time and relabeled on switch
        // (T10 integration recipe #1).
        let locale = texts::Locale::from_tag(&sys_locale::get_locale().unwrap_or_default());
        #[cfg(target_os = "macos")]
        let native_menu_bridge =
            ui::menu_bridge::init_bridge_with_menu(&cc.egui_ctx, ui::menu::build_native(locale));
        // Dark theme by default: point colors read best on a dark canvas.
        // Theme switching is out of scope (spec §5 non-goals).
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        // System fonts chain (003 spec §6.1): install before the first
        // frame so no frame renders with the bare default chain; timed for
        // the M5/A7 acceptance record.
        let t0 = std::time::Instant::now();
        let defs = ui::fonts::load_system_fonts();
        cc.egui_ctx.set_fonts(defs);
        tracing::info!(
            elapsed_ms = t0.elapsed().as_millis() as u64,
            "system fonts ready"
        );
        Self {
            viewport: Arc::new(Mutex::new(ViewportState::new())),
            pending_object: None,
            load: None,
            locale,
            error: None,
            objects_state: ObjectsPanelState::default(),
            status_bar: StatusBar::new(),
            messages: Vec::new(),
            demo_install: if std::env::var("ROBOVIEW_DEMO_SCENE").is_ok() {
                Some(demo_pending_objects())
            } else {
                None
            },
            demo_ids: Vec::new(),
            demo_pickups_added: false,
            demo_sweep_next: 0.0,
            last_fps_at: 0.0,
            #[cfg(target_os = "macos")]
            native_menu_bridge,
        }
    }

    /// Open the native file dialog for one file family and start a
    /// background load of the chosen file. Blocking by design (rfd's
    /// modal dialog); the toolbar's Open dropdown entries are disabled
    /// while a load is in flight, so at most one worker exists at a time.
    fn open_file_dialog(&mut self, ctx: &egui::Context, kind: OpenKind) {
        let (title, filter, extensions) = kind.dialog_spec(self.locale);
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
    /// The single "Open…" entry of the top menu: one native dialog listing
    /// every file family with its own filter (004 T10). Per-family entries
    /// live in the toolbar Open▾ (T20), which opens the family dialog
    /// directly.
    fn open_any_dialog(&mut self, ctx: &egui::Context) {
        let (_, filter_pc, exts_pc) = OpenKind::PointCloud.dialog_spec(self.locale);
        let (_, filter_mesh, exts_mesh) = OpenKind::Mesh.dialog_spec(self.locale);
        let (_, filter_path, exts_path) = OpenKind::Path.dialog_spec(self.locale);
        let Some(path) = rfd::FileDialog::new()
            .set_title(texts::tool_open(self.locale))
            .add_filter(filter_pc, exts_pc)
            .add_filter(filter_mesh, exts_mesh)
            .add_filter(filter_path, exts_path)
            .pick_file()
        else {
            return; // Cancelled: nothing to do.
        };
        // The dialog filtered by family, so the extension selects the kind
        // for the load log; the loader itself dispatches by extension.
        let kind = match path
            .extension()
            .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
            .as_deref()
        {
            Some("ply" | "pcd") => OpenKind::PointCloud,
            Some("obj") => OpenKind::Mesh,
            Some("csv" | "xyz") => OpenKind::Path,
            _ => OpenKind::PointCloud,
        };
        self.start_background_load(ctx, path, kind);
    }

    /// Dispatch one action of the shared vocabulary (004 T10/T20): the
    /// macOS native tree, the Win/Linux in-window bar, and the toolbar all
    /// funnel here, so every door behaves identically.
    fn dispatch_action(&mut self, ctx: &egui::Context, action: AppAction) {
        match action {
            AppAction::Open => self.open_any_dialog(ctx),
            AppAction::AddFrame => {
                let (center, scale) = viewport::lock_state(&self.viewport).ui_defaults();
                self.objects_state.open_add_frame(center, scale);
            }
            AppAction::AddMarker => {
                let (center, scale) = viewport::lock_state(&self.viewport).ui_defaults();
                self.objects_state.open_add_marker(center, scale);
            }
            AppAction::Language(locale) => {
                self.locale = locale;
                #[cfg(target_os = "macos")]
                if let Some(bridge) = &self.native_menu_bridge {
                    bridge.relabel(locale);
                }
            }
            // Helper-layer toggles of the D3 button set: the grid/axes
            // session state lives in the viewport layer (T13 subject) and
            // is the single source for every door — the menu items, the
            // toolbar buttons, and the HUD badges (spec §6 double doors) —
            // so the toggle happens here, once, and the native macOS check
            // marks follow the state.
            AppAction::ToggleGrid => {
                viewport::lock_state(&self.viewport).toggle_grid();
                self.reconcile_native_toggles();
            }
            AppAction::ToggleAxes => {
                viewport::lock_state(&self.viewport).toggle_axes();
                self.reconcile_native_toggles();
            }
            // The Fit door is toolbar-owned (no menu node produces it,
            // ui/menu.rs — the action exists so the toolbar maps into this
            // vocabulary unchanged): reframe the camera on the scene.
            AppAction::Fit => self.fit_scene(),
            // Native-terminated (Quit never emits a MenuEvent) or without a
            // door yet.
            AppAction::ResetView | AppAction::Quit => {
                tracing::debug!(?action, "menu action without a handler yet");
            }
        }
    }

    /// Mirror the authoritative helper-layer state into the native macOS
    /// menu check items after a toggle through a non-menu door. muda's
    /// check items auto-toggle only for their own clicks (ui/menu.rs); the
    /// toolbar and HUD doors of the same toggles reconcile here so the
    /// native marks cannot drift. No-op on platforms without the native
    /// menu.
    fn reconcile_native_toggles(&self) {
        #[cfg(target_os = "macos")]
        if let Some(bridge) = &self.native_menu_bridge {
            let state = viewport::lock_state(&self.viewport);
            bridge.set_grid_checked(state.grid_on());
            bridge.set_axes_checked(state.axes_on());
        }
    }

    /// Reframe the camera on the union of the measurable scene bounds —
    /// the framing the first object of an empty scene receives
    /// (display-types spec §6). One path for the toolbar's Fit door
    /// ([`AppAction::Fit`]) and the objects panel's Fit button; the doors
    /// disable themselves while nothing measurable exists (frames and
    /// markers never join the bounds union).
    fn fit_scene(&mut self) {
        let mut state = viewport::lock_state(&self.viewport);
        state.scene.camera = OrbitCamera::framing(state.scene.bounds_union().as_ref());
    }

    /// Reflect the single-flight load state into the native menu's Open
    /// item (the in-window bar filters at dispatch instead).
    fn set_open_enabled(&mut self, enabled: bool) {
        #[cfg(target_os = "macos")]
        if let Some(bridge) = &self.native_menu_bridge {
            bridge.set_open_enabled(enabled);
        }
    }

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
                self.set_open_enabled(false);
            }
            Err(error) => {
                tracing::warn!(%error, "could not start the background loader");
                self.record_message(MessageItem::new(
                    MessageLevel::Error,
                    texts::loader_start_failed(self.locale, &error),
                ));
                self.error = Some(ErrorEvent::StartFailed(error));
            }
        }
    }

    /// Append one error/warning to the bottom-strip log (004 spec D2),
    /// newest last. The log stays bounded: the strip shows only the most
    /// recent `MAX_VISIBLE_MESSAGES` entries, and older items beyond the
    /// session cap below drop out first.
    fn record_message(&mut self, item: MessageItem) {
        self.messages.push(item);
        const SESSION_CAP: usize = 16;
        if self.messages.len() > SESSION_CAP {
            let overflow = self.messages.len() - SESSION_CAP;
            self.messages.drain(..overflow);
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
                self.set_open_enabled(true);
                self.pending_object = Some(PendingObject { name, object });
            }
            Ok(LoadOutcome::Failed { file, error }) => {
                // D2: the strip carries the failure alongside the window.
                self.record_message(MessageItem::new(
                    MessageLevel::Error,
                    texts::load_failed(self.locale, &file, &error),
                ));
                self.load = None;
                self.set_open_enabled(true);
                self.error = Some(ErrorEvent::Failed { file, error });
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.record_message(MessageItem::new(
                    MessageLevel::Error,
                    texts::loader_aborted(self.locale),
                ));
                self.load = None;
                self.set_open_enabled(true);
                self.error = Some(ErrorEvent::Aborted);
            }
        }
    }

    /// Install the pending object once the renderer exists (see
    /// [`ViewportState::install_object`]); a successful install clears any
    /// stale error from an earlier failed load. The pending slot is taken
    /// only when the renderer is ready, so no loaded data is ever cloned or
    /// dropped by the install fallback.
    fn install_pending_object(&mut self) {
        // A12/M9 demo-scene hook: install one generated object per frame
        // through the same path, then the frame + marker once the queue is
        // empty (the viewport add path takes care of the uploads).
        if let Some(queue) = &mut self.demo_install {
            if !queue.is_empty() {
                if !viewport::lock_state(&self.viewport).renderer_ready() {
                    return;
                }
                let next = queue.remove(0);
                if viewport::lock_state(&self.viewport).install_object(next.object, &next.name) {
                    tracing::info!(name = %next.name, "demo scene object added");
                    if let Some(id) = viewport::lock_state(&self.viewport)
                        .scene
                        .iter()
                        .last()
                        .map(|o| o.id)
                    {
                        self.demo_ids.push(id);
                    }
                }
                return; // one demo install per frame
            }
            self.demo_install = None;
            if !self.demo_pickups_added {
                self.demo_pickups_added = true;
                let mut lock = viewport::lock_state(&self.viewport);
                lock.add_frame(glam::Vec3::new(3.0, 0.0, 0.0), 1.0);
                lock.add_marker(roboview_core::displays::Marker::Text(
                    roboview_core::displays::MarkerText {
                        anchor: glam::Vec3::new(5.0, 1.0, 0.0),
                        text: "demo marker".to_owned(),
                    },
                ));
            }
        }
        if self.pending_object.is_none() || !viewport::lock_state(&self.viewport).renderer_ready() {
            return; // Nothing pending, or retry on a later frame.
        }
        let pending = self
            .pending_object
            .take()
            .expect("pending_object checked above");
        let kind = loaded_kind(&pending.object);
        let installed =
            viewport::lock_state(&self.viewport).install_object(pending.object, &pending.name);
        if installed {
            tracing::info!(name = %pending.name, "object added to the scene");
            // Group default color for a *new* member (D4: new members
            // only). Applied to the colorable kinds — point cloud, mesh,
            // path — through the appearance channel; frames keep the 002
            // semantic axis colors (no override) and markers have no own
            // color (T16-2 note). The viewport registry already synced the
            // tree's per-kind defaults and reports the unset sentinel when
            // none is configured.
            if let Some(id) = viewport::lock_state(&self.viewport)
                .scene
                .iter()
                .last()
                .map(|o| o.id)
            {
                let mut lock = viewport::lock_state(&self.viewport);
                lock.apply_new_member_default_color(id, kind);
            }
            self.error = None;
        } else {
            // Invariant violation (renderer_ready just said yes); the
            // loaded data is lost — log loudly rather than loop.
            tracing::error!(name = %pending.name, "pending object dropped: renderer vanished");
            self.pending_object = None;
            self.error = None;
        }
    }

    /// The in-window menu bar row (Windows/Linux only — macOS renders the
    /// native global bar per D5, so this row never draws there). One door
    /// of the dual-path menu module: every click funnels into
    /// [`Self::dispatch_action`], and the Open action is filtered here
    /// while a background load runs (single-flight loading; the native
    /// menu disables its item through the bridge instead).
    #[cfg(not(target_os = "macos"))]
    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let loading = self.load.is_some();
        let mut actions = Vec::new();
        ui::menu::egui_menu_bar(ui, self.locale, &mut actions);
        for action in actions {
            // Single-flight: the Open action is inert while a load runs.
            if matches!(action, AppAction::Open) && loading {
                continue;
            }
            self.dispatch_action(ui.ctx(), action);
        }
    }

    /// The toolbar row of the four-region skeleton (004 spec §6, D3
    /// button set): the Open dropdown with the three per-family dialogs,
    /// Fit, the Add dropdown (frame / marker — the dialogs stay until T17
    /// replaces them with inline forms), and the Grid / Axes toggles.
    ///
    /// Clicks are queued while the row is drawn and applied when the row
    /// returns — the closures borrow only the queue and this frame's
    /// snapshots, the handlers need `&mut self`. The Open entries are
    /// disabled while a background load runs (single-flight loading, like
    /// the menu's Open door).
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let locale = self.locale;
        let loading = self.load.is_some();
        // Snapshot the frame's session subjects for the row: the grid and
        // axes states live in the viewport layer (T13 subject) and are
        // painted here from their getters; Fit is enabled only when
        // something measurable exists (the same bounds-union read the
        // objects panel's Fit button uses).
        let (grid_on, axes_on, can_fit) = {
            let state = viewport::lock_state(&self.viewport);
            (
                state.grid_on(),
                state.axes_on(),
                state.scene.bounds_union().is_some(),
            )
        };
        let mut events: Vec<ToolbarEvent> = Vec::new();

        ui.horizontal(|ui| {
            // Open ▾ — one entry per file family, each opening its own
            // dialog (the menu keeps the single combined door).
            ui.menu_button(texts::tool_open(locale), |ui| {
                for kind in [OpenKind::PointCloud, OpenKind::Mesh, OpenKind::Path] {
                    if ui
                        .add_enabled(!loading, egui::Button::new(kind.menu_label(locale)))
                        .clicked()
                    {
                        events.push(ToolbarEvent::OpenFamily(kind));
                        ui.close();
                    }
                }
            });

            // Fit — reframes the camera on the measurable scene bounds.
            if ui
                .add_enabled(can_fit, egui::Button::new(texts::objects_fit(locale)))
                .on_hover_text(if can_fit {
                    texts::objects_fit_tooltip(locale)
                } else {
                    texts::objects_fit_tooltip_disabled(locale)
                })
                .clicked()
            {
                events.push(ToolbarEvent::Action(AppAction::Fit));
            }

            // Add ▾ — the two create doors of the shared vocabulary.
            ui.menu_button(texts::tool_add(locale), |ui| {
                if ui.button(texts::objects_add_frame(locale)).clicked() {
                    events.push(ToolbarEvent::Action(AppAction::AddFrame));
                    ui.close();
                }
                if ui.button(texts::objects_add_marker(locale)).clicked() {
                    events.push(ToolbarEvent::Action(AppAction::AddMarker));
                    ui.close();
                }
            });

            // Grid / Axes — stateful doors over the helper-layer session
            // state; they dispatch the same actions the menu items fire.
            let grid = ui
                .selectable_label(grid_on, texts::toggle_grid(locale))
                .on_hover_text(texts::grid_toggle_tooltip(locale));
            if grid.clicked() {
                events.push(ToolbarEvent::Action(AppAction::ToggleGrid));
            }
            let axes = ui
                .selectable_label(axes_on, texts::toggle_axes(locale))
                .on_hover_text(texts::axes_toggle_tooltip(locale));
            if axes.clicked() {
                events.push(ToolbarEvent::Action(AppAction::ToggleAxes));
            }
        });

        for event in events {
            match event {
                ToolbarEvent::OpenFamily(kind) => {
                    // Defensive single-flight gate (the entries are
                    // disabled too): never start a second load while one
                    // runs.
                    if !loading {
                        self.open_file_dialog(ui.ctx(), kind);
                    }
                }
                ToolbarEvent::Action(action) => self.dispatch_action(ui.ctx(), action),
            }
        }
    }

    /// The top region of the four-region skeleton (004 spec §6): the
    /// menu bar row with the toolbar row underneath, sharing one panel.
    #[cfg(not(target_os = "macos"))]
    fn top_region(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top(egui::Id::new("menu_bar")).show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                self.menu_bar(ui);
            });
            // Second row of the same panel: the toolbar below the menu.
            ui.add_space(4.0);
            self.toolbar(ui);
        });
    }

    /// The top region on macOS: the toolbar only. The menu lives in the
    /// native system bar (D5), and the toolbar is a button row, not a
    /// menu — so unlike the in-window menu it stays visible.
    #[cfg(target_os = "macos")]
    fn top_region(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top(egui::Id::new("toolbar")).show(ctx, |ui| {
            self.toolbar(ui);
        });
    }

    /// The left objects panel: Fit, Add frame/marker entries, and the
    /// per-object list. Its requests open the add dialogs with defaults
    /// derived from the visible scene.
    fn objects_panel(&mut self, ctx: &egui::Context) {
        let state = Arc::clone(&self.viewport);
        // Width constraint of the four-region skeleton (004 spec §6, A13):
        // at the 480×360 minimum window the panel stays ≥ 180 px wide while
        // the viewport keeps a sliver; egui clamps the panel to the space
        // that remains when the window is tighter.
        let frame = region_frame(&ctx.style());
        let output = egui::SidePanel::left(egui::Id::new("objects_panel"))
            .resizable(true)
            .default_width(230.0)
            .width_range(180.0..=360.0)
            .frame(frame)
            .show(ctx, |ui| {
                let lock = viewport::lock_state(&state);
                objects_panel::ui(ui, &mut self.objects_state, &lock.scene, self.locale)
            })
            .inner;
        // Scene mutations the panel queued (rename/visibility/delete): the
        // GPU handles live in the objects' Drop path, so the A6 resource
        // ledger stays balanced; a stale id is a no-op in the scene API.
        if !output.actions.is_empty() {
            objects_panel::apply_actions(
                &mut viewport::lock_state(&self.viewport).scene,
                &output.actions,
            );
        }
        if output.fit {
            self.fit_scene();
        }
        if let Some((origin, length)) = output.add_frame {
            viewport::lock_state(&self.viewport).add_frame(origin, length);
        }
        if let Some(marker) = output.add_marker {
            viewport::lock_state(&self.viewport).add_marker(marker);
        }
        // Group default colors → the viewport registry (T16-2), read on
        // new-member creation (D4: new members only; the tree owns the
        // per-kind defaults and only user-set entries appear here).
        if !self.objects_state.group_default_color.is_empty() {
            let mut lock = viewport::lock_state(&self.viewport);
            for (kind, color) in &self.objects_state.group_default_color {
                lock.set_group_default_color(*kind, *color);
            }
        }
    }

    /// The right properties region of the four-region skeleton (004 spec
    /// §6). Task T14's `ui/properties_panel.rs` renders the grouped
    /// read-only property cards of the selected object (the selection
    /// label comes from the objects panel state, T12); the region stays
    /// width-constrained (200–360) so the 480×360 minimum window keeps a
    /// viewport sliver (spec A13).
    fn properties_panel(&mut self, ctx: &egui::Context) {
        let frame = region_frame(&ctx.style());
        let output = egui::SidePanel::right(egui::Id::new("properties_panel"))
            .resizable(true)
            .default_width(220.0)
            .width_range(200.0..=360.0)
            .frame(frame)
            .show(ctx, |ui| {
                let lock = viewport::lock_state(&self.viewport);
                properties_panel::ui(ui, self.objects_state.selected, &lock.scene, self.locale)
            })
            .inner;
        // Commit this frame's property edits (T16-1 output → the viewport
        // single-object service): ≤1 frame effect (A3/A4). Fields go through
        // apply_object_edits, the color row through the appearance channel.
        for edit in output.edits {
            let mut lock = viewport::lock_state(&self.viewport);
            lock.apply_object_edits(edit.id, &edit.fields);
            if let Some(color) = edit.color {
                lock.appearance_override(edit.id, color);
            }
        }
        // Selection mirror → viewport highlight (A2): the tree is the
        // selection source; the viewport follows within one frame. The
        // call is idempotent (no-op on an unchanged selection).
        viewport::lock_state(&self.viewport).set_selected(self.objects_state.selected);
    }

    /// The bottom status region of the four-region skeleton (004 spec
    /// §6): task T15's `ui/status_bar.rs` draws the fixed 26 px band — the
    /// load-state / tool / pointer-coordinate / FPS readouts (spec A7) and
    /// the lightweight message strip (D2). The strip is fed from the
    /// session log [`Self::record_message`] fills on every load failure.
    fn status_bar_panel(&mut self, ctx: &egui::Context) {
        // The 26 px band trims the shared 8 px panel margin vertically so
        // the single text row stays unclipped (status_bar module doc).
        let frame = egui::Frame::side_top_panel(&ctx.style())
            .fill(theme::PANEL_BACKGROUND)
            .inner_margin(egui::Margin::symmetric(8, 2));
        // Pointer-world intersection from the viewport layer: the frame's
        // stored rect and pointer, reference plane Z=0 while the grid is
        // shown and the camera-target plane while hidden.
        let pointer_world = viewport::lock_state(&self.viewport).pointer_world();
        let info = StatusInfo {
            loading: self.load.is_some(),
            pointer_world,
            tool: TOOL_NAVIGATE,
            messages: &self.messages,
        };
        egui::TopBottomPanel::bottom(egui::Id::new("status_bar"))
            .resizable(false)
            .exact_height(26.0)
            .frame(frame)
            .show(ctx, |ui| {
                self.status_bar.ui(ui, self.locale, &info);
            });
    }

    /// Non-modal error notification: a closable window anchored top-right.
    /// Closing it clears the message, so a dismissed error stays dismissed
    /// (it only reappears for a *new* failure).
    fn error_window(&mut self, ctx: &egui::Context) {
        let mut dismissed = false;
        if let Some(event) = &self.error {
            // Assemble in the *current* locale every frame (003 spec §6.4):
            // an already-open error window follows a language switch.
            let message = match event {
                ErrorEvent::Failed { file, error } => texts::load_failed(self.locale, file, error),
                ErrorEvent::StartFailed(error) => texts::loader_start_failed(self.locale, error),
                ErrorEvent::Aborted => texts::loader_aborted(self.locale).to_owned(),
            };
            let mut open = true;
            egui::Window::new(texts::error_window_title(self.locale))
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
            self.error = None;
        }
    }
}

impl eframe::App for RoboViewApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Native macOS menu events (004 T10): drain the bridge queue once
        // per frame and map ids through the single action table — the same
        // dispatch the in-window bar uses.
        #[cfg(target_os = "macos")]
        if let Some(bridge) = &mut self.native_menu_bridge {
            for event in bridge.drain() {
                match ui::menu::action_from_id(event.id()) {
                    Some(action) => self.dispatch_action(ctx, action),
                    None => tracing::warn!(menu_id = %event.id().0, "menu event without an action"),
                }
            }
        }

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

        // 4. Four-region fixed skeleton (004 spec §6): the status bar of
        // the bottom band records this frame's duration first (its FPS
        // readout is a window over recent frames), then the top region
        // (menu bar + toolbar on Windows/Linux, the toolbar alone on macOS
        // — the native system bar hosts the menu there, D5), the full-width
        // bottom status band (T15), the left objects panel, the right
        // properties panel (T14), and the central viewport last — it fills
        // whatever the regions leave.
        self.status_bar
            .record(std::time::Duration::from_secs_f64(f64::from(
                ctx.input(|i| i.unstable_dt),
            )));
        // A12/M9 perf sample (one line per elapsed second, release runs):
        // the readout exposes the window p95 so the protocol can be
        // measured from the log; the selection sweep below exercises the
        // per-object appearance channel (mandatory in the demo scene).
        let now_secs = ctx.input(|i| i.time);
        if now_secs - self.last_fps_at >= 1.0 {
            self.last_fps_at = now_secs;
            let fps = self.status_bar.fps();
            let p95_ms = self.status_bar.p95_frame_ms();
            let lock = viewport::lock_state(&self.viewport);
            tracing::info!(
                fps = fps.unwrap_or_default(),
                p95_ms = p95_ms.unwrap_or_default(),
                grid = lock.grid_on(),
                objects = lock.scene.iter().count(),
                "perf sample (A12)"
            );
        }
        if !self.demo_ids.is_empty() && now_secs - self.demo_sweep_next >= 0.5 {
            self.demo_sweep_next = now_secs;
            let idx = (self.demo_sweep_next * 2.0).floor() as usize % self.demo_ids.len();
            let id = self.demo_ids[idx];
            viewport::lock_state(&self.viewport).set_selected(Some(id));
        }
        // A12 run keep-alive: an idle winit loop never repaints, so the
        // measuring run re-arms itself for the *next* frame while the demo
        // scene is active — uncapped, so the recorded frame times are the
        // true interactive cost (the manual protocol needs continuous
        // samples).
        if self.demo_install.is_some() || !self.demo_ids.is_empty() {
            ctx.request_repaint();
        }
        self.top_region(ctx);
        self.status_bar_panel(ctx);
        self.objects_panel(ctx);
        self.properties_panel(ctx);

        let viewport_state = Arc::clone(&self.viewport);
        let loading = self.load.is_some();
        egui::CentralPanel::default()
            // The viewport floor token (004 spec §6, ui/theme.rs): the
            // neutral backdrop behind the 3D content, pinned to the palette
            // instead of a per-frame derivation from the theme's panel fill.
            .frame(egui::Frame::NONE.fill(theme::VIEWPORT_FLOOR))
            .show(ctx, |ui| {
                viewport::show_viewport(ui, &viewport_state, loading, self.locale);
            });

        // 5. Floating windows on top of the panels (the add dialogs are
        // gone — A5: the panel's inline forms replace them).
        self.error_window(ctx);
    }
}

/// A12/M9 demo scene (ROBOVIEW_DEMO_SCENE=1): the spec's acceptance
/// composite C rendered procedurally — a 1M-point cloud and a 100k-face
/// mesh — so the perf protocol never needs repository data files (the A9
/// guard stays untouched). Frame + marker are added at install
/// (see `install_pending_object`).
fn demo_pending_objects() -> Vec<PendingObject> {
    const POINTS: usize = 1_000_000;
    const GRID: usize = 224; // 224² quads × 2 = 100,352 faces (spec C ≥100k)

    let mut state = 0x9e37_79b9u32;
    let mut next = move || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        state
    };
    let mut positions = Vec::with_capacity(POINTS);
    for _ in 0..POINTS {
        let x = (next() % 200_000) as f32 / 2_000.0 - 50.0;
        let y = (next() % 200_000) as f32 / 2_000.0 - 50.0;
        let z = (next() % 100_000) as f32 / 2_000.0 - 25.0;
        positions.push(glam::Vec3::new(x, y, z));
    }
    let cloud_bounds = io::Aabb::from_points(&positions);
    let cloud = io::PointCloudData {
        positions,
        colors: None,
        bounds: cloud_bounds,
        format: io::Format::Ply,
    };

    let side = GRID as f32 / 2.0;
    let mut mesh_positions = Vec::with_capacity((GRID + 1) * (GRID + 1));
    for iy in 0..=GRID {
        for ix in 0..=GRID {
            let x = ix as f32 - side;
            let y = iy as f32 - side;
            let z = 4.0 * (x * 0.15).sin() * (y * 0.15).cos();
            mesh_positions.push(glam::Vec3::new(x, y, z));
        }
    }
    let mut indices: Vec<u32> = Vec::with_capacity(GRID * GRID * 6);
    for iy in 0..GRID {
        for ix in 0..GRID {
            let a = (iy * (GRID + 1) + ix) as u32;
            let b = a + 1;
            let c = a + (GRID as u32 + 1);
            let d = c + 1;
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    let mesh_bounds = io::Aabb::from_points(&mesh_positions);
    let mesh = io::MeshData {
        positions: mesh_positions,
        normals: None,
        indices: Some(indices),
        bounds: mesh_bounds,
    };

    vec![
        PendingObject {
            name: "demo-points-1m".to_owned(),
            object: io::LoadedObject::PointCloud(cloud),
        },
        PendingObject {
            name: "demo-mesh-100k".to_owned(),
            object: io::LoadedObject::Mesh(mesh),
        },
    ]
}

/// The display kind of a loaded object (group-default color injection,
/// T16-3).
fn loaded_kind(object: &LoadedObject) -> roboview_core::displays::DisplayKind {
    match object {
        LoadedObject::PointCloud(_) => roboview_core::displays::DisplayKind::PointCloud,
        LoadedObject::Mesh(_) => roboview_core::displays::DisplayKind::Mesh,
        LoadedObject::Path(_) => roboview_core::displays::DisplayKind::Path,
    }
}

/// Panel chrome of the four-region skeleton (004 spec §6): egui's standard
/// side/top-bottom panel frame filled with the semantic palette token
/// `theme::PANEL_BACKGROUND` instead of a per-frame derivation from the
/// theme's `panel_fill`. The token is the same dark gray today
/// (ui/theme.rs), so the swap is visually a no-op that pins the fill to
/// one source.
fn region_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::side_top_panel(style).fill(theme::PANEL_BACKGROUND)
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
