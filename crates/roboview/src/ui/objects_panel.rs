//! The fixed objects sidebar (display-types plan §3.4) and the two
//! non-modal add dialogs it opens.
//!
//! Panel layout (per plan §3.4, the sidebar hosts the UI-add entries while
//! the File menu only opens files):
//!
//! - the heading;
//! - the action row: the **Fit** control (display-types spec §6: reframe
//!   the camera to the union of the visible objects — disabled while nothing
//!   measurable is visible) and the two **Add…** entries that open the
//!   dialogs below;
//! - the object list: one row per scene object in add order (spec §4). A
//!   row shows the visibility checkbox, a kind label, the object's name
//!   (truncated), and the remove button; every action is stable-id based
//!   ([`roboview_core::scene::Scene::toggle_visible`] /
//!   [`roboview_core::scene::Scene::remove`]) and rows keep a stable egui
//!   identity (`Id::new(object.id)`) so widget state never leaks between
//!   rows as the list changes. The empty scene shows the hint copy instead.
//!
//! The add dialogs open non-modally next to the panel (display-types spec
//! §7 F3/F4): frame (origin + axis length) and marker (text label or
//! arrow), with defaults derived from the visible scene bounds
//! ([`ViewportState::ui_defaults`]) so UI-placed geometry lands in view.
//! All copy lives in `texts.rs`; every entry point takes the current
//! [`Locale`] (003 spec §6.2: explicit injection) and reads its copy
//! through `texts::…(locale)` — the dialogs store no locale of their own,
//! so an open dialog re-renders in the switched language on the next frame.
//!
//! The scene lock is the same single uncontended lock the viewport panel
//! uses ([`viewport::lock_state`]): the panel and dialogs run on the UI
//! thread and briefly lock per action, never while painting.

use std::sync::{Arc, Mutex};

use eframe::egui;
use glam::Vec3;

use roboview_core::displays::{DisplayKind, Marker};
use roboview_core::scene::camera::OrbitCamera;

use super::texts::{self, Locale};
use super::viewport::{self, ViewportState};

/// Panel requests to the caller (main.rs), consumed after the panel body
/// ran: each `open_*` flag asks the caller to open the matching dialog with
/// fresh scene-derived defaults.
#[derive(Debug, Default)]
pub struct PanelRequests {
    /// The Add frame entry was clicked.
    pub open_add_frame: bool,
    /// The Add marker entry was clicked.
    pub open_add_marker: bool,
}

/// Draw the objects sidebar into `ui` (used inside the left
/// [`egui::SidePanel`]). Returns the requests the caller should act on.
pub fn show_objects_panel(
    ui: &mut egui::Ui,
    state: &Arc<Mutex<ViewportState>>,
    locale: Locale,
) -> PanelRequests {
    let mut requests = PanelRequests::default();

    ui.add_space(4.0);
    ui.heading(texts::objects_panel_title(locale));

    // Action row: Fit + the two UI-add entries. Wrapped so a narrow panel
    // folds the buttons instead of clipping them.
    let can_fit = viewport::lock_state(state).scene.bounds_union().is_some();
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(can_fit, egui::Button::new(texts::objects_fit(locale)))
            .on_hover_text(if can_fit {
                texts::objects_fit_tooltip(locale)
            } else {
                texts::objects_fit_tooltip_disabled(locale)
            })
            .clicked()
        {
            // Fit reframes to the union of the visible objects; an empty
            // union falls back to the default pose (core `framing(None)`).
            let mut viewport = viewport::lock_state(state);
            viewport.scene.camera = OrbitCamera::framing(viewport.scene.bounds_union().as_ref());
        }
        if ui.button(texts::objects_add_frame(locale)).clicked() {
            requests.open_add_frame = true;
        }
        if ui.button(texts::objects_add_marker(locale)).clicked() {
            requests.open_add_marker = true;
        }
    });

    ui.add_space(8.0);

    // Object list, add-ordered. The ids are snapshotted so rows can remove
    // themselves while the list iterates; a removed id is simply skipped.
    let ids: Vec<u64> = viewport::lock_state(state)
        .scene
        .iter()
        .map(|object| object.id)
        .collect();
    if ids.is_empty() {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(texts::objects_empty_hint(locale))
                .color(ui.visuals().weak_text_color()),
        );
        return requests;
    }

    egui::ScrollArea::vertical()
        .id_salt("objects_list")
        .auto_shrink([false, false])
        .max_height(ui.available_height())
        .show(ui, |ui| {
            for id in ids {
                show_object_row(ui, state, locale, id);
            }
        });

    requests
}

/// One object-list row, keyed by the object's stable scene id. The row
/// snapshots the entry it shows, then acts by id — the scene is only
/// locked for the snapshot and for the requested mutation, so rows never
/// hold the lock while egui lays out widgets.
fn show_object_row(ui: &mut egui::Ui, state: &Arc<Mutex<ViewportState>>, locale: Locale, id: u64) {
    let Some((visible, kind, name)) = viewport::lock_state(state)
        .scene
        .get(id)
        .map(|object| (object.visible, object.object.kind(), object.name.clone()))
    else {
        return; // Removed earlier this frame.
    };

    let mut visible = visible;
    let mut remove = false;
    ui.push_id(egui::Id::new(id), |ui| {
        ui.horizontal(|ui| {
            // Visibility checkbox: toggling skips drawing only — resources
            // stay uploaded (display-types spec §6).
            if ui.checkbox(&mut visible, "").changed() {
                viewport::lock_state(state).scene.toggle_visible(id);
            }
            kind_pill(ui, kind, locale);
            // The name takes the flexible middle slot, truncated; the
            // remove button is pinned to the row's right edge afterwards.
            let row_height = ui.spacing().interact_size.y.max(20.0);
            let name_width = (ui.available_width() - REMOVE_SLOT).max(0.0);
            let name_color = if visible {
                ui.visuals().text_color()
            } else {
                ui.visuals().weak_text_color()
            };
            ui.add_sized(
                egui::vec2(name_width, row_height),
                egui::Label::new(egui::RichText::new(name).color(name_color)).truncate(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                remove = ui
                    .small_button(texts::OBJECTS_REMOVE)
                    .on_hover_text(texts::objects_remove_tooltip(locale))
                    .clicked();
            });
        });
    });
    if remove {
        viewport::lock_state(state).scene.remove(id);
    }
}

/// Reserved row width for the pinned remove button (the flexible name slot
/// leaves this much on the right for the right-to-left button region).
const REMOVE_SLOT: f32 = 26.0;

/// The kind column: a small rounded label in the panel's code-tint color.
/// The mapping from kind to copy is `texts::object_kind_label` — the core's
/// `DisplayKind::as_str` is a ledger key, not UI text.
fn kind_pill(ui: &mut egui::Ui, kind: DisplayKind, locale: Locale) {
    let color = ui.visuals().weak_text_color();
    let galley = ui.painter().layout_no_wrap(
        texts::object_kind_label(locale, kind).to_owned(),
        egui::FontId::proportional(10.0),
        color,
    );
    let pad = egui::vec2(6.0, 2.0);
    let (rect, _) = ui.allocate_exact_size(galley.size() + pad * 2.0, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, ui.visuals().code_bg_color);
    painter.galley(rect.min + pad, galley, color);
}

/// Non-modal Add frame dialog (display-types spec §7 F3): origin + axis
/// length, then "Add" uploads the frame through the line pipeline and
/// appends it to the scene under a generated name. Adding closes the window
/// — a frame is a one-shot placement, unlike the marker dialog which stays
/// open for repeat adds.
pub struct AddFrameDialog {
    open: bool,
    origin: Vec3,
    length: f32,
    /// Drag step of every DragValue, matched to the scene scale at open
    /// time (see [`drag_speed`]).
    speed: f32,
}

impl AddFrameDialog {
    pub fn new() -> Self {
        Self {
            open: false,
            origin: Vec3::ZERO,
            length: 1.0,
            speed: 0.05,
        }
    }

    /// Open the dialog with defaults derived from the visible scene
    /// (center of the bounds union, axis length a quarter of its largest
    /// dimension).
    pub fn open(&mut self, center: Vec3, scale: f32) {
        self.open = true;
        self.origin = center;
        self.length = (scale * 0.25).max(1e-4);
        self.speed = drag_speed(scale);
    }

    pub fn show(&mut self, ctx: &egui::Context, state: &Arc<Mutex<ViewportState>>, locale: Locale) {
        if !self.open {
            return;
        }
        let mut add = false;
        egui::Window::new(texts::add_frame_window_title(locale))
            .id(egui::Id::new("add_frame_dialog"))
            .collapsible(false)
            .resizable(false)
            .open(&mut self.open)
            .show(ctx, |ui| {
                xyz_row(
                    ui,
                    texts::add_frame_origin(locale),
                    &mut self.origin,
                    self.speed,
                );
                ui.horizontal(|ui| {
                    ui.label(texts::add_frame_length(locale));
                    // Negative or zero lengths draw no geometry in the line
                    // pipeline, so clamp the axis length to positive values.
                    ui.add(
                        egui::DragValue::new(&mut self.length)
                            .speed(self.speed)
                            .range(0.0..=f32::MAX),
                    );
                });
                // The confirm button, pinned right of the window.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(texts::add_object_button(locale)).clicked() {
                        add = true;
                    }
                });
            });
        if add {
            viewport::lock_state(state).add_frame(self.origin, self.length);
            self.open = false;
        }
    }
}

impl Default for AddFrameDialog {
    fn default() -> Self {
        Self::new()
    }
}

/// The chosen marker shape (display-types spec §7 F4): a viewport overlay
/// text label or a 3D arrow with a head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkerShape {
    Text,
    Arrow,
}

/// Non-modal Add marker dialog (display-types spec §7 F4): one shape radio
/// (text label / arrow), shape-specific parameter rows, and "Add" — which
/// appends the marker under a generated name and stays open (closing via
/// the window button), so several markers can be placed in one session.
/// Adding a text label clears its text field for the next entry.
pub struct AddMarkerDialog {
    open: bool,
    shape: MarkerShape,
    anchor: Vec3,
    text: String,
    start: Vec3,
    end: Vec3,
    speed: f32,
}

impl AddMarkerDialog {
    pub fn new() -> Self {
        Self {
            open: false,
            shape: MarkerShape::Text,
            anchor: Vec3::ZERO,
            text: String::new(),
            start: Vec3::ZERO,
            end: Vec3::X,
            speed: 0.05,
        }
    }

    /// Open the dialog with defaults derived from the visible scene: the
    /// text anchor and the arrow tail at the bounds center, the arrow tip
    /// a fifth of the largest dimension along +X.
    pub fn open(&mut self, center: Vec3, scale: f32) {
        self.open = true;
        self.shape = MarkerShape::Text;
        self.anchor = center;
        self.text.clear();
        self.start = center;
        self.end = center + Vec3::X * (scale * 0.2).max(1e-4);
        self.speed = drag_speed(scale);
    }

    pub fn show(&mut self, ctx: &egui::Context, state: &Arc<Mutex<ViewportState>>, locale: Locale) {
        if !self.open {
            return;
        }
        let mut add = false;
        egui::Window::new(texts::add_marker_window_title(locale))
            .id(egui::Id::new("add_marker_dialog"))
            .collapsible(false)
            .resizable(false)
            .open(&mut self.open)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(
                        &mut self.shape,
                        MarkerShape::Text,
                        texts::marker_shape_text(locale),
                    );
                    ui.selectable_value(
                        &mut self.shape,
                        MarkerShape::Arrow,
                        texts::marker_shape_arrow(locale),
                    );
                });
                ui.add_space(4.0);
                match self.shape {
                    MarkerShape::Text => {
                        xyz_row(
                            ui,
                            texts::marker_anchor(locale),
                            &mut self.anchor,
                            self.speed,
                        );
                        ui.horizontal(|ui| {
                            ui.label(texts::marker_text(locale));
                            ui.add(
                                egui::TextEdit::singleline(&mut self.text)
                                    .hint_text(texts::marker_text_hint(locale))
                                    .desired_width(180.0),
                            );
                        });
                    }
                    MarkerShape::Arrow => {
                        xyz_row(ui, texts::marker_start(locale), &mut self.start, self.speed);
                        xyz_row(ui, texts::marker_end(locale), &mut self.end, self.speed);
                    }
                }
                // A text label with empty text would be invisible in the
                // viewport (and uneditable in the scene, display-types spec
                // §5 non-goal), so Add waits for actual text.
                let has_label_text =
                    self.shape != MarkerShape::Text || !self.text.trim().is_empty();
                ui.add_space(4.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            has_label_text,
                            egui::Button::new(texts::add_object_button(locale)),
                        )
                        .clicked()
                    {
                        add = true;
                    }
                });
            });
        if add {
            let marker = match self.shape {
                MarkerShape::Text => Marker::text(self.anchor, self.text.trim().to_owned()),
                MarkerShape::Arrow => Marker::arrow(self.start, self.end),
            };
            viewport::lock_state(state).add_marker(marker);
            if self.shape == MarkerShape::Text {
                self.text.clear();
            }
        }
    }
}

impl Default for AddMarkerDialog {
    fn default() -> Self {
        Self::new()
    }
}

/// One parameter row: a row label, then the three XYZ drag values prefixed
/// by their axis letters (texts.rs). Every value drags at `speed` units
/// per point.
fn xyz_row(ui: &mut egui::Ui, label: &str, value: &mut Vec3, speed: f32) {
    ui.horizontal(|ui| {
        ui.label(label);
        axis_drag(ui, texts::AXIS_X, &mut value.x, speed);
        axis_drag(ui, texts::AXIS_Y, &mut value.y, speed);
        axis_drag(ui, texts::AXIS_Z, &mut value.z, speed);
    });
}

fn axis_drag(ui: &mut egui::Ui, axis: &str, value: &mut f32, speed: f32) {
    let axis_color = ui.visuals().weak_text_color();
    ui.label(egui::RichText::new(axis).color(axis_color));
    ui.add(egui::DragValue::new(value).speed(speed));
}

/// Drag step matched to the scene scale: roughly `scale / 500` per drag
/// point (a full-width drag moves the value by ≈ a quarter of the scene),
/// floored so micro-scenes keep a workable step.
fn drag_speed(scale: f32) -> f32 {
    (scale * 0.002).max(1e-3)
}
