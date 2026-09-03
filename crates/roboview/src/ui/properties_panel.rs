//! The properties panel — the fixed right sidebar of the four-zone
//! skeleton (ui-blueprint spec §6): the grouped property rows of the
//! object selected in the objects tree (004 spec §4 A3, display side;
//! plan §3.5 A8 — 004 task T14).
//!
//! The rows cover the editable parameter set the display-types spec
//! locked (002 spec §7 F3/F4), organized as:
//!
//! - the common rows (every kind): the display name and the visibility,
//!   read from the scene entry ([`roboview_core::scene::SceneObject`]);
//! - the geometry card (heading [`TextKey::PropGroupTransform`]): the
//!   spatial rows — a frame's origin, a text marker's anchor, an arrow's
//!   endpoints;
//! - the kind-specific card: a frame's axis length (heading
//!   [`TextKey::PropGroupFrame`]), a text marker's label text and a
//!   mesh's face color (heading = the kind label,
//!   [`TextKey::KindMesh`] / [`TextKey::KindMarker`]). Point clouds and
//!   paths carry no kind-specific parameter (002 locks none beyond
//!   name/visibility), so they show the common rows only.
//!
//! Every row label and card heading resolves through [`texts`] (no widget
//! inlines copy); the axis letters stay the invariant
//! [`texts::AXIS_X`] family.
//!
//! # Read-only stage (T14)
//!
//! This wave draws the values, it does not edit them (plan §3.5: T16
//! enables editing through the single-object commit service of
//! `viewport.rs`). Each value is snapshotted at the top of the frame
//! ([`selected_props`]) — the panel never touches the scene afterwards,
//! and the entry signature takes `&Scene` without locking, so the caller
//! (main.rs) holds its lock only for the snapshot — and the rows render
//! the snapshot with the *future editor of every row in its disabled
//! state*:
//!
//! - numeric rows reuse the same [`egui::DragValue`]s as the 002 add
//!   dialogs ([`egui::Ui::add_enabled`] with `false`): the value format —
//!   XYZ triples with axis-letter prefixes, single scalars — is identical
//!   to the dialogs by construction, and T16 re-enables exactly the row
//!   it binds to a commit;
//! - the name row is a disabled single-line [`egui::TextEdit`] of the
//!   same shape the tree's inline rename uses;
//! - the visibility row is the tree's eye glyph, tinted by the state
//!   (strong while visible, weak while hidden);
//! - the mesh color row paints a swatch of the object's current face
//!   color — which today is always the renderer's default albedo, see
//!   [`MESH_DEFAULT_FACE_COLOR_SRGB`] for the provenance.
//!
//! The one entry point [`ui`] returns a [`PropertiesOutput`]: empty in
//! this wave — no mutation exists yet — the type is the forward contract
//! of the edit wave (T16 fills it with the commit requests the caller
//! applies under its scene lock, mirroring [`objects_panel`]'s output).

use eframe::egui;
use glam::Vec3;

use roboview_core::displays::{DisplayKind, DisplayObject, Marker};
use roboview_core::io::Color;
use roboview_core::scene::Scene;

#[cfg(test)]
use roboview_core::scene::camera::OrbitCamera;

use super::texts::{self, Locale, TextKey};
use super::theme;

/// The eye glyph of the visibility row, mirroring the tree's per-row eye
/// (objects_panel.rs keeps its own copy next to the column it paints). A
/// glyph invariant like the `texts::OBJECTS_REMOVE` trash can: it is the
/// visibility icon of the panel rows, not translatable copy.
const EYE_GLYPH: &str = "👁";

/// Width of the row-label column of every row (labels truncate within it,
/// so the narrowest panel width — 200 px at the 480×360 minimum window,
/// 004 spec §4 A13 — cannot clip them).
const LABEL_COLUMN: f32 = 64.0;

/// The mesh face color the read-only color row displays — the *current*
/// face color of every mesh in the scene today.
///
/// A mesh's face color lives only in the per-object appearance uniform of
/// its GPU handle (004 spec §6 appearance channel), which the app cannot
/// read back; the CPU side — the app-level `id → Appearance` registry of
/// plan §3.5 — is T16 work. Until it lands, every uploaded mesh shows the
/// default albedo its upload provisions (core render/mesh.rs
/// `DEFAULT_MESH_FACE_COLOR`, linear `[0.7, 0.75, 0.8, 1.0]` — the moved
/// WGSL `FACE_COLOR` constant), converted to sRGB bytes by the standard
/// linear→sRGB curve (`1.055·c^(1/2.4) − 0.055` per channel):
/// `(218, 225, 231)` ≈ the `(0.854, 0.881, 0.906)` the core module
/// documents. T16's registry replaces this token as the row's source.
const MESH_DEFAULT_FACE_COLOR_SRGB: Color = Color {
    r: 218,
    g: 225,
    b: 231,
};

/// Output of the properties panel. Empty in the read-only wave (T14):
/// no scene mutation exists yet, so there is nothing to return — the type
/// is the forward contract of the edit wave (T16), which fills it with
/// the per-object commit requests the caller (main.rs) applies under its
/// scene lock, mirroring [`super::objects_panel::ObjectsPanelOutput`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropertiesOutput;

/// Draw the properties panel into `ui` (the right `SidePanel` of the
/// four-zone layout). `selected` is the tree's selection
/// ([`super::objects_panel::ObjectsPanelState::selected`]): `None` — or
/// an id the scene no longer holds, which the tree prunes but the panel
/// must survive regardless — shows the empty hint. The scene is only
/// snapshotted (see the module docs: the caller holds its lock across
/// this call, exactly as for [`super::objects_panel::ui`]).
pub fn ui(
    ui: &mut egui::Ui,
    selected: Option<u64>,
    scene: &Scene<DisplayObject>,
    locale: Locale,
) -> PropertiesOutput {
    let props = selected_props(selected, scene);
    ui.add_space(4.0);
    egui::ScrollArea::vertical()
        .id_salt("properties_panel")
        .auto_shrink([false, false])
        .max_height(ui.available_height())
        .show(ui, |ui| match props {
            None => empty_state_ui(ui, locale),
            Some(props) => panel_body_ui(ui, &props, locale),
        });
    PropertiesOutput
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
/// snapshot order.
fn panel_body_ui(ui: &mut egui::Ui, props: &SelectedProps, locale: Locale) {
    common_rows_ui(ui, props, locale);
    for card in &props.cards {
        ui.add_space(10.0);
        card_ui(ui, card, locale);
    }
}

/// The common rows of every selection: the object name (a disabled
/// single-line editor — the inline-rename widget of the tree, locked)
/// and the visibility state (the eye glyph, tinted by the state).
fn common_rows_ui(ui: &mut egui::Ui, props: &SelectedProps, locale: Locale) {
    ui.horizontal(|ui| {
        row_label(ui, texts::prop_label_name(locale));
        let mut name = props.name.clone();
        ui.add_enabled(
            false,
            egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY),
        );
    });
    ui.horizontal(|ui| {
        row_label(ui, texts::prop_label_visible(locale));
        let glyph_color = if props.visible {
            ui.visuals().text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        ui.label(egui::RichText::new(EYE_GLYPH).color(glyph_color));
    });
}

/// One property card: its heading and the rows beneath.
fn card_ui(ui: &mut egui::Ui, card: &Card, locale: Locale) {
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
                row_ui(ui, row, locale);
            }
        });
}

/// One property row, dispatched by its value shape (see [`Value`]).
fn row_ui(ui: &mut egui::Ui, row: &Row, locale: Locale) {
    let label = texts::resolve(locale, row.label);
    match &row.value {
        Value::Xyz { x, y, z } => {
            // Three axis-prefixed drag values after the row label; the
            // flow wraps when the panel is too narrow for the row (the
            // 200 px minimum of A13), each field keeping its axis prefix.
            ui.horizontal_wrapped(|ui| {
                row_label(ui, label);
                axis_field(ui, texts::AXIS_X, *x);
                axis_field(ui, texts::AXIS_Y, *y);
                axis_field(ui, texts::AXIS_Z, *z);
            });
        }
        Value::Scalar(value) => {
            ui.horizontal(|ui| {
                row_label(ui, label);
                scalar_field(ui, *value);
            });
        }
        Value::Text(text) => {
            ui.horizontal(|ui| {
                row_label(ui, label);
                if text.is_empty() {
                    // An empty label text is invisible by definition (and
                    // cannot be created through the 002 add dialog, which
                    // waits for text); show the field's own hint copy,
                    // dimmed, so the row still reads.
                    ui.label(
                        egui::RichText::new(texts::marker_text_hint(locale))
                            .color(ui.visuals().weak_text_color()),
                    );
                } else {
                    ui.add(egui::Label::new(text).truncate());
                }
            });
        }
        Value::Color(color) => {
            ui.horizontal(|ui| {
                row_label(ui, label);
                color_swatch(ui, *color);
            });
        }
    }
}

/// The row-label cell of one row: a fixed-width, truncating label.
fn row_label(ui: &mut egui::Ui, text: &str) {
    ui.add_sized(
        egui::vec2(LABEL_COLUMN, ui.spacing().interact_size.y),
        egui::Label::new(text).truncate(),
    );
}

/// One disabled drag value of a spatial row, prefixed by its axis letter
/// (the `X 0.5` layout of the 002 dialogs, minus the drag).
fn axis_field(ui: &mut egui::Ui, axis: &str, value: f32) {
    let mut value = value;
    ui.add_enabled(
        false,
        egui::DragValue::new(&mut value).prefix(format!("{axis} ")),
    );
}

/// One disabled scalar drag value (a frame's axis length).
fn scalar_field(ui: &mut egui::Ui, value: f32) {
    let mut value = value;
    ui.add_enabled(false, egui::DragValue::new(&mut value));
}

/// A color swatch: a small rounded square filled with the payload color,
/// outlined so the swatch reads on the dark panel. The interactive color
/// button of the edit wave (T16) replaces this paint.
fn color_swatch(ui: &mut egui::Ui, color: Color) {
    let size = ui.spacing().interact_size.y - 4.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, theme::to_color32(color));
    painter.rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
}

// Pure snapshot layer: everything the panel draws is copied out of the
// scene here, so the drawing functions never hold or read the scene.

/// The full read-only content of one panel frame: the two common rows
/// (name, visibility — fields of the scene entry) and the kind's cards.
#[derive(Debug, Clone, PartialEq)]
struct SelectedProps {
    /// The scene entry's display name (the name row).
    name: String,
    /// The scene entry's visibility (the visibility row's eye tint).
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

/// One property row: its copy label and the read-only value. The label is
/// stored as a [`TextKey`] so the snapshot is locale-independent and the
/// copy resolves at draw time.
#[derive(Debug, Clone, PartialEq)]
struct Row {
    label: TextKey,
    value: Value,
}

/// The value shape of one property row, mirroring the 002 parameter
/// vocabulary: a world point is an XYZ triple (three axis-prefixed drag
/// values), the frame axis length a scalar, the marker label text a
/// string, the mesh face color an sRGB byte color.
#[derive(Debug, Clone, PartialEq)]
enum Value {
    /// A world-space point: three axis-prefixed drag values.
    Xyz { x: f32, y: f32, z: f32 },
    /// A scalar in world units (a frame's axis length).
    Scalar(f32),
    /// A text payload (a text marker's label text).
    Text(String),
    /// An sRGB color payload (a mesh's face color).
    Color(Color),
}

/// Snapshot the panel content of `selected`, if the scene still holds it.
/// `None` — no selection or a stale id — is the empty state of the panel.
fn selected_props(selected: Option<u64>, scene: &Scene<DisplayObject>) -> Option<SelectedProps> {
    let entry = scene.get(selected?)?;
    Some(SelectedProps {
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
                    value: point(frame.origin),
                }],
            },
            Card {
                heading: CardHeading::Group(TextKey::PropGroupFrame),
                rows: vec![Row {
                    label: TextKey::PropLabelLength,
                    value: Value::Scalar(frame.length),
                }],
            },
        ],
        DisplayObject::Marker(Marker::Text(text)) => vec![
            Card {
                heading: CardHeading::Group(TextKey::PropGroupTransform),
                rows: vec![Row {
                    label: TextKey::MarkerAnchor,
                    value: point(text.anchor),
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
                    value: point(arrow.start),
                },
                Row {
                    label: TextKey::MarkerEnd,
                    value: point(arrow.end),
                },
            ],
        }],
    }
}

/// One world-space point as an XYZ row value.
fn point(p: Vec3) -> Value {
    Value::Xyz {
        x: p.x,
        y: p.y,
        z: p.z,
    }
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

    fn path() -> DisplayObject {
        DisplayObject::Path(Path::from_data(io::PathData {
            points: vec![Vec3::ZERO, Vec3::X],
            bounds: None,
        }))
    }

    fn assert_xyz(row: &Row, label: TextKey, x: f32, y: f32, z: f32) {
        assert_eq!(row.label, label, "row label of {label:?}");
        assert_eq!(row.value, Value::Xyz { x, y, z }, "row value of {label:?}");
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
        // The common rows copy the scene entry, not the display payload.
        assert_eq!(props.name, "world frame");
        assert!(props.visible);

        // Geometry card: the origin XYZ triple (002 §7 F3).
        assert_eq!(props.cards.len(), 2);
        assert_eq!(
            props.cards[0].heading,
            CardHeading::Group(TextKey::PropGroupTransform)
        );
        assert_eq!(props.cards[0].rows.len(), 1);
        assert_xyz(
            &props.cards[0].rows[0],
            TextKey::AddFrameOrigin,
            1.0,
            -2.0,
            3.5,
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
                value: Value::Scalar(0.5),
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
        assert_xyz(
            &props.cards[0].rows[0],
            TextKey::MarkerAnchor,
            0.0,
            1.0,
            2.0,
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
        // with the empty payload — the painter dims its hint copy.
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

        // One geometry card holding both endpoint triples (002 §7 F4).
        assert_eq!(props.cards.len(), 1);
        assert_eq!(
            props.cards[0].heading,
            CardHeading::Group(TextKey::PropGroupTransform)
        );
        assert_xyz(&props.cards[0].rows[0], TextKey::MarkerStart, 0.0, 0.0, 0.0);
        assert_xyz(&props.cards[0].rows[1], TextKey::MarkerEnd, 4.0, 0.0, 0.0);
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
        // painted geometry. T16's id → Appearance registry replaces it.
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
}
