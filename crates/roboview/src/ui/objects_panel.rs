//! The fixed objects sidebar (display-types plan §3.4): the type-grouped
//! object tree (004 spec §6, A6) and the inline Add frame/marker forms it
//! expands under the action row (004 spec A5 — the floating Add windows of
//! the 002 era are gone, M3).
//!
//! Panel layout:
//!
//! - the heading;
//! - the action row: the **Fit** control (display-types spec §6: reframe
//!   the camera to the union of the measurable objects — disabled while
//!   nothing measurable is visible) and the two **Add** entries, which
//!   expand (and, re-clicked, collapse) the inline form below the row;
//! - the search field (004 spec §6 A6): filters member rows by name
//!   substring, case-insensitive;
//! - the tree: rows grouped by display type (point cloud / mesh / path /
//!   frame / marker groups in the canonical order of the display set).
//!
//! Tree semantics (004 spec §6 and the A6/A8 acceptance):
//!
//! - every group header carries the collapse triangle, the kind label, the
//!   group's default-color chip (opens egui's color picker; the color
//!   applies to *new* members only, decision D4), and the member count;
//! - every member row shows the name (truncated, dimmed while hidden) and
//!   the eye visibility toggle; a row click selects the object
//!   (`state.selected`, the selection source while picking is not yet
//!   wired), a right click selects and opens the row menu with Rename /
//!   Show|Hide / Delete (A8);
//! - Rename edits inline: Enter or a click-away commits the new name
//!   through the scene API, Escape cancels, an empty name cancels;
//! - Delete removes the object through [`roboview_core::scene::Scene::remove`]
//!   — the 002 A6 resource-ledger semantics of the previous list rows are
//!   unchanged, only the affordance moved from a per-row button into the
//!   row menu;
//! - filtering forces groups expanded; a filter without matches shows the
//!   no-match hint; an empty scene shows the empty hint instead of the
//!   search field.
//!
//! Session state (search text, collapsed groups, group colors, selection,
//! the pending rename, the open inline add form) lives in
//! [`ObjectsPanelState`], which the caller owns across frames. The scene
//! itself stays read-only for the panel body: rows are snapshotted once at
//! the top and the requested mutations — the [`TreeAction`]s and the
//! commits of the inline add forms — come back in the
//! [`ObjectsPanelOutput`] for the caller to apply under its scene lock, so
//! the panel never holds the lock while laying out widgets. Actions are
//! id-based and the scene never reuses ids, so an action for an object
//! removed meanwhile is a safe no-op.
//!
//! One entry: [`ui`] takes the app-owned [`ObjectsPanelState`] and returns
//! the [`ObjectsPanelOutput`] (tree actions + committed inline adds + the
//! camera reframe) for the caller (main.rs) to apply under its scene lock.
//!
//! The two Add entries of the action row are the 004 spec A5 doors: no
//! floating Add window exists anywhere (M3 — the 002 dialogs left with
//! T17). A click expands a small inline form beneath the row — one form at
//! a time, frame (origin + axis length) or marker (text label / arrow
//! radio with its parameters) — whose fields are seeded on open from the
//! visible scene bounds, exactly the defaults the dialogs' `open(center,
//! scale)` received from the viewport (`ui_defaults` of `viewport.rs`:
//! bounds center, axis length a quarter of the largest dimension, arrow
//! tip a fifth along +X). The Add button or Enter commits the draft
//! through the output and the caller adds under the viewport lock through
//! `ViewportState::add_frame`/`add_marker` (generated names, default
//! colors; inheriting the group default colors at add time is the T16
//! wiring, D4). Escape or a re-click of the open entry closes the form
//! without adding. All copy lives in `texts.rs`; every entry point takes
//! the current [`Locale`] (003 spec §6.2: explicit injection) and reads
//! its copy through `texts::…(locale)`.
//!
//! The scene lock is the same single uncontended lock the viewport panel
//! uses (`viewport::lock_state`): the panel never locks while painting —
//! the caller applies every mutation (tree actions and inline adds alike)
//! after the panel pass, under the lock and never while painting.

use eframe::egui;
use eframe::egui::widgets::color_picker::color_edit_button_srgb;
use glam::Vec3;

use roboview_core::displays::{DisplayKind, DisplayObject, Marker};
use roboview_core::io::{Aabb, Color};
use roboview_core::scene::Scene;

#[cfg(test)]
use roboview_core::scene::camera::OrbitCamera;

use super::texts::{self, Locale};
use super::theme::{SELECT_HIGHLIGHT, to_color32};

/// Height of every tree row (group headers and members alike).
const ROW_HEIGHT: f32 = 22.0;
/// Width reserved at the right of every tree row: the eye toggle on member
/// rows, the member count on group headers (mirrors the remove-button slot
/// of the flat-list era).
const RIGHT_SLOT: f32 = 26.0;
/// Horizontal inset of member rows under their group header.
const MEMBER_INDENT: f32 = 30.0;
/// Left zone of the group header holding the collapse triangle.
const TRIANGLE_ZONE: f32 = 16.0;
/// Width of the group default-color chip (egui's color button is fixed to
/// the spacing interact size, 40 × 18).
const CHIP_WIDTH: f32 = 40.0;
/// Alpha of the selection fill behind the selected row (the selection
/// highlight token is the A9 orange; the fill is its translucent form).
const SELECTION_FILL_ALPHA: f32 = 0.32;

/// The eye glyph of the per-row visibility toggle. A glyph invariant like
/// the `texts::OBJECTS_REMOVE` trash can, but tree-local: it is the
/// visibility icon of the tree column, not shared copy, so it stays here
/// next to the column it paints.
const EYE_GLYPH: &str = "👁";

/// The panel key of a tree object: its display type. Kept as a named alias
/// so the state contract reads in panel vocabulary (the display set of
/// display-types spec §7) instead of core type names.
pub type TypeKey = DisplayKind;

/// A requested scene mutation of the tree (004 spec A6/A8). Id-based:
/// every action names its object by stable scene id, so a stale action is
/// a safe no-op once the caller applies it (the scene never reuses ids).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeAction {
    /// Flip the object's visibility (the row's eye toggle).
    ToggleVisible(u64),
    /// Commit an inline rename (trimmed, non-empty).
    Rename { id: u64, name: String },
    /// Remove the object from the scene (row menu Delete).
    Delete(u64),
}

/// Panel output of the panel body: the mutations the caller should apply to
/// the live scene — the tree actions and the commits of the inline add
/// forms — plus the camera reframe request; everything the panel itself
/// cannot do with its read-only scene snapshot.
#[derive(Default)]
pub struct ObjectsPanelOutput {
    /// Tree actions to apply to the scene, in the order the panel queued
    /// them (see [`apply_actions`]).
    pub actions: Vec<TreeAction>,
    /// The Fit button was clicked: reframe the camera to the union of the
    /// measurable objects (`OrbitCamera::framing(bounds_union().as_ref())`).
    pub fit: bool,
    /// The inline Add frame form committed (its Add button or Enter): the
    /// caller adds the frame under its scene lock
    /// (`viewport::ViewportState::add_frame` — generated name, GPU upload
    /// through the line pipeline). The form closed with the commit.
    pub add_frame: Option<(Vec3, f32)>,
    /// The inline Add marker form committed (its Add button or Enter): the
    /// caller adds the marker under its scene lock
    /// (`viewport::ViewportState::add_marker`). The form stays open for
    /// repeat adds.
    pub add_marker: Option<Marker>,
}

/// Session state of the objects tree, owned by the caller across frames
/// (main.rs). Public fields are the panel's contract with the rest of the
/// app — the tree drives selection (004 spec §6: without picking, the
/// selection comes from the tree alone), and the later tasks
/// read/write the group colors when new members are created (T16 applies
/// them at add time; D4: new members only).
#[derive(Debug, Default)]
pub struct ObjectsPanelState {
    /// Live search text of the tree's filter field (matches object names,
    /// case-insensitive substring; an empty string shows everything).
    pub filter: String,
    /// Groups the user collapsed. Filtering forces every group expanded.
    pub group_collapsed: std::collections::HashSet<TypeKey>,
    /// User-set default colors per group, applied to *new* members of the
    /// kind only (004 decision D4). A kind without an entry falls back to
    /// [`GROUP_COLOR_UNSET`].
    pub group_default_color: std::collections::HashMap<TypeKey, Color>,
    /// The selected object, if any. Rows select on click and on right
    /// click; the scene id is never reused, so a stale id simply matches
    /// no row (pruned by [`ObjectsPanelState::prune`]).
    pub selected: Option<u64>,
    /// The object whose name is being edited inline, if any.
    pub renaming: Option<u64>,
    /// Text of the pending inline rename, committed by
    /// [`ObjectsPanelState::commit_rename`].
    rename_draft: String,
    /// The next drawn rename editor should request keyboard focus (set by
    /// [`ObjectsPanelState::begin_rename`], consumed by the row painter).
    rename_focus_pending: bool,
    /// The open inline Add form (004 spec A5), if any: the draft of the
    /// frame or marker form currently expanded under the action row.
    add_draft: Option<AddDraft>,
    /// The next drawn marker text field should request keyboard focus (set
    /// by [`ObjectsPanelState::open_add_marker`] and by the shape radio
    /// when switching back to the text label, consumed by `add_form_ui`).
    add_focus_pending: bool,
    /// Enter was held at the end of the previous panel frame: the
    /// key-repeat latch of the inline form's Enter-to-add (a held Enter
    /// commits once, not once per OS key repeat).
    enter_down: bool,
}

/// Default group color shown (and seeded into the picker) while the group
/// has no user-set color: a neutral light gray, deliberately not a
/// semantic token — it only marks "no default chosen yet".
pub const GROUP_COLOR_UNSET: Color = Color {
    r: 190,
    g: 190,
    b: 190,
};

impl ObjectsPanelState {
    /// Whether the group of `kind` is collapsed (only meaningful while no
    /// filter is active — filtering always expands groups).
    pub fn is_group_collapsed(&self, kind: TypeKey) -> bool {
        self.group_collapsed.contains(&kind)
    }

    /// Toggle the collapse state of the group of `kind`.
    pub fn toggle_group_collapsed(&mut self, kind: TypeKey) {
        if !self.group_collapsed.remove(&kind) {
            self.group_collapsed.insert(kind);
        }
    }

    /// The group's default color for new members: the user-set color, or
    /// [`GROUP_COLOR_UNSET`] while none is stored.
    pub fn group_color(&self, kind: TypeKey) -> Color {
        self.group_default_color
            .get(&kind)
            .copied()
            .unwrap_or(GROUP_COLOR_UNSET)
    }

    /// Store the group's default color for new members.
    pub fn set_group_color(&mut self, kind: TypeKey, color: Color) {
        self.group_default_color.insert(kind, color);
    }

    /// Start an inline rename of the object `id`, pre-filling the editor
    /// with its current name. The object becomes the selected one.
    pub fn begin_rename(&mut self, id: u64, current_name: &str) {
        self.renaming = Some(id);
        self.rename_draft = current_name.to_owned();
        self.rename_focus_pending = true;
        self.selected = Some(id);
    }

    /// Abort the pending inline rename, keeping the scene name unchanged.
    pub fn cancel_rename(&mut self) {
        self.renaming = None;
        self.rename_draft.clear();
        self.rename_focus_pending = false;
    }

    /// Finish the pending inline rename. Returns the trimmed new name for
    /// the object still being renamed; `None` when no rename is pending or
    /// the trimmed name is empty (an empty name cancels — the scene never
    /// stores blank names). The pending state is cleared either way.
    pub fn commit_rename(&mut self) -> Option<(u64, String)> {
        let id = self.renaming.take()?;
        let trimmed = std::mem::take(&mut self.rename_draft);
        self.rename_focus_pending = false;
        let trimmed = trimmed.trim().to_owned();
        (!trimmed.is_empty()).then_some((id, trimmed))
    }

    /// Drop the selection and any pending rename whose object is no longer
    /// in the scene (removed meanwhile — by the row menu, by a load
    /// replacement, or by a future task). Called at the top of every tree
    /// frame with the fresh scene snapshot.
    fn prune(&mut self, rows: &[Row]) {
        if self
            .selected
            .is_some_and(|id| !rows.iter().any(|row| row.id == id))
        {
            self.selected = None;
        }
        if self
            .renaming
            .is_some_and(|id| !rows.iter().any(|row| row.id == id))
        {
            self.cancel_rename();
        }
    }
}

impl ObjectsPanelState {
    // — Inline Add forms (004 spec A5; the 002 §7 F3/F4 add doors) —

    /// Open the inline Add frame form under the action row, seeding its
    /// draft from the visible-scene defaults the caller derived ([`add_defaults`]
    /// mirrors the viewport's `ui_defaults`: origin at the bounds center,
    /// axis length a quarter of the largest dimension). Replaces any open
    /// form — one form at a time. Both doors land here: the panel's own
    /// action row and the caller's shared Add action (main.rs dispatch).
    pub fn open_add_frame(&mut self, center: Vec3, scale: f32) {
        self.add_draft = Some(AddDraft::Frame(FrameDraft {
            origin: center,
            length: (scale * 0.25).max(1e-4),
            speed: drag_speed(scale),
        }));
        self.add_focus_pending = false;
    }

    /// Open the inline Add marker form under the action row, seeding its
    /// draft with the dialog defaults: the text-label shape, its anchor at
    /// the bounds center, empty label text, and the arrow's tail at the
    /// center with its tip a fifth of the largest dimension along +X.
    /// Replaces any open form and requests keyboard focus in the text
    /// field. Same doors as [`ObjectsPanelState::open_add_frame`].
    pub fn open_add_marker(&mut self, center: Vec3, scale: f32) {
        self.add_draft = Some(AddDraft::Marker(MarkerDraft {
            shape: MarkerShape::Text,
            anchor: center,
            text: String::new(),
            start: center,
            end: center + Vec3::X * (scale * 0.2).max(1e-4),
            speed: drag_speed(scale),
        }));
        self.add_focus_pending = true;
    }

    /// Close the inline add form, discarding its draft without adding
    /// (re-clicking the open action-row entry and Escape both land here).
    pub fn close_add(&mut self) {
        self.add_draft = None;
        self.add_focus_pending = false;
    }

    /// Commit the open frame form (its Add button or Enter). Returns the
    /// (origin, length) the caller adds under its scene lock and closes
    /// the form — a frame add is a one-shot placement, like the dialog it
    /// replaces. `None` while no frame form is open.
    fn commit_add_frame(&mut self) -> Option<(Vec3, f32)> {
        let draft = self.add_draft.as_mut()?;
        let AddDraft::Frame(frame) = draft else {
            return None;
        };
        let add = (frame.origin, frame.length);
        self.add_draft = None;
        Some(add)
    }

    /// Commit the open marker form (its Add button or Enter). Returns the
    /// marker the caller adds under its scene lock. A text label must
    /// carry actual text — an empty label would be invisible in the
    /// viewport and uneditable in the scene (display-types spec §5
    /// non-goal), so an empty-text commit is refused and the form stays
    /// open for typing. The form stays open after a commit for repeat
    /// adds, and a committed label clears its text field for the next
    /// entry (the dialog's semantics). `None` while no marker form is
    /// open.
    fn commit_add_marker(&mut self) -> Option<Marker> {
        let draft = self.add_draft.as_mut()?;
        let AddDraft::Marker(marker) = draft else {
            return None;
        };
        let marker_out = match marker.shape {
            MarkerShape::Text => {
                let text = marker.text.trim().to_owned();
                if text.is_empty() {
                    return None;
                }
                Marker::text(marker.anchor, text)
            }
            MarkerShape::Arrow => Marker::arrow(marker.start, marker.end),
        };
        if marker.shape == MarkerShape::Text {
            marker.text.clear();
        }
        Some(marker_out)
    }
}

/// Apply panel actions to the live scene. Every action is id-based, so
/// applying an action for an object that vanished meanwhile is a safe
/// no-op (the scene APIs report the miss). Callers hold their scene lock
/// across this call.
pub fn apply_actions(scene: &mut Scene<DisplayObject>, actions: &[TreeAction]) {
    for action in actions {
        match action {
            TreeAction::ToggleVisible(id) => {
                scene.toggle_visible(*id);
            }
            TreeAction::Rename { id, name } => {
                if let Some(object) = scene.get_mut(*id) {
                    object.name.clone_from(name);
                }
            }
            TreeAction::Delete(id) => {
                scene.remove(*id);
            }
        }
    }
}

/// Draw the objects sidebar into `ui` (used inside the left
/// [`egui::SidePanel`]) with the app-owned tree state. Returns the panel
/// output for the caller to apply under its scene lock: the tree actions,
/// the Fit request, and the committed inline adds (see module docs).
pub fn ui(
    ui: &mut egui::Ui,
    state: &mut ObjectsPanelState,
    scene: &Scene<DisplayObject>,
    locale: Locale,
) -> ObjectsPanelOutput {
    let rows = rows_from_scene(scene);
    // The Fit enablement is a scene read (bounds union over the measurable
    // objects); the reframe itself is a camera mutation the caller applies.
    // The same union seeds the inline add forms' defaults (see
    // [`add_defaults`]).
    let bounds = scene.bounds_union();
    let can_fit = bounds.is_some();
    let mut output = ObjectsPanelOutput::default();
    panel_body(
        ui,
        state,
        &rows,
        can_fit,
        add_defaults(bounds),
        locale,
        &mut output,
    );
    output
}

/// The shared panel chrome and tree body: the heading, the action row, the
/// inline Add form while one is open, the search field, and the grouped
/// tree (or its hints).
fn panel_body(
    ui: &mut egui::Ui,
    state: &mut ObjectsPanelState,
    rows: &[Row],
    can_fit: bool,
    defaults: (Vec3, f32),
    locale: Locale,
    output: &mut ObjectsPanelOutput,
) {
    ui.add_space(4.0);
    ui.heading(texts::objects_panel_title(locale));

    // Action row: Fit + the two UI-add entries. Wrapped so a narrow panel
    // folds the buttons instead of clipping them. The Add entries toggle
    // their inline form (004 spec A5): opening one kind replaces the other
    // and re-clicking the open entry closes the form again.
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
            output.fit = true;
        }
        let frame_open = matches!(state.add_draft, Some(AddDraft::Frame(_)));
        if ui
            .add(egui::Button::new(texts::objects_add_frame(locale)).selected(frame_open))
            .clicked()
        {
            if frame_open {
                state.close_add();
            } else {
                state.open_add_frame(defaults.0, defaults.1);
            }
        }
        let marker_open = matches!(state.add_draft, Some(AddDraft::Marker(_)));
        if ui
            .add(egui::Button::new(texts::objects_add_marker(locale)).selected(marker_open))
            .clicked()
        {
            if marker_open {
                state.close_add();
            } else {
                state.open_add_marker(defaults.0, defaults.1);
            }
        }
    });

    if state.add_draft.is_some() {
        ui.add_space(6.0);
        add_form_ui(ui, state, locale, output);
    }
    ui.add_space(6.0);
    tree_section(ui, state, rows, locale, output);

    // Refresh the Enter key-repeat latch of the inline form's Enter-to-add
    // every frame — also while no form is open, so a held Enter never
    // leaks into a form opened mid-hold.
    state.enter_down = ui.input(|i| i.key_down(egui::Key::Enter));
}

/// The search field and the grouped tree (or the empty / no-match hints).
/// Rows are a snapshot of the scene taken at the top of the frame; every
/// mutation the tree causes goes out through `output` and is applied by
/// the caller, never here.
fn tree_section(
    ui: &mut egui::Ui,
    state: &mut ObjectsPanelState,
    rows: &[Row],
    locale: Locale,
    output: &mut ObjectsPanelOutput,
) {
    // Drop state that points at objects that vanished this frame.
    state.prune(rows);

    if rows.is_empty() {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(texts::objects_empty_hint(locale))
                .color(ui.visuals().weak_text_color()),
        );
        return;
    }

    // Search field: filters member rows by name. Focused Escape clears it.
    let search = ui.add(
        egui::TextEdit::singleline(&mut state.filter)
            .hint_text(texts::tree_search_placeholder(locale))
            .desired_width(f32::INFINITY),
    );
    if search.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.filter.clear();
    }
    ui.add_space(4.0);

    let query = state.filter.trim();
    let filtering = !query.is_empty();
    let groups = group_members(rows, query);

    // A pending inline rename whose row this frame's view no longer draws
    // (its group was collapsed, or the filter stopped matching) cannot
    // observe the focus loss of the click that hid it; apply the same
    // "click-away commits" rule here, once.
    if let Some((id, name)) = commit_rename_if_hidden(state, filtering, &groups) {
        output.actions.push(TreeAction::Rename { id, name });
    }

    if groups.is_empty() {
        ui.label(
            egui::RichText::new(texts::tree_no_match_hint(locale))
                .color(ui.visuals().weak_text_color()),
        );
        return;
    }

    egui::ScrollArea::vertical()
        .id_salt("objects_tree")
        .auto_shrink([false, false])
        .max_height(ui.available_height())
        .show(ui, |ui| {
            for (kind, members) in groups {
                let collapsed = !filtering && state.is_group_collapsed(kind);
                group_header_ui(ui, state, kind, members.len(), collapsed, locale);
                if !collapsed {
                    for row in members {
                        member_row_ui(ui, state, row, locale, output);
                    }
                }
            }
        });
}

/// Finish the pending inline rename when its row is hidden from the
/// current view (see [`tree_section`]). Rows that are simply gone from the
/// scene are handled by [`ObjectsPanelState::prune`] instead.
fn commit_rename_if_hidden(
    state: &mut ObjectsPanelState,
    filtering: bool,
    groups: &[(DisplayKind, Vec<&Row>)],
) -> Option<(u64, String)> {
    let id = state.renaming?;
    let visible = if filtering {
        groups
            .iter()
            .any(|group| group.1.iter().any(|row| row.id == id))
    } else {
        groups
            .iter()
            .find(|group| group.1.iter().any(|row| row.id == id))
            .is_some_and(|group| !state.is_group_collapsed(group.0))
    };
    if visible { None } else { state.commit_rename() }
}

/// One group header row: the collapse triangle, the kind label, the
/// group's default-color chip (opens egui's color picker; stored into
/// `state` for new members of the kind, D4), and the member count.
/// Clicking anywhere on the row outside the chip toggles the group.
fn group_header_ui(
    ui: &mut egui::Ui,
    state: &mut ObjectsPanelState,
    kind: DisplayKind,
    member_count: usize,
    collapsed: bool,
    locale: Locale,
) {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_HEIGHT),
        egui::Sense::click(),
    );
    if response.clicked() {
        state.toggle_group_collapsed(kind);
    }

    // The group band: a subtle divider fill under the header, strengthened
    // while hovered so the row reads as one click target.
    let fill = if response.hovered() {
        ui.visuals().widgets.hovered.weak_bg_fill
    } else {
        ui.visuals().faint_bg_color
    };
    ui.painter().rect_filled(rect, 2.0, fill);

    // The row content runs in its own ui anchored on the band (the row
    // response above is already registered, so clicks on these children
    // win over the row within their rects).
    let mut inner = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(kind)
            .max_rect(rect.shrink2(egui::vec2(0.0, 2.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    inner.add_space(TRIANGLE_ZONE);
    let band = inner.available_height();

    // Kind label, truncated to leave room for the chip and the count.
    let label_width = (inner.available_width() - (CHIP_WIDTH + RIGHT_SLOT + 16.0)).max(0.0);
    inner.add_sized(
        egui::vec2(label_width, band),
        egui::Label::new(egui::RichText::new(texts::object_kind_label(locale, kind)).strong())
            .truncate(),
    );

    // Default-color chip: the egui color button (a swatch that opens the
    // picker). The click is consumed here, not by the header toggle.
    let mut srgb = [
        state.group_color(kind).r,
        state.group_color(kind).g,
        state.group_color(kind).b,
    ];
    let chip = color_edit_button_srgb(&mut inner, &mut srgb)
        .on_hover_text(texts::group_default_color(locale));
    if chip.changed() {
        state.set_group_color(
            kind,
            Color {
                r: srgb[0],
                g: srgb[1],
                b: srgb[2],
            },
        );
    }

    // The member count, pinned to the row's right edge (the same rail the
    // member eyes sit on).
    inner.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(
            egui::RichText::new(member_count.to_string())
                .color(ui.visuals().weak_text_color())
                .size(11.0),
        );
    });

    // The collapse triangle, painted as a vector glyph (the ▲/▼ code
    // points are missing from the bundled font chain; ▶ exists but the two
    // states must match, so both are drawn): pointing right while
    // collapsed, pointing down while expanded. Decorative — the click
    // target is the whole header row.
    let painter = ui.painter();
    let color = ui.visuals().weak_text_color();
    let center = egui::pos2(rect.left() + 7.0, rect.center().y);
    let arm = 3.5;
    let points = if collapsed {
        // Right-pointing triangle.
        vec![
            egui::pos2(center.x - arm * 0.6, center.y - arm),
            egui::pos2(center.x - arm * 0.6, center.y + arm),
            egui::pos2(center.x + arm, center.y),
        ]
    } else {
        // Down-pointing triangle.
        vec![
            egui::pos2(center.x - arm, center.y - arm * 0.6),
            egui::pos2(center.x + arm, center.y - arm * 0.6),
            egui::pos2(center.x, center.y + arm),
        ]
    };
    painter.add(egui::Shape::convex_polygon(
        points,
        color,
        egui::Stroke::NONE,
    ));
}

/// One member row: background (selection highlight / hover), the name
/// (inline editor while renaming, dimmed while hidden), and the eye
/// visibility toggle pinned right. A click or a right click selects the
/// object; the right click also opens the row menu.
fn member_row_ui(
    ui: &mut egui::Ui,
    state: &mut ObjectsPanelState,
    row: &Row,
    locale: Locale,
    output: &mut ObjectsPanelOutput,
) {
    let renaming = state.renaming == Some(row.id);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_HEIGHT),
        egui::Sense::click(),
    );

    // Selection: any row click — also the right click that opens the menu
    // (004 spec §6: the selection comes from the tree).
    if response.clicked() || response.secondary_clicked() {
        state.selected = Some(row.id);
    }
    paint_row_background(ui, rect, state.selected == Some(row.id), response.hovered());

    let mut inner = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(row.id)
            .max_rect(rect.shrink2(egui::vec2(0.0, 2.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let band = inner.available_height();
    inner.add_space(MEMBER_INDENT);

    // The name slot: inline editor while renaming, truncated label
    // otherwise. The eye keeps its right slot in both states.
    let name_width = (inner.available_width() - RIGHT_SLOT).max(0.0);
    if renaming {
        let edit = inner.add_sized(
            egui::vec2(name_width.max(40.0), band),
            egui::TextEdit::singleline(&mut state.rename_draft)
                .frame(false)
                .margin(egui::Margin::symmetric(4, 0)),
        );
        let first_frame = state.rename_focus_pending;
        state.rename_focus_pending = false;
        if first_frame {
            edit.request_focus();
        }
        let escape = edit.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape));
        if escape {
            // Escape cancels the inline edit and releases the editor.
            state.cancel_rename();
            edit.surrender_focus();
        } else if edit.lost_focus() {
            // Enter (single-line editors surrender focus on Enter) and
            // click-aways both land here: commit the typed name.
            if let Some((id, name)) = state.commit_rename() {
                output.actions.push(TreeAction::Rename { id, name });
            }
        }
    } else {
        let name_color = if row.visible {
            ui.visuals().text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        inner.add_sized(
            egui::vec2(name_width, band),
            egui::Label::new(egui::RichText::new(&row.name).color(name_color)).truncate(),
        );
    }

    // The eye visibility toggle, pinned to the row's right edge.
    inner.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let eye_color = if row.visible {
            ui.visuals().text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        let eye = ui
            .small_button(egui::RichText::new(EYE_GLYPH).color(eye_color))
            .on_hover_text(texts::context_toggle_visible(locale));
        if eye.clicked() {
            state.selected = Some(row.id);
            output.actions.push(TreeAction::ToggleVisible(row.id));
        }
    });

    // Row menu (A8): Rename / Show|Hide / Delete. The popup opens from the
    // row's own response, so it also works for right clicks over the name.
    response.context_menu(|ui| {
        member_context_menu(ui, state, row, locale, output);
    });
}

/// The three items of the member-row menu (004 spec §6 A8): Rename (starts
/// the inline edit), the visibility toggle (labeled by the current state,
/// Show or Hide), and Delete (the scene removal — same A6 semantics as the
/// flat-list trash button it replaces; the trash glyph and its tooltip stay
/// on the item).
fn member_context_menu(
    ui: &mut egui::Ui,
    state: &mut ObjectsPanelState,
    row: &Row,
    locale: Locale,
    output: &mut ObjectsPanelOutput,
) {
    if ui.button(texts::context_rename(locale)).clicked() {
        state.begin_rename(row.id, &row.name);
        ui.close();
    }
    let visibility_label = if row.visible {
        texts::context_hide(locale)
    } else {
        texts::context_show(locale)
    };
    if ui.button(visibility_label).clicked() {
        output.actions.push(TreeAction::ToggleVisible(row.id));
        ui.close();
    }
    if ui
        .button(egui::RichText::new(format!(
            "{} {}",
            texts::OBJECTS_REMOVE,
            texts::context_delete(locale)
        )))
        .on_hover_text(texts::objects_remove_tooltip(locale))
        .clicked()
    {
        output.actions.push(TreeAction::Delete(row.id));
        ui.close();
    }
}

/// Paint the row background: the selection highlight (A9 orange token at
/// low alpha, plus a full-strength left accent) for the selected row, and
/// the subtle hover fill otherwise.
fn paint_row_background(ui: &egui::Ui, rect: egui::Rect, selected: bool, hovered: bool) {
    if selected {
        let highlight = to_color32(SELECT_HIGHLIGHT);
        ui.painter()
            .rect_filled(rect, 2.0, highlight.gamma_multiply(SELECTION_FILL_ALPHA));
        let accent =
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + 2.0, rect.bottom()));
        ui.painter().rect_filled(accent, 0.0, highlight);
    } else if hovered {
        ui.painter()
            .rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
}

/// One snapshot row of the tree: the object's display fields, copied from
/// the scene at the top of the frame so the body never reads the scene
/// while laying out (and the caller may release its lock while painting).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    /// Stable scene id of the object.
    id: u64,
    /// Its display type — the group the row is shown under.
    kind: DisplayKind,
    /// Current visibility (the eye toggle paints from this snapshot).
    visible: bool,
    /// Current name (shown, filtered, and renamed against this snapshot).
    name: String,
}

/// Snapshot every object of `scene` into tree rows, in scene (add) order.
fn rows_from_scene(scene: &Scene<DisplayObject>) -> Vec<Row> {
    scene
        .iter()
        .map(|object| Row {
            id: object.id,
            kind: object.object.kind(),
            visible: object.visible,
            name: object.name.clone(),
        })
        .collect()
}

/// Group the rows by display type, in the canonical order of the display
/// set (display-types spec §7: point cloud, mesh, path, frame, marker),
/// each group keeping the scene's add order. A non-empty `filter` keeps
/// only the matching rows (case-insensitive substring over the name) and
/// drops groups left without matches; an empty filter keeps every row.
fn group_members<'a>(rows: &'a [Row], filter: &str) -> Vec<(DisplayKind, Vec<&'a Row>)> {
    let filter = filter.trim();
    let mut members: Vec<(DisplayKind, Vec<&Row>)> = Vec::with_capacity(rows.len());
    for row in rows {
        if !filter.is_empty() && !matches_query(filter, &row.name) {
            continue;
        }
        match members.iter_mut().find(|group| group.0 == row.kind) {
            Some((_, list)) => list.push(row),
            None => members.push((row.kind, vec![row])),
        }
    }
    members.sort_by_key(|group| kind_order(group.0));
    members
}

/// Position of `kind` in the canonical group order of the tree.
fn kind_order(kind: DisplayKind) -> u8 {
    match kind {
        DisplayKind::PointCloud => 0,
        DisplayKind::Mesh => 1,
        DisplayKind::Path => 2,
        DisplayKind::Frame => 3,
        DisplayKind::Marker => 4,
    }
}

/// Whether `name` matches the search `filter`: a case-insensitive
/// substring test (the filter itself is already trimmed).
fn matches_query(filter: &str, name: &str) -> bool {
    name.to_lowercase().contains(&filter.to_lowercase())
}

/// The chosen marker shape of the inline Add marker form (display-types
/// spec §7 F4): a viewport overlay text label or a 3D arrow with a head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkerShape {
    Text,
    Arrow,
}

/// Draft parameters of the inline frame form (004 spec A5).
#[derive(Debug, Clone, PartialEq)]
struct FrameDraft {
    /// Frame origin (three XYZ drag values).
    origin: Vec3,
    /// Axis length (non-negative; zero or negative lengths draw no
    /// geometry in the line pipeline).
    length: f32,
    /// Drag step of every DragValue, matched to the scene scale at open
    /// time (see [`drag_speed`]).
    speed: f32,
}

/// Draft parameters of the inline marker form (004 spec A5).
#[derive(Debug, Clone, PartialEq)]
struct MarkerDraft {
    /// Chosen shape (text label or arrow).
    shape: MarkerShape,
    /// Anchor of the text label (its world point).
    anchor: Vec3,
    /// Label text, committed trimmed and non-empty (an empty label would
    /// be invisible, display-types spec §5 non-goal).
    text: String,
    /// Arrow tail.
    start: Vec3,
    /// Arrow tip.
    end: Vec3,
    /// Drag step of every DragValue, matched to the scene scale at open
    /// time (see [`drag_speed`]).
    speed: f32,
}

/// The draft of the open inline Add form: the editable parameters of the
/// frame or marker form expanded under the action row. Seeded on open from
/// the visible-scene defaults ([`add_defaults`], the dialog-era `open`
/// semantics); the Add button or Enter commits it through the output,
/// Escape or a re-click of the open entry discards it.
#[derive(Debug, Clone, PartialEq)]
enum AddDraft {
    /// A coordinate frame: origin plus axis length.
    Frame(FrameDraft),
    /// A marker: the shape radio plus the shape's parameters.
    Marker(MarkerDraft),
}

/// Fallback scene scale the inline add defaults use while the scene holds
/// nothing measurable: the mirror of `viewport.rs`'s private
/// `DEFAULT_UI_SCALE`, the value the removed dialogs received from the
/// viewport's `ui_defaults` on an empty or frame/marker-only scene. The
/// pairing keeps the empty-scene add defaults (world origin, frame axis
/// length 2.5) identical to the dialog era.
const ADD_UI_SCALE_FALLBACK: f32 = 10.0;

/// The (center, scale) pair the inline add forms seed their fields from on
/// open: the center and largest dimension of the visible bounds union, or
/// the origin/fallback-scale pair when the scene holds nothing measurable.
/// Mirrors the viewport's `ui_defaults` — same input (the same live scene
/// snapshot) and same outputs — so a form can be seeded in the same frame
/// its entry opens, without the panel touching the viewport lock.
fn add_defaults(bounds: Option<Aabb>) -> (Vec3, f32) {
    let Some(bounds) = bounds else {
        return (Vec3::ZERO, ADD_UI_SCALE_FALLBACK);
    };
    let extent = bounds.largest_dimension();
    if !extent.is_finite() || extent <= 0.0 {
        // Degenerate union (a single-point cloud): frame its center at the
        // fallback scale, like the viewport's own `ui_defaults`.
        (bounds.center(), ADD_UI_SCALE_FALLBACK)
    } else {
        (bounds.center(), extent)
    }
}

/// The inline Add form under the action row (004 spec A5): the parameter
/// rows of the open draft and its commit affordances, drawn only while
/// `ObjectsPanelState::add_draft` is open. The Add button and Enter commit
/// the draft into `output` (`add_frame`/`add_marker`, for the caller to
/// add under its scene lock); Escape and re-clicking the open entry close
/// the form without adding.
///
/// Enter-to-add is gated so the panel never steals the keys of another
/// text input: while any widget outside the form holds the keyboard (the
/// tree's search or rename editors, a properties-panel field), Enter and
/// Escape stay with that widget — the form's own label field counts as the
/// form. A held Enter commits once: `ObjectsPanelState::enter_down` is the
/// key-repeat latch, refreshed every frame at the end of [`panel_body`].
fn add_form_ui(
    ui: &mut egui::Ui,
    state: &mut ObjectsPanelState,
    locale: Locale,
    output: &mut ObjectsPanelOutput,
) {
    let enter_down = ui.input(|i| i.key_down(egui::Key::Enter));
    let enter_pressed = enter_down && !state.enter_down;
    let focus_pending = std::mem::take(&mut state.add_focus_pending);

    let kind_is_frame = matches!(state.add_draft, Some(AddDraft::Frame(_)));
    let title = if kind_is_frame {
        texts::add_frame_window_title(locale)
    } else {
        texts::add_marker_window_title(locale)
    };

    let mut submit = false;
    let mut close = false;
    let mut refocus_text = false;
    let mut text_has_focus = false;
    let mut text_field: Option<egui::Response> = None;

    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(egui::RichText::new(title).strong());
        ui.add_space(4.0);

        let mut shape_is_text = false;
        let mut text_empty = false;
        match state.add_draft.as_mut() {
            Some(AddDraft::Frame(frame)) => {
                xyz_row(
                    ui,
                    texts::add_frame_origin(locale),
                    &mut frame.origin,
                    frame.speed,
                );
                ui.horizontal(|ui| {
                    ui.label(texts::add_frame_length(locale));
                    // Negative or zero lengths draw no geometry in the line
                    // pipeline, so the drag clamps to non-negative values —
                    // the removed dialog's rule, unchanged.
                    ui.add(
                        egui::DragValue::new(&mut frame.length)
                            .speed(frame.speed)
                            .range(0.0..=f32::MAX),
                    );
                });
            }
            Some(AddDraft::Marker(marker)) => {
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .selectable_value(
                            &mut marker.shape,
                            MarkerShape::Text,
                            texts::marker_shape_text(locale),
                        )
                        .clicked()
                    {
                        // Back on the text shape: pull keyboard focus into
                        // the label field for typing.
                        state.add_focus_pending = true;
                    }
                    ui.selectable_value(
                        &mut marker.shape,
                        MarkerShape::Arrow,
                        texts::marker_shape_arrow(locale),
                    );
                });
                ui.add_space(4.0);
                match marker.shape {
                    MarkerShape::Text => {
                        shape_is_text = true;
                        text_empty = marker.text.trim().is_empty();
                        xyz_row(
                            ui,
                            texts::marker_anchor(locale),
                            &mut marker.anchor,
                            marker.speed,
                        );
                        ui.horizontal(|ui| {
                            ui.label(texts::marker_text(locale));
                            let edit = ui.add(
                                egui::TextEdit::singleline(&mut marker.text)
                                    .hint_text(texts::marker_text_hint(locale))
                                    .desired_width(f32::INFINITY),
                            );
                            if std::mem::take(&mut state.add_focus_pending) || focus_pending {
                                edit.request_focus();
                            }
                            text_field = Some(edit);
                        });
                    }
                    MarkerShape::Arrow => {
                        xyz_row(
                            ui,
                            texts::marker_start(locale),
                            &mut marker.start,
                            marker.speed,
                        );
                        xyz_row(ui, texts::marker_end(locale), &mut marker.end, marker.speed);
                    }
                }
            }
            None => {}
        }

        text_has_focus = text_field.as_ref().is_some_and(egui::Response::has_focus);
        // Enter and Escape belong to the form unless an outside widget
        // holds the keyboard; the form's own label field is the form.
        let outside_takes_keys = ui.ctx().wants_keyboard_input() && !text_has_focus;
        let can_submit = kind_is_frame || !shape_is_text || !text_empty;

        if ui.input(|i| i.key_pressed(egui::Key::Escape)) && !outside_takes_keys {
            close = true;
        }

        ui.add_space(4.0);
        // The confirm button, pinned right of the card. A text label with
        // empty text would be invisible in the viewport (and uneditable in
        // the scene, display-types spec §5 non-goal), so its commit waits
        // for actual text — the removed dialog's rule, unchanged.
        let mut add_clicked = false;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            add_clicked = ui
                .add_enabled(
                    can_submit,
                    egui::Button::new(texts::add_object_button(locale)),
                )
                .clicked();
        });
        if add_clicked || (enter_pressed && !outside_takes_keys && can_submit) {
            submit = true;
        }
        // A keyboard commit — or an Enter that found nothing to commit on
        // the text shape — pulls focus back into the label field for the
        // next entry.
        if shape_is_text
            && !outside_takes_keys
            && (enter_pressed || (add_clicked && text_has_focus))
        {
            refocus_text = true;
        }
    });

    if close {
        // The focused widget will not be drawn next frame; egui drops its
        // focus then, but surrendering here also ends any in-flight edit.
        if let Some(edit) = &text_field {
            if edit.has_focus() {
                edit.surrender_focus();
            }
        }
        state.close_add();
    }
    if submit {
        if kind_is_frame {
            if let Some(add) = state.commit_add_frame() {
                output.add_frame = Some(add);
            }
        } else if let Some(add) = state.commit_add_marker() {
            output.add_marker = Some(add);
        }
    }
    if refocus_text {
        if let Some(edit) = text_field {
            edit.request_focus();
        }
    }
}

/// One parameter row of an inline add form: the row label, then the three
/// XYZ drag values, each prefixed by its axis letter (texts.rs). Wrapped
/// so a narrow sidebar folds the row instead of clipping it; every value
/// drags at `speed` units per point.
fn xyz_row(ui: &mut egui::Ui, label: &str, value: &mut Vec3, speed: f32) {
    ui.horizontal_wrapped(|ui| {
        ui.label(label);
        axis_drag(ui, texts::AXIS_X, &mut value.x, speed);
        axis_drag(ui, texts::AXIS_Y, &mut value.y, speed);
        axis_drag(ui, texts::AXIS_Z, &mut value.z, speed);
    });
}

fn axis_drag(ui: &mut egui::Ui, axis: &str, value: &mut f32, speed: f32) {
    // The axis letter rides inside the drag value so a wrapped row never
    // splits a letter from its value.
    ui.add(egui::DragValue::new(value).speed(speed).prefix(axis));
}

/// Drag step matched to the scene scale: roughly `scale / 500` per drag
/// point (a full-width drag moves the value by ≈ a quarter of the scene),
/// floored so micro-scenes keep a workable step.
fn drag_speed(scale: f32) -> f32 {
    (scale * 0.002).max(1e-3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use roboview_core::displays::Frame;

    /// A tiny scene with one object of every kind, named so filters can
    /// target individual rows; the second frame is hidden.
    fn mixed_scene() -> Scene<DisplayObject> {
        let mut scene = Scene::new(OrbitCamera::new(Vec3::ZERO));
        scene.add(
            DisplayObject::Frame(Frame::new(Vec3::ZERO, 1.0)),
            "origin frame",
        );
        scene.add(
            DisplayObject::Marker(Marker::text(Vec3::ZERO, "label")),
            "note marker",
        );
        let hidden = scene.add(
            DisplayObject::Frame(Frame::new(Vec3::X, 1.0)),
            "hidden frame",
        );
        // Objects start visible; hide the third one for the snapshot tests.
        scene.toggle_visible(hidden);
        scene
    }

    fn marker_scene() -> Scene<DisplayObject> {
        let mut scene = Scene::new(OrbitCamera::new(Vec3::ZERO));
        scene.add(
            DisplayObject::Marker(Marker::arrow(Vec3::ZERO, Vec3::X)),
            "arrow A",
        );
        scene.add(
            DisplayObject::Marker(Marker::text(Vec3::ZERO, "hey")),
            "label B",
        );
        scene
    }

    fn frame_scene() -> Scene<DisplayObject> {
        let mut scene = Scene::new(OrbitCamera::new(Vec3::ZERO));
        scene.add(
            DisplayObject::Frame(Frame::new(Vec3::ZERO, 1.0)),
            "world frame",
        );
        scene
    }

    #[test]
    fn rows_snapshot_scene_in_add_order() {
        let mut scene = mixed_scene();
        let rows = rows_from_scene(&scene);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].name, "origin frame");
        assert_eq!(rows[0].kind, DisplayKind::Frame);
        assert!(rows[0].visible);
        assert_eq!(rows[1].name, "note marker");
        assert_eq!(rows[1].kind, DisplayKind::Marker);
        assert_eq!(rows[2].name, "hidden frame");
        assert!(!rows[2].visible);

        // The snapshot is detached from the live scene: mutating the scene
        // afterwards does not change previously taken rows.
        scene.toggle_visible(rows[0].id);
        let again = rows_from_scene(&scene);
        assert!(!again[0].visible);
        assert!(rows[0].visible, "snapshot must copy, not alias");
    }

    #[test]
    fn matches_query_is_case_insensitive_substring() {
        assert!(matches_query("marker", "Marker 3"));
        assert!(matches_query("MARKER", "Marker 3"));
        assert!(matches_query("3", "Marker 3"));
        assert!(!matches_query("frame", "Marker 3"));
        assert!(matches_query("note", "note marker"));
        assert!(matches_query("NOTE", "Note Marker"));
        assert!(!matches_query("note", "nothing here"));
    }

    #[test]
    fn group_members_orders_kinds_canonically_and_skips_empty() {
        let rows = rows_from_scene(&mixed_scene());
        let groups = group_members(&rows, "");
        // Canonical display-set order (point cloud, mesh, path, frame,
        // marker); only the kinds present appear, each in add order.
        let kinds: Vec<DisplayKind> = groups.iter().map(|group| group.0).collect();
        assert_eq!(kinds, vec![DisplayKind::Frame, DisplayKind::Marker]);
        assert_eq!(
            groups[0].1.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![rows[0].id, rows[2].id],
            "a group keeps the scene's add order"
        );
    }

    #[test]
    fn group_members_filters_rows_and_drops_empty_groups() {
        let rows = rows_from_scene(&mixed_scene());
        let groups = group_members(&rows, "MARKER");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].0, DisplayKind::Marker);
        assert_eq!(groups[0].1.len(), 1);
        assert_eq!(groups[0].1[0].name, "note marker");

        assert!(
            group_members(&rows, "point").is_empty(),
            "no match -> no groups"
        );
    }

    #[test]
    fn group_members_empty_filter_keeps_every_row() {
        let rows = rows_from_scene(&marker_scene());
        let groups = group_members(&rows, "");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].1.len(), 2);
        let groups = group_members(&rows, "   ");
        assert_eq!(groups.len(), 1, "whitespace-only filters match everything");
    }

    #[test]
    fn toggle_group_collapsed_round_trips() {
        let mut state = ObjectsPanelState::default();
        assert!(!state.is_group_collapsed(DisplayKind::Mesh));
        state.toggle_group_collapsed(DisplayKind::Mesh);
        assert!(state.is_group_collapsed(DisplayKind::Mesh));
        state.toggle_group_collapsed(DisplayKind::Mesh);
        assert!(!state.is_group_collapsed(DisplayKind::Mesh));
        // Kinds are independent.
        state.toggle_group_collapsed(DisplayKind::Marker);
        assert!(!state.is_group_collapsed(DisplayKind::Mesh));
        assert!(state.is_group_collapsed(DisplayKind::Marker));
    }

    #[test]
    fn group_color_falls_back_until_set() {
        let mut state = ObjectsPanelState::default();
        assert_eq!(state.group_color(DisplayKind::Mesh), GROUP_COLOR_UNSET);
        let orange = Color {
            r: 255,
            g: 128,
            b: 0,
        };
        state.set_group_color(DisplayKind::Mesh, orange);
        assert_eq!(state.group_color(DisplayKind::Mesh), orange);
        assert_eq!(
            state.group_color(DisplayKind::Path),
            GROUP_COLOR_UNSET,
            "other kinds keep their fallback"
        );
    }

    #[test]
    fn begin_commit_cancel_rename_round_trip() {
        let mut state = ObjectsPanelState::default();
        assert_eq!(state.commit_rename(), None, "no rename pending");

        state.begin_rename(7, "old name");
        assert_eq!(state.renaming, Some(7));
        assert_eq!(
            state.selected,
            Some(7),
            "beginning a rename selects the row"
        );

        state.rename_draft.push_str(" + trimmed");
        assert_eq!(
            state.commit_rename(),
            Some((7, "old name + trimmed".to_owned()))
        );
        assert_eq!(state.renaming, None);
        assert_eq!(
            state.commit_rename(),
            None,
            "committing clears the pending state"
        );

        // Empty (or blank) names cancel instead of renaming to nothing.
        state.begin_rename(8, "keep me");
        state.rename_draft.clear();
        state.rename_draft.push_str("   ");
        assert_eq!(state.commit_rename(), None);
        assert_eq!(state.renaming, None);

        state.begin_rename(9, "abort me");
        state.rename_draft.push('!');
        state.cancel_rename();
        assert_eq!(state.renaming, None);
        assert_eq!(state.commit_rename(), None, "cancel discards the draft");
    }

    #[test]
    fn prune_drops_selection_and_rename_of_removed_objects() {
        let rows = rows_from_scene(&frame_scene());
        let mut state = ObjectsPanelState {
            selected: Some(rows[0].id),
            renaming: Some(rows[0].id),
            ..ObjectsPanelState::default()
        };
        state.prune(&rows);
        assert_eq!(state.selected, Some(rows[0].id));
        assert_eq!(state.renaming, Some(rows[0].id));

        // The object vanished: both pointers drop.
        state.prune(&[]);
        assert_eq!(state.selected, None);
        assert_eq!(state.renaming, None);
    }

    #[test]
    fn commit_rename_if_hidden_commits_only_hidden_edits() {
        let rows = rows_from_scene(&mixed_scene());
        let groups = group_members(&rows, "");
        let mut state = ObjectsPanelState::default();

        // The renamed row is visible: no commit.
        state.begin_rename(rows[0].id, &rows[0].name);
        assert_eq!(commit_rename_if_hidden(&mut state, false, &groups), None);
        assert_eq!(state.renaming, Some(rows[0].id));

        // Collapsed group: the edit cannot be drawn, so it commits once.
        state.toggle_group_collapsed(DisplayKind::Frame);
        assert!(commit_rename_if_hidden(&mut state, false, &groups).is_some());
        assert_eq!(state.renaming, None);

        // Filtering: a row excluded by the filter commits as well.
        state.begin_rename(rows[0].id, &rows[0].name);
        let filtered = group_members(&rows, "marker");
        assert!(commit_rename_if_hidden(&mut state, true, &filtered).is_some());
        assert_eq!(state.renaming, None);
    }

    #[test]
    fn apply_actions_mutates_the_scene_and_misses_are_no_ops() {
        let mut scene = mixed_scene();
        let rows = rows_from_scene(&scene);

        apply_actions(
            &mut scene,
            &[
                TreeAction::ToggleVisible(rows[0].id),
                TreeAction::Rename {
                    id: rows[1].id,
                    name: "renamed".to_owned(),
                },
                TreeAction::Delete(rows[2].id),
                // Stale actions (ids never reused) are safe no-ops.
                TreeAction::ToggleVisible(99),
                TreeAction::Rename {
                    id: 99,
                    name: "ghost".to_owned(),
                },
                TreeAction::Delete(99),
            ],
        );

        assert!(!scene.get(rows[0].id).unwrap().visible);
        assert_eq!(scene.get(rows[1].id).unwrap().name, "renamed");
        assert!(scene.get(rows[2].id).is_none());
        assert_eq!(rows_from_scene(&scene).len(), 2);
    }

    // — Inline Add forms (004 spec A5) —

    #[test]
    fn add_defaults_mirrors_viewport_ui_defaults() {
        // Nothing measurable: the world origin at the fallback scale — the
        // viewport's `DEFAULT_UI_SCALE` mirror, the defaults the removed
        // dialogs received for an empty or frame/marker-only scene.
        assert_eq!(add_defaults(None), (Vec3::ZERO, ADD_UI_SCALE_FALLBACK));

        // A degenerate union (a single-point cloud) frames the point's
        // center at the fallback scale, like the viewport's own fallback.
        let point = Aabb {
            min: Vec3::new(1.0, -2.0, 3.0),
            max: Vec3::new(1.0, -2.0, 3.0),
        };
        assert_eq!(
            add_defaults(Some(point)),
            (Vec3::new(1.0, -2.0, 3.0), ADD_UI_SCALE_FALLBACK)
        );

        // A measurable union: the center plus its largest dimension.
        let boxed = Aabb {
            min: Vec3::ZERO,
            max: Vec3::new(1.0, 0.5, 0.25),
        };
        assert_eq!(
            add_defaults(Some(boxed)),
            (Vec3::new(0.5, 0.25, 0.125), 1.0)
        );
    }

    #[test]
    fn open_add_frame_seeds_the_dialog_defaults() {
        let mut state = ObjectsPanelState::default();
        state.open_add_frame(Vec3::new(1.0, 2.0, 3.0), 4.0);
        match &state.add_draft {
            Some(AddDraft::Frame(frame)) => {
                assert_eq!(frame.origin, Vec3::new(1.0, 2.0, 3.0));
                assert_eq!(frame.length, 1.0, "a quarter of the scene scale");
                assert_eq!(frame.speed, drag_speed(4.0));
            }
            _ => panic!("expected an open frame form"),
        }
        assert!(
            !state.add_focus_pending,
            "the frame form never requests text focus"
        );
    }

    #[test]
    fn open_add_marker_seeds_the_dialog_defaults() {
        let mut state = ObjectsPanelState::default();
        state.open_add_marker(Vec3::new(1.0, -1.0, 2.0), 5.0);
        match &state.add_draft {
            Some(AddDraft::Marker(marker)) => {
                assert_eq!(marker.shape, MarkerShape::Text);
                assert_eq!(marker.anchor, Vec3::new(1.0, -1.0, 2.0));
                assert!(marker.text.is_empty(), "the label starts empty");
                assert_eq!(marker.start, Vec3::new(1.0, -1.0, 2.0));
                assert_eq!(
                    marker.end,
                    Vec3::new(2.0, -1.0, 2.0),
                    "the arrow tip waits a fifth of the scale along +X"
                );
                assert_eq!(marker.speed, drag_speed(5.0));
            }
            _ => panic!("expected an open marker form"),
        }
        assert!(
            state.add_focus_pending,
            "the text shape requests the label field's focus"
        );
    }

    #[test]
    fn toggle_and_switch_of_the_inline_forms() {
        let mut state = ObjectsPanelState::default();
        assert!(state.add_draft.is_none(), "no form open by default");

        // Opening a kind and closing it again (the re-click toggle).
        state.open_add_frame(Vec3::ZERO, 1.0);
        assert!(matches!(state.add_draft, Some(AddDraft::Frame(_))));
        state.close_add();
        assert!(state.add_draft.is_none());

        // Opening one kind replaces the other (one form at a time).
        state.open_add_marker(Vec3::ZERO, 1.0);
        assert!(matches!(state.add_draft, Some(AddDraft::Marker(_))));
        state.open_add_frame(Vec3::ZERO, 1.0);
        assert!(matches!(state.add_draft, Some(AddDraft::Frame(_))));
        assert!(
            !state.add_focus_pending,
            "the frame open clears the request"
        );

        state.close_add();
        assert!(state.add_draft.is_none());
        assert!(
            !state.add_focus_pending,
            "closing drops any pending focus request"
        );
    }

    #[test]
    fn commit_add_frame_returns_defaults_and_closes() {
        let mut state = ObjectsPanelState::default();
        assert_eq!(state.commit_add_frame(), None, "no frame form open");

        state.open_add_frame(Vec3::new(2.0, 0.0, -1.0), 8.0);
        let add = state
            .commit_add_frame()
            .expect("the open frame form commits");
        assert_eq!(add.0, Vec3::new(2.0, 0.0, -1.0));
        assert_eq!(add.1, 2.0, "a quarter of the scale, the dialog-era seed");
        assert!(
            state.add_draft.is_none(),
            "a frame add is one-shot: the form closes"
        );
    }

    #[test]
    fn commit_add_marker_requires_text_and_keeps_the_form_open() {
        let mut state = ObjectsPanelState::default();
        assert!(state.commit_add_marker().is_none(), "no marker form open");

        state.open_add_marker(Vec3::new(0.0, 1.0, 0.0), 2.0);
        assert!(
            state.commit_add_marker().is_none(),
            "an empty label would be invisible: refused"
        );
        assert!(
            matches!(state.add_draft, Some(AddDraft::Marker(_))),
            "the refused commit keeps the form open for typing"
        );

        let text = match &mut state.add_draft {
            Some(AddDraft::Marker(marker)) => &mut marker.text,
            _ => unreachable!(),
        };
        text.push_str("  note  ");
        let marker = state.commit_add_marker().expect("a label commit");
        let Marker::Text(label) = &marker else {
            panic!("a label commit must produce a text marker");
        };
        assert_eq!(label.anchor, Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(label.text, "note", "the committed text is trimmed");
        assert!(
            matches!(state.add_draft, Some(AddDraft::Marker(_))),
            "the marker form stays open for repeat adds"
        );
        let cleared = match &state.add_draft {
            Some(AddDraft::Marker(marker)) => &marker.text,
            _ => unreachable!(),
        };
        assert!(cleared.is_empty(), "a committed label clears its field");
    }

    #[test]
    fn commit_add_marker_arrow_keeps_its_default_endpoints() {
        let mut state = ObjectsPanelState::default();
        state.open_add_marker(Vec3::new(1.0, 2.0, 3.0), 10.0);
        match &mut state.add_draft {
            Some(AddDraft::Marker(marker)) => marker.shape = MarkerShape::Arrow,
            _ => unreachable!(),
        };
        let marker = state.commit_add_marker().expect("an arrow commit");
        let Marker::Arrow(arrow) = &marker else {
            panic!("an arrow commit must produce an arrow marker");
        };
        assert_eq!(arrow.start, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(
            arrow.end,
            Vec3::new(3.0, 2.0, 3.0),
            "the tip keeps its seed a fifth of the scale along +X"
        );
        assert!(
            matches!(state.add_draft, Some(AddDraft::Marker(_))),
            "the marker form stays open for repeat adds"
        );
    }

    #[test]
    fn committing_one_kind_never_touches_the_other() {
        let mut state = ObjectsPanelState::default();
        state.open_add_marker(Vec3::ZERO, 1.0);
        assert_eq!(
            state.commit_add_frame(),
            None,
            "a marker form cannot commit a frame"
        );
        assert!(
            matches!(state.add_draft, Some(AddDraft::Marker(_))),
            "the refused commit leaves the marker form open"
        );

        state.open_add_frame(Vec3::ZERO, 1.0);
        assert!(
            state.commit_add_marker().is_none(),
            "a frame form cannot commit a marker"
        );
        assert!(matches!(state.add_draft, Some(AddDraft::Frame(_))));
    }
}
