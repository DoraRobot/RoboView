//! The properties panel — the fixed right sidebar of the four-zone
//! skeleton (ui-blueprint spec §6): the grouped property rows of the
//! object selected in the objects tree (004 spec §4 A3/A4 — the edit
//! wave, 004 task T16).
//!
//! The rows cover the editable parameter set the display-types spec
//! locked (002 spec §7 F3/F4), organized as:
//!
//! - the common rows (every kind): the display name (a live single-line
//!   editor) and the visibility (a live checkbox), read from the scene
//!   entry ([`roboview_core::scene::SceneObject`]);
//! - the geometry card (heading [`TextKey::PropGroupTransform`]): the
//!   spatial rows — a frame's origin, a text marker's anchor, an arrow's
//!   endpoints — each an XYZ triple of live [`egui::DragValue`]s;
//! - the kind-specific card: a frame's axis length (heading
//!   [`TextKey::PropGroupFrame`]), a text marker's label text, a mesh's
//!   face color (heading = the kind label, [`TextKey::KindMesh`] /
//!   [`TextKey::KindMarker`]). Point clouds and paths carry no
//!   kind-specific parameter (002 locks none beyond name/visibility), so
//!   they show the common rows only.
//!
//! Every row label and card heading resolves through [`texts`] (no widget
//! inlines copy); the axis letters stay the invariant
//! [`texts::AXIS_X`] family.
//!
//! # Editing (T16)
//!
//! The panel is live: every row *is* its editor, and the changed rows of
//! one frame leave in [`PropertiesOutput`] as one [`PropertiesEdit`] per
//! touched object — [`PropertiesEdit::fields`] in the field vocabulary of
//! the viewport commit service ([`super::viewport::ObjectEdit`]), plus
//! [`PropertiesEdit::color`], the color-row request that routes through
//! the appearance channel
//! ([`super::viewport::ViewportState::appearance_override`]). The caller
//! (main.rs) applies the output under its scene lock after this call —
//! the panel itself never mutates the scene: values are snapshotted at
//! the top of the frame ([`selected_props`]), only *changes* report, and
//! the effect lands within one frame (004 spec §4 A4, plan §3.5).
//!
//! Row semantics mirror the tree's inline rename (objects_panel.rs, T12):
//! a text row commits its trimmed, non-blank text on Enter or a
//! click-away, and Escape cancels; drag values commit while dragged.
//! A draft whose editor the frame no longer draws — the selection moved
//! while the user was typing — settles at the top of the frame
//! ([`flush_abandoned_drafts`]) with the same commit-or-cancel decision.
//!
//! The panel's session state (the text drafts, the mesh color mirror)
//! rides in egui's session memory, so the panel owns no state of its own
//! and the caller holds its scene lock only for this call, exactly as
//! for [`super::objects_panel::ui`].

use eframe::egui;
use glam::Vec3;

use roboview_core::displays::{DisplayKind, DisplayObject, Marker};
use roboview_core::io::Color;
use roboview_core::scene::Scene;

#[cfg(test)]
use roboview_core::scene::camera::OrbitCamera;

use super::texts::{self, Locale, TextKey};
use super::viewport::ObjectEdit;

/// Width of the row-label column of every row (labels truncate within it,
/// so the narrowest panel width — 200 px at the 480×360 minimum window,
/// 004 spec §4 A13 — cannot clip them).
const LABEL_COLUMN: f32 = 64.0;

/// Decimal formatting of the numeric rows: the readout keeps at least two
/// decimals so a zero still reads as `0.00` while idle, and shows at most
/// four — the fixed format rounds the display only, never the value.
const MIN_DECIMALS: usize = 2;
const MAX_DECIMALS: usize = 4;

/// The drag step of the numeric rows matches the scene's scale like the
/// 002 dialogs' `drag_speed` (objects_panel.rs): one drag point moves a
/// value by `scale × 0.002`.
const DRAG_SPEED_RATIO: f32 = 0.002;

/// Drag-step floor of the numeric rows: micro scenes (a degenerate box, a
/// lone frame or marker near the origin) keep a usable drag anyway.
const MIN_DRAG_SPEED: f32 = 1e-3;

/// Drag step while the scene measures nothing at all — only frames and
/// markers, which never join the bounds union (the 002 dialogs' pre-open
/// default).
const FALLBACK_DRAG_SPEED: f32 = 0.05;

/// The face color the mesh color row shows while no override exists: the
/// renderer's default face albedo — linear `[0.7, 0.75, 0.8, 1.0]`, core
/// render/mesh.rs `DEFAULT_MESH_FACE_COLOR` — converted to sRGB bytes by
/// the standard linear→sRGB curve per channel:
/// `(218, 225, 231)` ≈ the `(0.854, 0.881, 0.906)` the core module
/// documents.
///
/// A mesh's *effective* face color lives in the viewport's appearance
/// registry ([`super::viewport::ViewportState::appearance_of`]), which the panel
/// cannot reach through its `&Scene` snapshot — so this constant is the
/// fallback, and the color row additionally honors the panel's own last
/// commit for the object ([`PanelMemory::mesh_color`]). Overrides set
/// outside the panel (the 002 group default colors at creation) are the
/// one gap; the T16-3 wiring should feed the effective color into the
/// panel.
const MESH_DEFAULT_FACE_COLOR_SRGB: Color = Color {
    r: 218,
    g: 225,
    b: 231,
};

/// Output of the properties panel: the confirmed edits of this frame —
/// one [`PropertiesEdit`] per touched object, in report order — for the
/// caller (main.rs) to apply under its scene lock after the panel drew
/// (mirroring [`super::objects_panel::ObjectsPanelOutput`]). An empty
/// output means the user changed nothing this frame.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertiesOutput {
    /// The frame's per-object edit requests.
    pub edits: Vec<PropertiesEdit>,
}

/// The confirmed changes of one object in one frame — the unit the caller
/// commits: [`PropertiesEdit::fields`] go through
/// [`super::viewport::ViewportState::apply_object_edits`] as one batch, and
/// [`PropertiesEdit::color`] (a mesh color-row change; the only editable
/// row that has no CPU field) through
/// [`super::viewport::ViewportState::appearance_override`]. Both commit paths
/// are id-addressed and no-op safely — a kind-mismatched field or a
/// vanished id changes nothing — so the caller can apply the whole output
/// without re-checking the scene.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertiesEdit {
    /// The scene entry the rows edited.
    pub id: u64,
    /// The changed rows, packed into the field vocabulary of the viewport
    /// commit service ([`super::viewport::ObjectEdit`]), in report order.
    pub fields: Vec<ObjectEdit>,
    /// A color-row change of the same object (appearance channel).
    pub color: Option<Color>,
}

/// Draw the properties panel into `ui` (the right `SidePanel` of the
/// four-zone layout). `selected` is the tree's selection
/// ([`super::objects_panel::ObjectsPanelState::selected`]): `None` — or
/// an id the scene no longer holds, which the tree prunes but the panel
/// must survive regardless — shows the empty hint. The scene is only
/// snapshotted (see the module docs: the caller holds its lock across
/// this call, exactly as for [`super::objects_panel::ui`]); the returned
/// output carries everything the caller must commit.
pub fn ui(
    ui: &mut egui::Ui,
    selected: Option<u64>,
    scene: &Scene<DisplayObject>,
    locale: Locale,
) -> PropertiesOutput {
    // The panel's session memory (text drafts, color mirror) rides in
    // egui's temporary data: those values live until egui shuts down and
    // are never serialized (egui util/id_type_map.rs) — the panel needs no
    // caller-owned state to hold an editor buffer between frames.
    let memory_key = egui::Id::new("properties_panel.memory");
    let mut memory = ui
        .data(|data| data.get_temp::<PanelMemory>(memory_key))
        .unwrap_or_default();

    // Drafts whose editor the body will not draw cannot report their own
    // settle (Enter and click-aways happen on a drawn widget): flush them
    // before the body draws, with the same decision the editor would make.
    let mut edits: Vec<PropertiesEdit> = Vec::new();
    for (id, field) in flush_abandoned_drafts(&mut memory, selected, scene) {
        fold_field(&mut edits, id, field);
    }

    let props = selected_props(selected, scene);
    let speed = row_speed(scene);
    ui.add_space(4.0);
    egui::ScrollArea::vertical()
        .id_salt("properties_panel")
        .auto_shrink([false, false])
        .max_height(ui.available_height())
        .show(ui, |ui| match &props {
            None => empty_state_ui(ui, locale),
            Some(props) => panel_body_ui(ui, props, locale, speed, &mut memory, &mut edits),
        });

    // Write the memory back unconditionally: a frame that settled a slot
    // (an Enter, an Escape, a flush) must not leave the stale copy
    // stored, or the flush would settle the same draft again next frame.
    ui.data_mut(|data| data.insert_temp(memory_key, memory));

    PropertiesOutput { edits }
}

/// The empty hint of the panel while no object is selected (or the
/// selected id vanished): one weak line under the panel top, in the same
/// visual register as the tree hints (objects_panel.rs).
fn empty_state_ui(ui: &mut egui::Ui, locale: Locale) {
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(texts::prop_empty_hint(locale)).color(ui.visuals().weak_text_color()),
    );
}

/// The panel body of one selected object: the common name/visibility rows
/// first, then the kind's cards (geometry card, kind-specific card) in
/// snapshot order. Every row is live; its confirmed changes fold into
/// `edits` as they happen.
fn panel_body_ui(
    ui: &mut egui::Ui,
    props: &SelectedProps,
    locale: Locale,
    speed: f32,
    memory: &mut PanelMemory,
    edits: &mut Vec<PropertiesEdit>,
) {
    common_rows_ui(ui, props, locale, memory, edits);
    for card in &props.cards {
        ui.add_space(10.0);
        card_ui(ui, props.id, card, locale, speed, memory, edits);
    }
}

/// The common rows of every selection: the display name — a draft-backed
/// single-line editor, the inline-rename widget of the tree, live — and
/// the visibility checkbox.
fn common_rows_ui(
    ui: &mut egui::Ui,
    props: &SelectedProps,
    locale: Locale,
    memory: &mut PanelMemory,
    edits: &mut Vec<PropertiesEdit>,
) {
    ui.horizontal(|ui| {
        row_label(ui, texts::prop_label_name(locale));
        if let Some(name) = text_row_ui(ui, &mut memory.name_draft, props.id, &props.name, None) {
            fold_field(edits, props.id, ObjectEdit::Rename(name));
        }
    });
    ui.horizontal(|ui| {
        row_label(ui, texts::prop_label_visible(locale));
        let mut visible = props.visible;
        if ui.add(egui::Checkbox::without_text(&mut visible)).changed() {
            fold_field(edits, props.id, ObjectEdit::Visible(visible));
        }
    });
}

/// One property card: its heading and the rows beneath (all editing the
/// scene entry `id`).
fn card_ui(
    ui: &mut egui::Ui,
    id: u64,
    card: &Card,
    locale: Locale,
    speed: f32,
    memory: &mut PanelMemory,
    edits: &mut Vec<PropertiesEdit>,
) {
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::symmetric(6, 4))
        .show(ui, |ui| {
            let heading = match card.heading {
                CardHeading::Group(key) => texts::resolve(locale, key),
                CardHeading::Kind(kind) => texts::object_kind_label(locale, kind),
            };
            ui.label(egui::RichText::new(heading).strong());
            ui.add_space(3.0);
            for row in &card.rows {
                row_ui(ui, id, row, locale, speed, memory, edits);
            }
        });
}

/// One property row, dispatched by its payload shape (see [`Value`]); the
/// row edits the scene entry `id` and folds its changes into `edits`.
fn row_ui(
    ui: &mut egui::Ui,
    id: u64,
    row: &Row,
    locale: Locale,
    speed: f32,
    memory: &mut PanelMemory,
    edits: &mut Vec<PropertiesEdit>,
) {
    let label = texts::resolve(locale, row.label);
    match &row.value {
        // A world-space point: the axis-prefixed XYZ triple of the 002
        // dialogs, live. Each of the four kinds binds the triple to its
        // own field edit through the constructor pointer.
        Value::Origin(point) => {
            point_row_ui(ui, label, *point, speed, edits, id, ObjectEdit::Origin)
        }
        Value::Anchor(point) => {
            point_row_ui(ui, label, *point, speed, edits, id, ObjectEdit::Anchor)
        }
        Value::Start(point) => point_row_ui(ui, label, *point, speed, edits, id, ObjectEdit::Start),
        Value::End(point) => point_row_ui(ui, label, *point, speed, edits, id, ObjectEdit::End),
        Value::Length(length) => length_row_ui(ui, label, *length, speed, edits, id),
        Value::Color(_) => color_row_ui(ui, label, id, memory, edits),
        Value::Text(text) => {
            ui.horizontal(|ui| {
                row_label(ui, label);
                if let Some(committed) = text_row_ui(
                    ui,
                    &mut memory.label_draft,
                    id,
                    text,
                    Some(texts::marker_text_hint(locale)),
                ) {
                    fold_field(edits, id, ObjectEdit::Text(committed));
                }
            });
        }
    }
}

/// One live XYZ row: three axis-prefixed drag values after the row label
/// (the 002 dialogs' layout); the flow wraps when the panel is too narrow
/// for the row (the 200 px minimum of A13), each field keeping its axis
/// prefix. A drag commits the row — the moved axes and the untouched
/// snapshot axes are packed into the point edit of the row's kind
/// (`into_edit`, chosen by the caller) — while a click that moved nothing
/// changes nothing.
fn point_row_ui(
    ui: &mut egui::Ui,
    label: &str,
    point: Vec3,
    speed: f32,
    edits: &mut Vec<PropertiesEdit>,
    id: u64,
    into_edit: fn(Vec3) -> ObjectEdit,
) {
    let (mut x, mut y, mut z) = (point.x, point.y, point.z);
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        row_label(ui, label);
        changed |= ui
            .add(drag_value(&mut x, speed).prefix(format!("{} ", texts::AXIS_X)))
            .changed();
        changed |= ui
            .add(drag_value(&mut y, speed).prefix(format!("{} ", texts::AXIS_Y)))
            .changed();
        changed |= ui
            .add(drag_value(&mut z, speed).prefix(format!("{} ", texts::AXIS_Z)))
            .changed();
    });
    if changed {
        fold_field(edits, id, into_edit(Vec3::new(x, y, z)));
    }
}

/// The frame axis-length row: one scalar drag value clamped to the
/// non-negative range the length invariant allows (002 spec F3).
fn length_row_ui(
    ui: &mut egui::Ui,
    label: &str,
    length: f32,
    speed: f32,
    edits: &mut Vec<PropertiesEdit>,
    id: u64,
) {
    let mut length = length;
    let changed = ui
        .horizontal(|ui| {
            row_label(ui, label);
            ui.add(drag_value(&mut length, speed).range(0.0..=f32::MAX))
                .changed()
        })
        .inner;
    if changed {
        fold_field(edits, id, ObjectEdit::Length(length));
    }
}

/// The mesh color row: egui's color button (a swatch that opens the
/// color picker popup), showing the object's current face color — the
/// panel's own last pick ([`PanelMemory::mesh_color`]), falling back to
/// the renderer default ([`MESH_DEFAULT_FACE_COLOR_SRGB`]). Every change
/// of the picker commits immediately — through the appearance channel
/// ([`PropertiesEdit::color`]) and into the mirror — so dragging the
/// picker previews live on the mesh within the same frame (004 spec A4).
fn color_row_ui(
    ui: &mut egui::Ui,
    label: &str,
    id: u64,
    memory: &mut PanelMemory,
    edits: &mut Vec<PropertiesEdit>,
) {
    let color = effective_mesh_color(memory, id);
    let mut srgb = [color.r, color.g, color.b];
    let changed = ui
        .horizontal(|ui| {
            row_label(ui, label);
            egui::widgets::color_picker::color_edit_button_srgb(ui, &mut srgb).changed()
        })
        .inner;
    if changed {
        let color = Color {
            r: srgb[0],
            g: srgb[1],
            b: srgb[2],
        };
        memory.mesh_color = Some((id, color));
        fold_color(edits, id, color);
    }
}

/// The row-label cell of one row: a fixed-width, truncating label.
fn row_label(ui: &mut egui::Ui, text: &str) {
    ui.add_sized(
        egui::vec2(LABEL_COLUMN, ui.spacing().interact_size.y),
        egui::Label::new(text).truncate(),
    );
}

/// One live drag value of the numeric rows, tuned to the scene scale (see
/// [`row_speed`]) and formatted with the shared decimal rule.
fn drag_value<'a>(value: &'a mut f32, speed: f32) -> egui::DragValue<'a> {
    egui::DragValue::new(value)
        .speed(speed)
        .min_decimals(MIN_DECIMALS)
        .max_decimals(MAX_DECIMALS)
}

/// One frame of a draft-backed single-line text editor — the name row and
/// the marker-text row share the widget and the commit semantics of the
/// tree's inline rename (objects_panel.rs, T12): Enter (single-line
/// editors surrender focus on Enter) and click-aways commit through
/// [`take_text_commit`] — trimmed, non-blank text, only when it differs
/// from the current copy — while Escape cancels the draft. While the
/// editor idles, the draft mirrors the scene copy, so external changes
/// (an applied edit of this panel, the tree's inline rename, a reload)
/// are picked up on the next frame. An empty buffer shows the field's
/// hint copy. Returns the committed text.
fn text_row_ui(
    ui: &mut egui::Ui,
    slot: &mut Option<(u64, String)>,
    id: u64,
    current: &str,
    hint: Option<&str>,
) -> Option<String> {
    if slot.as_ref().is_none_or(|(draft_id, _)| *draft_id != id) {
        *slot = Some((id, current.to_owned()));
    }
    let mut committed = None;
    let response = {
        let (_, text) = slot.as_mut().expect("the draft was seeded just above");
        let editor = egui::TextEdit::singleline(text).desired_width(f32::INFINITY);
        let editor = match hint {
            Some(hint) => editor.hint_text(hint),
            None => editor,
        };
        ui.add(editor)
    };
    if response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        // Escape cancels the pending text; the idle frame reseeds from
        // the scene copy.
        *slot = None;
        response.surrender_focus();
    } else if response.lost_focus() {
        // Enter and click-aways both land here.
        committed = take_text_commit(slot, current).map(|(_, text)| text);
    } else if !response.has_focus() {
        // Idle: mirror the scene copy (an applied edit, a re-selection).
        *slot = Some((id, current.to_owned()));
    }
    committed
}

// Pure layer: everything the panel draws is copied out of the scene here,
// so the drawing functions never hold or read the scene; the edits of a
// frame are folded into plain data too ([`fold_field`], [`fold_color`]).

/// The full content of one panel frame: the two common rows (name,
/// visibility — fields of the scene entry) and the kind's cards.
#[derive(Debug, Clone, PartialEq)]
struct SelectedProps {
    /// The scene entry id the rows edit (the selection itself).
    id: u64,
    /// The scene entry's display name (the name row).
    name: String,
    /// The scene entry's visibility (the visibility row's checkbox).
    visible: bool,
    /// The kind's property cards below the common rows, in draw order.
    cards: Vec<Card>,
}

/// One property card: its heading and rows, in draw order.
#[derive(Debug, Clone, PartialEq)]
struct Card {
    heading: CardHeading,
    rows: Vec<Row>,
}

/// What heads a card: a grouping key of the properties panel
/// ([`TextKey::PropGroupTransform`], [`TextKey::PropGroupFrame`]) or a
/// display kind whose label heads the kind-specific card (mesh color,
/// text-marker text — the [`TextKey::KindMesh`]/[`TextKey::KindMarker`]
/// family, resolved through `object_kind_label`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CardHeading {
    /// One of the panel's own grouping keys.
    Group(TextKey),
    /// A display kind; its label heads the card.
    Kind(DisplayKind),
}

/// One property row: its copy label (a [`TextKey`], so the snapshot is
/// locale-independent and the copy resolves at draw time) and the current
/// payload the row edits.
#[derive(Debug, Clone, PartialEq)]
struct Row {
    label: TextKey,
    value: Value,
}

/// The payload shape of one property row. The variant *is* the scene
/// field the row edits — the commit vocabulary mirrors it 1:1
/// ([`super::viewport::ObjectEdit`]): a world point is an XYZ triple (three
/// axis-prefixed drag values), the frame axis length a scalar, the marker
/// label text a string, the mesh face color an sRGB byte color.
#[derive(Debug, Clone, PartialEq)]
enum Value {
    /// A frame's shared origin corner (edits as [`ObjectEdit::Origin`]).
    Origin(Vec3),
    /// A text marker's anchor point (edits as [`ObjectEdit::Anchor`]).
    Anchor(Vec3),
    /// An arrow marker's tail point (edits as [`ObjectEdit::Start`]).
    Start(Vec3),
    /// An arrow marker's tip point (edits as [`ObjectEdit::End`]).
    End(Vec3),
    /// A frame's axis length in world units (edits as
    /// [`ObjectEdit::Length`]; non-negative, 002 spec F3).
    Length(f32),
    /// A text marker's label text (edits as [`ObjectEdit::Text`]).
    Text(String),
    /// A mesh's face color (commits through the appearance channel).
    Color(Color),
}

/// Snapshot the panel content of `selected`, if the scene still holds it.
/// `None` — no selection or a stale id — is the empty state of the panel.
fn selected_props(selected: Option<u64>, scene: &Scene<DisplayObject>) -> Option<SelectedProps> {
    let entry = scene.get(selected?)?;
    Some(SelectedProps {
        id: entry.id,
        name: entry.name.clone(),
        visible: entry.visible,
        cards: cards_of(&entry.object),
    })
}

/// The kind's property cards of one display object, in draw order
/// (geometry card first when it exists, then the kind-specific card).
fn cards_of(object: &DisplayObject) -> Vec<Card> {
    match object {
        // The file-backed data kinds lock no parameter beyond
        // name/visibility (display-types spec §7 F0–F2): no cards.
        DisplayObject::PointCloud(_) | DisplayObject::Path(_) => Vec::new(),
        DisplayObject::Mesh(_) => vec![Card {
            heading: CardHeading::Kind(DisplayKind::Mesh),
            rows: vec![Row {
                label: TextKey::PropLabelColor,
                value: Value::Color(MESH_DEFAULT_FACE_COLOR_SRGB),
            }],
        }],
        DisplayObject::Frame(frame) => vec![
            Card {
                heading: CardHeading::Group(TextKey::PropGroupTransform),
                rows: vec![Row {
                    label: TextKey::AddFrameOrigin,
                    value: Value::Origin(frame.origin),
                }],
            },
            Card {
                heading: CardHeading::Group(TextKey::PropGroupFrame),
                rows: vec![Row {
                    label: TextKey::PropLabelLength,
                    value: Value::Length(frame.length),
                }],
            },
        ],
        DisplayObject::Marker(Marker::Text(text)) => vec![
            Card {
                heading: CardHeading::Group(TextKey::PropGroupTransform),
                rows: vec![Row {
                    label: TextKey::MarkerAnchor,
                    value: Value::Anchor(text.anchor),
                }],
            },
            Card {
                heading: CardHeading::Kind(DisplayKind::Marker),
                rows: vec![Row {
                    label: TextKey::MarkerText,
                    value: Value::Text(text.text.clone()),
                }],
            },
        ],
        DisplayObject::Marker(Marker::Arrow(arrow)) => vec![Card {
            heading: CardHeading::Group(TextKey::PropGroupTransform),
            rows: vec![
                Row {
                    label: TextKey::MarkerStart,
                    value: Value::Start(arrow.start),
                },
                Row {
                    label: TextKey::MarkerEnd,
                    value: Value::End(arrow.end),
                },
            ],
        }],
    }
}

/// The panel's session memory, stored in egui's temporary data (values
/// live until egui shuts down and are never serialized): the text drafts
/// of the two text rows and the mesh-color mirror. Slots are keyed by the
/// object id they belong to; the frame body seeds and settles the slots
/// of the drawn object, and the frame-top flush settles the rest.
#[derive(Debug, Clone, Default, PartialEq)]
struct PanelMemory {
    /// The name row's draft: the buffer of the object's name editor.
    name_draft: Option<(u64, String)>,
    /// The marker-text row's draft: the buffer of the label editor.
    label_draft: Option<(u64, String)>,
    /// The last face color this panel committed for the object of `id`
    /// (one slot — a single object is selected at a time). The mirror
    /// keeps the color row showing the panel's own picks across
    /// re-selections; the row falls back to
    /// [`MESH_DEFAULT_FACE_COLOR_SRGB`] for objects the panel never
    /// re-colored.
    mesh_color: Option<(u64, Color)>,
}

/// The color the mesh color row shows: the panel's own last pick for this
/// object (its mirror of the appearance channel), or the renderer's
/// default face color when the panel never re-colored it (see
/// [`MESH_DEFAULT_FACE_COLOR_SRGB`] for the provenance).
fn effective_mesh_color(memory: &PanelMemory, id: u64) -> Color {
    match memory.mesh_color {
        Some((mirrored_id, color)) if mirrored_id == id => color,
        _ => MESH_DEFAULT_FACE_COLOR_SRGB,
    }
}

/// The drag step of the numeric rows, matched to the scene's scale like
/// the 002 add dialogs (objects_panel.rs `drag_speed`): `largest
/// dimension × [`DRAG_SPEED_RATIO`]`, floored at [`MIN_DRAG_SPEED`] —
/// and the dialogs' pre-open default while the scene measures nothing
/// (bounds union of the visible measurable objects, `scene.rs`; frames
/// and markers never join it).
fn row_speed(scene: &Scene<DisplayObject>) -> f32 {
    match scene.bounds_union() {
        Some(bounds) => (bounds.largest_dimension() * DRAG_SPEED_RATIO).max(MIN_DRAG_SPEED),
        None => FALLBACK_DRAG_SPEED,
    }
}

/// Fold one confirmed field change into the frame's output: the changes
/// of one object accumulate into a single entry — the caller commits one
/// batch per id through
/// [`super::viewport::ViewportState::apply_object_edits`] — in report order.
fn fold_field(edits: &mut Vec<PropertiesEdit>, id: u64, field: ObjectEdit) {
    match edits.iter_mut().find(|entry| entry.id == id) {
        Some(entry) => entry.fields.push(field),
        None => edits.push(PropertiesEdit {
            id,
            fields: vec![field],
            color: None,
        }),
    }
}

/// The same fold for a color-row change, which commits through the
/// appearance channel ([`super::viewport::ViewportState::appearance_override`])
/// instead of the field list.
fn fold_color(edits: &mut Vec<PropertiesEdit>, id: u64, color: Color) {
    match edits.iter_mut().find(|entry| entry.id == id) {
        Some(entry) => entry.color = Some(color),
        None => edits.push(PropertiesEdit {
            id,
            fields: Vec::new(),
            color: Some(color),
        }),
    }
}

/// The commit decision of a text draft whose editor is being dismissed —
/// Enter, a click-away, or the frame-top flush: the text commits trimmed
/// when it is non-blank and differs from the current copy. A blank result
/// or an unchanged text cancels (the scene never stores blank names, and
/// an unchanged name must not emit an edit). The slot is consumed in
/// every case, so a settled draft cannot commit twice.
fn take_text_commit(slot: &mut Option<(u64, String)>, current: &str) -> Option<(u64, String)> {
    let (id, text) = slot.take()?;
    let trimmed = text.trim();
    (!trimmed.is_empty() && trimmed != current.trim()).then(|| (id, trimmed.to_owned()))
}

/// The frame-top pass over the drafts whose editor the panel will not
/// draw this frame: a draft whose object is no longer the selection (the
/// user clicked another tree row while typing) or left the scene can no
/// longer be settled by its editor — Enter and click-aways only happen on
/// a drawn widget — so it settles here with the same decision the editor
/// would make: the text commits (trimmed, when it differs from the scene
/// copy and is not blank), otherwise it cancels; the slot is consumed
/// either way. Drafts of the object the body will draw are left for the
/// drawn editor to settle. Returns the committed field edits.
fn flush_abandoned_drafts(
    memory: &mut PanelMemory,
    selected: Option<u64>,
    scene: &Scene<DisplayObject>,
) -> Vec<(u64, ObjectEdit)> {
    let mut committed = Vec::new();

    // The name draft: settles when the name row will not be drawn — the
    // selection moved, or the object left the scene.
    if let Some(id) = memory.name_draft.as_ref().map(|(id, _)| *id) {
        let drawn = Some(id) == selected && scene.get(id).is_some();
        if !drawn {
            let commit = match scene.get(id) {
                Some(entry) => take_text_commit(&mut memory.name_draft, &entry.name),
                None => {
                    // The object left the scene: nothing to apply to.
                    memory.name_draft = None;
                    None
                }
            };
            if let Some((_, text)) = commit {
                committed.push((id, ObjectEdit::Rename(text)));
            }
        }
    }

    // The label draft: the same rule — the label row draws only while a
    // text marker is selected, so a draft of a vanished object or of an
    // object whose kind has no label row settles (cancels) too.
    if let Some(id) = memory.label_draft.as_ref().map(|(id, _)| *id) {
        let drawn = Some(id) == selected
            && matches!(
                scene.get(id).map(|entry| &entry.object),
                Some(DisplayObject::Marker(Marker::Text(_)))
            );
        if !drawn {
            let commit = match scene.get(id).map(|entry| &entry.object) {
                Some(DisplayObject::Marker(Marker::Text(text))) => {
                    take_text_commit(&mut memory.label_draft, &text.text)
                }
                _ => {
                    memory.label_draft = None;
                    None
                }
            };
            if let Some((_, text)) = commit {
                committed.push((id, ObjectEdit::Text(text)));
            }
        }
    }

    committed
}

#[cfg(test)]
mod tests {
    use super::*;
    use roboview_core::displays::{Frame, Mesh, Path, PointCloud};
    use roboview_core::io::{self, Format};

    /// An empty scene viewed from the default pose.
    fn empty_scene() -> Scene<DisplayObject> {
        Scene::new(OrbitCamera::new(Vec3::ZERO))
    }

    /// A scene holding one named object; returns its id.
    fn scene_with(object: DisplayObject, name: &str) -> (Scene<DisplayObject>, u64) {
        let mut scene = empty_scene();
        let id = scene.add(object, name);
        (scene, id)
    }

    fn cloud() -> DisplayObject {
        DisplayObject::PointCloud(PointCloud::from_data(io::PointCloudData {
            positions: vec![Vec3::ZERO],
            colors: None,
            bounds: None,
            format: Format::Ply,
        }))
    }

    fn mesh() -> DisplayObject {
        DisplayObject::Mesh(Mesh::from_data(io::MeshData {
            positions: vec![Vec3::ZERO],
            normals: None,
            indices: None,
            bounds: None,
        }))
    }

    fn mesh_with_bounds() -> DisplayObject {
        DisplayObject::Mesh(Mesh::from_data(io::MeshData {
            positions: vec![Vec3::ZERO],
            normals: None,
            indices: None,
            bounds: Some(io::Aabb {
                min: Vec3::ZERO,
                max: Vec3::new(100.0, 1.0, 1.0),
            }),
        }))
    }

    fn path() -> DisplayObject {
        DisplayObject::Path(Path::from_data(io::PathData {
            points: vec![Vec3::ZERO, Vec3::X],
            bounds: None,
        }))
    }

    fn assert_point(row: &Row, label: TextKey, value: Value) {
        assert_eq!(row.label, label, "row label of {label:?}");
        assert_eq!(row.value, value, "row value of {label:?}");
    }

    #[test]
    fn no_selection_or_stale_id_snapshots_to_none() {
        let (scene, _) = scene_with(DisplayObject::Frame(Frame::new(Vec3::ZERO, 1.0)), "frame");
        assert!(selected_props(None, &scene).is_none(), "no selection");
        assert!(
            selected_props(Some(u64::MAX), &scene).is_none(),
            "an id the scene no longer holds is the empty state too"
        );
        assert!(
            selected_props(Some(u64::MAX), &empty_scene()).is_none(),
            "empty scene with a stale selection"
        );
    }

    #[test]
    fn frame_rows_follow_display_types_f3() {
        let frame = DisplayObject::Frame(Frame::new(Vec3::new(1.0, -2.0, 3.5), 0.5));
        let (scene, id) = scene_with(frame, "world frame");
        let props = selected_props(Some(id), &scene).expect("the frame is selected");
        // The snapshot carries the scene entry id the rows will edit.
        assert_eq!(props.id, id);
        // The common rows copy the scene entry, not the display payload.
        assert_eq!(props.name, "world frame");
        assert!(props.visible);

        // Geometry card: the origin XYZ triple (002 spec §7 F3), bound to
        // the field it edits.
        assert_eq!(props.cards.len(), 2);
        assert_eq!(
            props.cards[0].heading,
            CardHeading::Group(TextKey::PropGroupTransform)
        );
        assert_eq!(props.cards[0].rows.len(), 1);
        assert_point(
            &props.cards[0].rows[0],
            TextKey::AddFrameOrigin,
            Value::Origin(Vec3::new(1.0, -2.0, 3.5)),
        );

        // Kind-specific card: the axis length scalar.
        assert_eq!(
            props.cards[1].heading,
            CardHeading::Group(TextKey::PropGroupFrame)
        );
        assert_eq!(
            props.cards[1].rows,
            vec![Row {
                label: TextKey::PropLabelLength,
                value: Value::Length(0.5),
            }]
        );
    }

    #[test]
    fn text_marker_rows_follow_display_types_f4() {
        let marker = DisplayObject::Marker(Marker::text(Vec3::new(0.0, 1.0, 2.0), "note"));
        let (scene, id) = scene_with(marker, "note marker");
        let props = selected_props(Some(id), &scene).expect("the marker is selected");
        assert_eq!(props.name, "note marker");

        // Geometry card: the anchor triple; kind card: the label text.
        assert_eq!(
            props.cards[0].heading,
            CardHeading::Group(TextKey::PropGroupTransform)
        );
        assert_point(
            &props.cards[0].rows[0],
            TextKey::MarkerAnchor,
            Value::Anchor(Vec3::new(0.0, 1.0, 2.0)),
        );
        assert_eq!(
            props.cards[1].heading,
            CardHeading::Kind(DisplayKind::Marker)
        );
        assert_eq!(
            props.cards[1].rows,
            vec![Row {
                label: TextKey::MarkerText,
                value: Value::Text("note".to_owned()),
            }]
        );
    }

    #[test]
    fn empty_marker_text_stays_visible_as_a_row() {
        // The add dialog never creates an empty label text, but a scene
        // can hold one (and 002 leaves the text editable): the row exists
        // with the empty payload — the editor shows its hint copy.
        let marker = DisplayObject::Marker(Marker::text(Vec3::ZERO, ""));
        let (scene, id) = scene_with(marker, "ghost label");
        let props = selected_props(Some(id), &scene).expect("the marker is selected");
        assert_eq!(props.cards.len(), 2);
        assert_eq!(
            props.cards[1].rows[0].value,
            Value::Text(String::new()),
            "an empty label text must survive the snapshot"
        );
    }

    #[test]
    fn arrow_marker_rows_cover_both_endpoints() {
        let arrow = DisplayObject::Marker(Marker::arrow(Vec3::ZERO, Vec3::X * 4.0));
        let (scene, id) = scene_with(arrow, "arrow A");
        let props = selected_props(Some(id), &scene).expect("the arrow is selected");

        // One geometry card holding both endpoint triples (002 spec §7 F4).
        assert_eq!(props.cards.len(), 1);
        assert_eq!(
            props.cards[0].heading,
            CardHeading::Group(TextKey::PropGroupTransform)
        );
        assert_point(
            &props.cards[0].rows[0],
            TextKey::MarkerStart,
            Value::Start(Vec3::ZERO),
        );
        assert_point(
            &props.cards[0].rows[1],
            TextKey::MarkerEnd,
            Value::End(Vec3::new(4.0, 0.0, 0.0)),
        );
    }

    #[test]
    fn mesh_color_row_holds_the_renderer_default_face_color() {
        let (scene, id) = scene_with(mesh(), "mesh.obj"); // A9: test-fixture file name
        let props = selected_props(Some(id), &scene).expect("the mesh is selected");

        // The kind card of the mesh heads with its kind label and holds
        // the color row (004 spec §4 A3: mesh → color).
        assert_eq!(props.cards.len(), 1);
        assert_eq!(props.cards[0].heading, CardHeading::Kind(DisplayKind::Mesh));
        assert_eq!(
            props.cards[0].rows,
            vec![Row {
                label: TextKey::PropLabelColor,
                value: Value::Color(MESH_DEFAULT_FACE_COLOR_SRGB),
            }]
        );
        // The token is the renderer's default face albedo (linear
        // [0.7, 0.75, 0.8, 1.0], core render/mesh.rs) in sRGB bytes —
        // pinned here so the displayed swatch cannot drift from the
        // painted geometry.
        assert_eq!(MESH_DEFAULT_FACE_COLOR_SRGB.r, 218);
        assert_eq!(MESH_DEFAULT_FACE_COLOR_SRGB.g, 225);
        assert_eq!(MESH_DEFAULT_FACE_COLOR_SRGB.b, 231);
    }

    #[test]
    fn data_kinds_show_only_the_common_rows() {
        for (object, name) in [(cloud(), "scan"), (path(), "route")] {
            let (scene, id) = scene_with(object, name);
            let props = selected_props(Some(id), &scene).expect("the object is selected");
            assert_eq!(props.name, name);
            assert_eq!(
                props.cards,
                Vec::new(),
                "point clouds and paths lock no kind-specific parameter"
            );
        }
    }

    #[test]
    fn snapshot_copies_the_visibility_and_detaches_from_the_scene() {
        let (mut scene, id) = scene_with(mesh(), "hidden mesh");
        scene.toggle_visible(id);
        let props = selected_props(Some(id), &scene).expect("the mesh is selected");
        assert!(
            !props.visible,
            "the hidden state comes from the scene entry"
        );

        // The snapshot is a copy: mutating the scene afterwards does not
        // change a previously taken snapshot.
        scene.toggle_visible(id);
        let again = selected_props(Some(id), &scene).expect("still selected");
        assert!(again.visible);
        assert!(!props.visible, "snapshots must copy, not alias");
    }

    #[test]
    fn text_commit_trims_and_requires_a_different_non_blank_text() {
        // A blank draft cancels.
        let mut slot = Some((3, "   ".to_owned()));
        assert_eq!(
            take_text_commit(&mut slot, "world frame"),
            None,
            "blank text cancels"
        );
        assert!(slot.is_none(), "the settled slot is consumed either way");

        // An unchanged text cancels too — no edit for a no-op.
        let mut slot = Some((3, "world frame".to_owned()));
        assert_eq!(take_text_commit(&mut slot, "world frame"), None);
        assert!(slot.is_none());

        // A change commits trimmed.
        let mut slot = Some((3, "  renamed  ".to_owned()));
        assert_eq!(
            take_text_commit(&mut slot, "world frame"),
            Some((3, "renamed".to_owned())),
            "committed text is trimmed"
        );
        assert!(slot.is_none());

        // An empty slot has nothing to commit.
        let mut slot = None;
        assert_eq!(take_text_commit(&mut slot, "world frame"), None);
    }

    #[test]
    fn row_edits_fold_into_one_entry_per_object() {
        let mut edits: Vec<PropertiesEdit> = Vec::new();
        fold_field(&mut edits, 7, ObjectEdit::Visible(false));
        fold_field(&mut edits, 7, ObjectEdit::Origin(Vec3::X));
        fold_field(&mut edits, 9, ObjectEdit::Length(2.0));
        fold_color(&mut edits, 7, Color { r: 1, g: 2, b: 3 });
        fold_color(&mut edits, 9, Color { r: 4, g: 5, b: 6 });
        assert_eq!(
            edits,
            vec![
                PropertiesEdit {
                    id: 7,
                    fields: vec![ObjectEdit::Visible(false), ObjectEdit::Origin(Vec3::X),],
                    color: Some(Color { r: 1, g: 2, b: 3 }),
                },
                PropertiesEdit {
                    id: 9,
                    fields: vec![ObjectEdit::Length(2.0)],
                    color: Some(Color { r: 4, g: 5, b: 6 }),
                },
            ],
            "one entry per object; rows keep report order; the color slot rides its object's entry"
        );
    }

    #[test]
    fn a_color_only_edit_creates_its_own_entry() {
        let mut edits = Vec::new();
        fold_color(&mut edits, 5, Color { r: 9, g: 8, b: 7 });
        assert_eq!(
            edits,
            vec![PropertiesEdit {
                id: 5,
                fields: Vec::new(),
                color: Some(Color { r: 9, g: 8, b: 7 }),
            }]
        );
    }

    #[test]
    fn the_editor_keeps_its_draft_while_the_object_is_selected() {
        // A selected text marker draws both text rows: their editors
        // settle their own drafts, so the flush leaves them alone.
        let (scene, id) = scene_with(
            DisplayObject::Marker(Marker::text(Vec3::ZERO, "note")),
            "marker",
        );
        let mut memory = PanelMemory {
            name_draft: Some((id, "typed".to_owned())),
            label_draft: Some((id, "typed label".to_owned())),
            mesh_color: None,
        };
        assert!(
            flush_abandoned_drafts(&mut memory, Some(id), &scene).is_empty(),
            "the drawn editors settle their own drafts"
        );
        assert_eq!(
            memory.name_draft,
            Some((id, "typed".to_owned())),
            "the name row still draws this frame"
        );
        assert_eq!(
            memory.label_draft,
            Some((id, "typed label".to_owned())),
            "the label row still draws this frame"
        );
    }

    #[test]
    fn flush_commits_a_draft_the_selection_left_behind() {
        // A text marker with both editors open; the selection moves to an
        // object the scene does not hold before the user settled them.
        let (scene, id) = scene_with(
            DisplayObject::Marker(Marker::text(Vec3::ZERO, "note")),
            "note marker",
        );
        let mut memory = PanelMemory {
            name_draft: Some((id, "  renamed  ".to_owned())),
            label_draft: Some((id, "reworded".to_owned())),
            mesh_color: None,
        };
        assert_eq!(
            flush_abandoned_drafts(&mut memory, Some(u64::MAX), &scene),
            vec![
                (id, ObjectEdit::Rename("renamed".to_owned())),
                (id, ObjectEdit::Text("reworded".to_owned())),
            ],
            "the abandoned drafts commit trimmed, and only when they differ"
        );
        assert_eq!(memory.name_draft, None, "settled slots are consumed");
        assert_eq!(memory.label_draft, None);
    }

    #[test]
    fn flush_cancels_unchanged_and_blank_drafts() {
        let (scene, id) = scene_with(DisplayObject::Frame(Frame::new(Vec3::ZERO, 1.0)), "frame");
        // The draft mirrors the scene copy: leaving cancels (no edit).
        let mut memory = PanelMemory {
            name_draft: Some((id, "frame".to_owned())),
            ..PanelMemory::default()
        };
        assert!(flush_abandoned_drafts(&mut memory, None, &scene).is_empty());
        assert!(memory.name_draft.is_none());

        // A blank draft cancels the same way.
        memory.name_draft = Some((id, "  ".to_owned()));
        assert!(flush_abandoned_drafts(&mut memory, None, &scene).is_empty());
        assert!(memory.name_draft.is_none());
    }

    #[test]
    fn flush_cancels_drafts_of_vanished_objects() {
        let (mut scene, id) =
            scene_with(DisplayObject::Frame(Frame::new(Vec3::ZERO, 1.0)), "frame");
        scene.remove(id);
        let mut memory = PanelMemory {
            name_draft: Some((id, "typed".to_owned())),
            label_draft: Some((id, "typed".to_owned())),
            mesh_color: None,
        };
        assert!(
            flush_abandoned_drafts(&mut memory, Some(id), &scene).is_empty(),
            "a removed object has nothing left to apply a commit to"
        );
        assert!(memory.name_draft.is_none());
        assert!(memory.label_draft.is_none());
    }

    #[test]
    fn flush_cancels_a_label_draft_when_the_object_is_not_a_text_marker() {
        // The label row exists only for text markers; a draft of any other
        // object kind is a no-op for the row that will never draw.
        let (scene, id) = scene_with(mesh(), "solid");
        let mut memory = PanelMemory {
            label_draft: Some((id, "typed".to_owned())),
            ..PanelMemory::default()
        };
        assert!(flush_abandoned_drafts(&mut memory, None, &scene).is_empty());
        assert!(memory.label_draft.is_none());
    }

    #[test]
    fn the_mesh_color_mirror_falls_back_to_the_renderer_default() {
        // A mirror of the panel's pick for another object must not leak
        // into the row of the selected mesh.
        let memory = PanelMemory {
            mesh_color: Some((9, Color { r: 255, g: 0, b: 0 })),
            ..PanelMemory::default()
        };
        assert_eq!(
            effective_mesh_color(&memory, 4),
            MESH_DEFAULT_FACE_COLOR_SRGB,
            "no pick of this panel for the object: the renderer default"
        );
    }

    #[test]
    fn the_mesh_color_mirror_shows_the_panels_own_pick() {
        let mut memory = PanelMemory {
            mesh_color: Some((4, Color { r: 255, g: 0, b: 0 })),
            ..PanelMemory::default()
        };
        assert_eq!(
            effective_mesh_color(&memory, 4),
            Color { r: 255, g: 0, b: 0 },
            "the row shows the color this panel last committed"
        );
        memory.mesh_color = Some((4, Color { r: 0, g: 255, b: 0 }));
        assert_eq!(
            effective_mesh_color(&memory, 4),
            Color { r: 0, g: 255, b: 0 },
            "a later pick wins"
        );
    }

    #[test]
    fn row_speed_follows_the_measurable_scene_scale() {
        // A measurable mesh scales the drag speed up proportionally.
        let (scene, _) = scene_with(mesh_with_bounds(), "scan.obj"); // A9: fixture name
        assert!(
            (row_speed(&scene) - 0.2).abs() < f32::EPSILON,
            "100 m largest dimension → 100 × 0.002 = 0.2, got {}",
            row_speed(&scene)
        );

        // A degenerate box (largest dimension 0) hits the floor, not zero.
        let (scene, _) = scene_with(
            DisplayObject::Mesh(Mesh::from_data(io::MeshData {
                positions: vec![Vec3::ZERO],
                normals: None,
                indices: None,
                bounds: Some(io::Aabb {
                    min: Vec3::ZERO,
                    max: Vec3::ZERO,
                }),
            })),
            "point.obj", // A9: test-fixture file name
        );
        assert_eq!(row_speed(&scene), MIN_DRAG_SPEED, "degenerate box → floor");

        // Objects without bounds — and an empty scene — fall back to the
        // dialogs' default (frames and markers never join the union).
        let (scene, _) = scene_with(mesh(), "solid.obj");
        assert_eq!(row_speed(&scene), FALLBACK_DRAG_SPEED, "no bounds");
        let (scene, _) = scene_with(DisplayObject::Frame(Frame::new(Vec3::ZERO, 1.0)), "frame");
        assert_eq!(row_speed(&scene), FALLBACK_DRAG_SPEED, "frames never bound");
        assert_eq!(row_speed(&empty_scene()), FALLBACK_DRAG_SPEED);
    }
}
