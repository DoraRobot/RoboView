//! 3D viewport: the shared scene/render state and its per-frame wgpu
//! paint callback.
//!
//! The scene is a container of heterogeneous display objects in add order
//! (display-types spec §1, plan §3.1/§3.2): one entry per loaded file —
//! point cloud, mesh, or path — plus the UI-added frames and markers.
//! Objects are appended, never replaced; toggling visibility only skips
//! drawing (spec §6); removal drops the entry and frees its GPU handles
//! through wgpu's deferred destruction semantics. The camera follows the
//! spec §6 ruling — the first object added to an empty scene frames the
//! scene bounds, later adds never move the camera, and the objects panel's
//! Fit button reframes on demand (ui/objects_panel.rs).
//!
//! The GPU side is a pipeline family of three owned slots that move
//! together: [`render::Renderer`] (the point pipeline plus the scene-wide
//! view-projection uniform), [`render::MeshPipeline`], and
//! [`render::LinePipeline`]. [`ViewportState::sync_renderer`] creates the
//! trio from eframe's wgpu render state and rebuilds it when the target
//! format changes, re-uploading every object — hidden ones too, because
//! visibility never releases resources and every bind group references the
//! old renderer's layout.
//!
//! # Viewport helper layer (004 spec §6, A11)
//!
//! On top of the scene content the viewport draws a helper layer: the
//! Z=0 ground grid, the world-origin axis trio, and the 2D orientation
//! indicator in the top-right corner. The grid is generated around the
//! camera's visible ground patch and refreshed in place whenever the
//! camera moves; lines always sit on world multiples, so the pattern never
//! crawls (A11). Fading is generation-side — the window follows the zoom
//! and the uniform step keeps a minimum spacing — so no alpha blending is
//! involved (the line pipeline draws blending-free). The grid and axes sit
//! at the head of the line pass (plan §3.3, spec §6): they depth-test
//! against the reference surface the point and mesh passes wrote but never
//! write depth, so content in front hides them, flat meshes lying on Z=0
//! are pushed away by their own depth bias and stay underdrawn by the
//! grid, and scene line objects always overdraw the helpers. Their session
//! switches (`grid_on` / `axes_on`, both on by default) live in
//! [`ViewportState`] — the single source every door reads and flips (menu
//! items, toolbar buttons, HUD corner badges). An empty scene keeps the
//! helpers navigable: the paint-callback gate is "scene not empty or a
//! helper is on", with the placeholder text drawn above the helpers
//! (spec §6).
//!
//! Frame flow (egui is single-threaded and immediate mode, so `update` and
//! the callbacks of the same frame never race):
//!
//! 1. `update` (the app) draws the viewport panel with [`show_viewport`];
//!    the state records the viewport rect of *this* frame and pointer
//!    input mutates the camera, then — when the scene holds any object or
//!    a helper-layer switch is on — one [`egui_wgpu::Callback`] is
//!    registered inside the frame's shape list.
//! 2. egui-wgpu later calls `ViewportCallback::prepare` for every such
//!    shape: it locks the state, recomputes the view-projection from the
//!    stored rect of this same frame, writes the scene's single uniform
//!    (one queue write per frame reaches every pipeline), and refreshes
//!    the helper layer — provisioning the persistent meshes on first need
//!    and regenerating the ground grid whenever the view-projection
//!    changed.
//! 3. During the render pass `ViewportCallback::paint` locks the state
//!    again and records the draws of every visible object, grouped into
//!    three passes by pipeline — points first (the depth reference
//!    surface), then mesh faces (pushed away by their depth bias), then
//!    line work (strict Less, no depth writes), which the helper layer
//!    opens ahead of the scene's paths, frames, and arrows (see above).
//!    The grouping keeps the shared-depth policy of the family and the
//!    pipeline switches stay at three per frame regardless of the object
//!    count.
//! 4. The overlay pass paints the viewport labels through the egui
//!    painter on top of the 3D content: the text markers' labels and the
//!    frames' axis letters, projected per frame with
//!    [`render::anchor_to_screen`] (spec §7 F3/F4, A4), and the corner
//!    orientation indicator, projected from the view-projection columns
//!    ([`render::camera_math::orientation_gizmo_dirs`], spec §6).
//!
//! The state lives behind an `Arc<Mutex<…>>` because the callback objects
//! must be `Send + Sync` (egui-wgpu requirement) and because wgpu access
//! must stay exclusive; on the UI thread the lock is uncontended. The
//! meshes and the scene data owned by the state make the whole structure
//! `Send`, which also future-proofs offloading uploads to a worker thread.
//!
//! # Single-object commit service (004 plan §3.5, task T16)
//!
//! On top of the load-time uploads the state exposes the per-object commit
//! service the properties panel edits through (004 spec §4 A3/A4):
//! [`ViewportState::apply_object_edits`] commits field edits — the common
//! name/visibility rows plus the kind-owned fields of frames and markers —
//! re-provisioning the object's GPU handle through the shared upload
//! dispatch when a field its geometry draws changed. The color rows of
//! meshes and point clouds commit through the appearance channel instead:
//! [`ViewportState::appearance_override`] and
//! [`ViewportState::clear_appearance_override`] set and remove the object's
//! color override by writing its per-object appearance uniform (the T7
//! channel) in place — one 64-byte queue write per change, never a
//! re-upload or a pipeline rebuild (spec §6).
//!
//! [`ViewportState::set_selected`] mirrors the objects tree's selection onto
//! the same channel, toggling the selection flag of at most the two affected
//! objects per change; a per-frame poll of an unchanged selection costs an
//! `Option` compare and writes nothing (spec M9/A12). The `id → Appearance`
//! registry in the state is the app-level CPU bearer of this whole channel
//! (plan §3.5): an entry exists exactly while the object's override is
//! active, carries the current selection bit, and is replayed whenever a
//! fresh upload resets the uniform to its default — renderer rebuilds and
//! geometry re-uploads both replay right behind the upload they reset. None
//! of the service ever touches the A6 handle ledger beyond the existing
//! upload arms and display-type drop notes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use eframe::egui;
use egui_wgpu::wgpu;
use glam::{Mat4, Vec2, Vec3, Vec4, vec2};

use roboview_core::displays::{self, DisplayKind, DisplayObject, Marker};
use roboview_core::io;
use roboview_core::render;
use roboview_core::render::camera_math::screen_to_ray;
use roboview_core::render::pick;
use roboview_core::render::renderer::Appearance;
use roboview_core::scene::camera::OrbitCamera;
use roboview_core::scene::{Scene, SceneObject};

use super::camera_input;
use super::texts::{self, Locale};
use super::theme;

/// Scene-scale fallback of the UI-add dialogs when the scene has no
/// measurable bounds (empty scene, or only frames/markers): mirrors the
/// camera's own default distance for empty scenes (scene/camera.rs), so an
/// empty scene yields the same magnitudes as a scene of that size.
const DEFAULT_UI_SCALE: f32 = 10.0;

/// Color of the X axis letter painted at a frame's +X tip. Matches the
/// frame axis segment colors of the line pipeline (render/line.rs, spec
/// §7 F3: X red, Y green, Z blue).
const AXIS_X_COLOR: egui::Color32 = egui::Color32::from_rgb(255, 0, 0);
const AXIS_Y_COLOR: egui::Color32 = egui::Color32::from_rgb(0, 255, 0);
const AXIS_Z_COLOR: egui::Color32 = egui::Color32::from_rgb(0, 0, 255);

/// Step (m) of the ground grid — the app's fixed base, mirroring the grid
/// module's default (render/grid.rs), which climbs the whole uniform grid
/// through the 1-2-5 ladder as the camera pulls back (spec §6: one
/// coherent grid, one step at a time — no concentric rings).
const GRID_STEP: f32 = 1.0;
/// Ceiling (m) of the ground-grid generation window radius. The persistent
/// mesh is prebuilt to `segment_capacity_bound` of this radius and the
/// per-frame window is clamped to it, so any reachable pose fits the
/// prebuilt capacity (the far-plane reach of the camera's distance clamp
/// stays below this cap even at extreme zooms and aspect ratios).
const GRID_RADIUS_CAP: f32 = 4.0e7;
/// Multiplicative slack of the measured visible-ground extent, absorbing
/// the float rounding of the frustum crossings.
const GRID_RADIUS_MARGIN: f32 = 1.05;
/// Fixed world length (m) of the world-origin axis trio (spec §6:
/// "示意长度固定" — one grid cell). World-fixed geometry never needs
/// a camera-driven refresh, unlike the windowed grid.
const ORIGIN_AXIS_LENGTH: f32 = 3.0;
/// Radius (points) of the orientation indicator's backdrop disc.
const INDICATOR_RADIUS: f32 = 30.0;
/// Distance (points) of the indicator disc center from the top-right
/// viewport corner.
const INDICATOR_INSET: f32 = 48.0;
/// Length (points) of the indicator's axis arms, from the disc center.
const INDICATOR_ARM_LEN: f32 = 12.0;
/// Width (points) of the indicator's axis arms.
const INDICATOR_ARM_WIDTH: f32 = 3.0;
/// Distance (points) of an axis letter's center from the disc center.
const INDICATOR_LETTER_RADIUS: f32 = 21.0;

/// The color a newly created object starts with while its group has no
/// user-set default (004 spec M4/D4) — a neutral light gray, numerically
/// identical to the objects panel's own unset marker (`GROUP_COLOR_UNSET`
/// of ui/objects_panel.rs, the mirror source). The viewport never imports a
/// sibling panel module (objects_panel already imports this one), so the
/// two constants stay in lockstep by this comment.
const GROUP_COLOR_UNSET: io::Color = io::Color {
    r: 190,
    g: 190,
    b: 190,
};

/// Upload-default albedo of triangle-face meshes — the CPU mirror of
/// `DEFAULT_MESH_FACE_COLOR` (private in render/mesh.rs): the baked face
/// color the mesh shader always reads, even without any override flag. The
/// commit service needs the value to restore a cleared override and to
/// replay the session appearance after a re-upload without touching core,
/// so the mirror is pinned here (its sRGB twin lives in
/// ui/properties_panel.rs; render/mesh.rs's own tests pin the core const).
const MESH_FACE_DEFAULT_ALBEDO: [f32; 4] = [0.7, 0.75, 0.8, 1.0];

/// Acquire the viewport state lock, recovering from poisoning: a poisoned
/// mutex still holds the state (the panicking thread unwound before any
/// invariant broke), so `into_inner` is safe here.
pub fn lock_state(state: &Arc<Mutex<ViewportState>>) -> MutexGuard<'_, ViewportState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// One field edit of a single scene object (004 plan §3.5, spec §4 A3/A4):
/// the property panel packs the changed rows of the selected object into a
/// batch and commits it through [`ViewportState::apply_object_edits`]. The
/// variant set maps 1:1 onto the editable CPU fields of the closed display
/// set:
///
/// | Display kind           | Editable rows                           |
/// |------------------------|-----------------------------------------|
/// | any scene entry        | [`ObjectEdit::Rename`], [`ObjectEdit::Visible`] |
/// | `Frame`                | [`ObjectEdit::Origin`], [`ObjectEdit::Length`] |
/// | `Marker::Text`         | [`ObjectEdit::Anchor`], [`ObjectEdit::Text`] |
/// | `Marker::Arrow`        | [`ObjectEdit::Start`], [`ObjectEdit::End`] |
/// | `Mesh` / `PointCloud`  | color row only — through the appearance channel ([`ViewportState::appearance_override`]) |
/// | `Path`                 | name/visibility rows only               |
///
/// An edit whose variant does not match the object's kind is a no-op: the
/// tree's type column is the contract, and the panel never commits rows the
/// selected kind does not own. Geometry rows (`Origin`, `Length`, `Start`,
/// `End`) re-upload the object; every other row is CPU-only (text labels
/// are painted by the overlay, visibility only skips drawing).
#[derive(Debug, Clone, PartialEq)]
pub enum ObjectEdit {
    /// Replace the object's display name (the tree's rename row; the same
    /// trim-and-reject-blank rule the objects panel's inline rename keeps —
    /// the scene never stores blank names).
    Rename(String),
    /// Set the object's visibility flag.
    Visible(bool),
    /// Move a frame's shared origin corner (002 spec F3).
    Origin(Vec3),
    /// Set a frame's axis length (002 spec F3).
    Length(f32),
    /// Move a text marker's anchor (002 spec F4).
    Anchor(Vec3),
    /// Replace a text marker's label (002 spec F4).
    Text(String),
    /// Move an arrow marker's tail (002 spec F4).
    Start(Vec3),
    /// Move an arrow marker's tip (002 spec F4).
    End(Vec3),
}

/// Shared state of the 3D viewport: the scene (camera + every display
/// object in add order), the renderer with the family pipelines, and the
/// per-frame viewport rect.
pub struct ViewportState {
    /// Scene: camera plus one object per scene entry, in add order
    /// (display-types plan §3.1). The payload is the closed display-type
    /// set of `displays::DisplayObject` (plan §3.2). Crate-visible: the
    /// objects panel (ui/objects_panel.rs) lists, toggles, and removes
    /// entries by id through this field.
    pub(crate) scene: Scene<DisplayObject>,
    /// Renderer for the current target format; `None` before the first
    /// frame in which eframe exposes its wgpu `RenderState`
    /// (egui_wgpu::RenderState), i.e. before any GPU work is possible.
    renderer: Option<render::Renderer>,
    /// Triangle pipeline of the scene family, built from the same renderer
    /// (render/mesh.rs). Exists exactly when [`ViewportState::renderer`]
    /// does: `sync_renderer` creates and rebuilds the three together.
    mesh_pipeline: Option<render::MeshPipeline>,
    /// Line pipeline of the scene family (paths, frames, marker arrows);
    /// same lifetime invariant as [`ViewportState::mesh_pipeline`].
    line_pipeline: Option<render::LinePipeline>,
    /// Appearance registry of the commit service (004 plan §3.5, task
    /// T16): the app-level CPU bearer of the per-object appearance channel.
    /// An entry exists exactly while the object's color override is active;
    /// its flags always carry the current selection bit — kept in sync by
    /// [`ViewportState::set_selected`] — so every replay writes the
    /// override and the highlight as one composite. Rows of removed objects
    /// are pruned at every mutation entry point (removals can also come
    /// from outside this file — the scene API is public — so the pruning is
    /// self-healing, never a hard invariant).
    appearances: HashMap<u64, Appearance>,
    /// A click-pick reported by the viewport this frame, pending the
    /// app's mirror onto the tree/properties selection source (005 A4,
    /// reverse direction of the tree→viewport mirror). `None` is a
    /// reported "click on empty space" (clears the tree selection), the
    /// `Option<Option<..>>` layer marks "no report since last take".
    click_selection: Option<Vec<u64>>,
    /// Box-select extras beyond the primary (005 A10): multi-select keeps
    /// the primary in `selected_mirror` and the rest here, so the single
    /// selection paths (properties mirror, session replay) keep working.
    /// Empty whenever the selection is a single object.
    selected_multi: Vec<u64>,
    /// Start pixel of an in-flight primary-button box drag (005 A9), or
    /// `None` when no box gesture is active. The rubber band spans from
    /// here to the current pointer; the camera never moves during it.
    box_drag_start: Option<Vec2>,
    /// Last selection the viewport mirrored, if any. The per-frame
    /// [`ViewportState::set_selected`] poll compares against it first: an
    /// unchanged selection costs one `Option` compare and writes nothing
    /// (004 spec M9/A12), while a change lands within the same frame.
    selected_mirror: Option<u64>,
    /// Group default colors of the objects tree, mirrored into the viewport
    /// (004 spec M4/D4: a group's default colors *new* members of that
    /// kind). The tree's chips are the authoring side; main.rs syncs them
    /// here through [`ViewportState::set_group_default_color`], and object
    /// creation reads the result through
    /// [`ViewportState::appearance_default_for_new`].
    group_default_colors: HashMap<DisplayKind, io::Color>,
    /// Session switches of the helper layer (spec §6): the ground grid and
    /// the world-origin axes, both on by default. Session state, not egui
    /// memory — every door (menu items, toolbar buttons, HUD corner
    /// badges) reads and flips these through the accessors below.
    grid_on: bool,
    axes_on: bool,
    /// Persistent ground-grid mesh of the helper layer, prebuilt once to
    /// the segment capacity of the capped generation window
    /// ([`GRID_RADIUS_CAP`]) and refreshed in place through
    /// [`render::LinePipeline::update_mesh`] whenever the camera moves.
    /// Created on the render thread (prepare); creation and refresh never
    /// route through the upload ledger (A6-safe).
    grid_mesh: Option<render::LineMesh>,
    /// The world-origin axis trio: three one-segment meshes, X red / Y
    /// green / Z blue (theme ORIGIN_AXIS — the 002 semantic colors), at
    /// the fixed [`ORIGIN_AXIS_LENGTH`]. World-fixed, so once provisioned
    /// they never refresh. Both helper mesh groups are dropped on a
    /// renderer rebuild — their bind groups reference the old renderer's
    /// layout — and re-provisioned by the next prepare.
    axes_meshes: Option<[render::LineMesh; 3]>,
    /// The axis reference lines through the origin (004 revision 2026-09-05:
    /// Blender-style axis lines) — one long segment per axis, clipped to
    /// the visible ground window like the grid, refreshed with it. Painted
    /// with the same 002 semantic colors, under the same `axes_on` switch.
    axis_lines: Option<[render::LineMesh; 3]>,
    /// View-projection of the last grid refresh — the refresh key. A
    /// bitwise-equal `view_proj` means the identical window and strips, so
    /// nothing regenerates while the camera sits still (A11: no crawl, no
    /// per-frame rework).
    grid_refresh_key: Option<Mat4>,
    /// Pointer position inside the viewport of the frame currently being
    /// built (status-bar world coordinate). `show_viewport` records it
    /// once per frame; empty when the pointer is outside.
    pointer_hover: Option<Vec2>,
    /// Viewport rect of the frame currently being built, in points.
    /// `show_viewport` records it while drawing; the paint callback of the
    /// same frame reads it as the aspect-ratio source. Rect proportions
    /// are scale-invariant, so using the point-space rect equals using the
    /// physical-pixel rect of the callback info.
    viewport_rect: egui::Rect,
    /// Per-kind add counters of the UI-added objects, fed to
    /// `texts::default_frame_name`/`default_marker_name`: every add takes
    /// the next serial, so generated names never repeat within a session.
    frame_serial: u64,
    marker_serial: u64,
}

impl ViewportState {
    /// Create an empty viewport: no objects, no renderer, a default camera
    /// that the first successful load replaces with a framing pose.
    pub fn new() -> Self {
        Self {
            scene: Scene::new(OrbitCamera::framing(None)),
            renderer: None,
            mesh_pipeline: None,
            line_pipeline: None,
            appearances: HashMap::new(),
            selected_mirror: None,
            click_selection: None,
            selected_multi: Vec::new(),
            box_drag_start: None,
            group_default_colors: HashMap::new(),
            // A12 perf-protocol hook (004 T18): ROBOVIEW_DEMO_GRID_OFF=1
            // starts with the ground grid hidden for the on/off comparison.
            grid_on: std::env::var("ROBOVIEW_DEMO_GRID_OFF").is_err(),
            axes_on: true,
            grid_mesh: None,
            axes_meshes: None,
            axis_lines: None,
            grid_refresh_key: None,
            pointer_hover: None,
            viewport_rect: egui::Rect::NOTHING,
            frame_serial: 0,
            marker_serial: 0,
        }
    }

    /// Align the renderer and the family pipelines with eframe's current
    /// wgpu render state.
    ///
    /// The first call (once `frame.wgpu_render_state()` is available)
    /// creates the renderer and both family pipelines; any later call whose
    /// target format differs — the window moved across screens with
    /// different surface capabilities — rebuilds all three and re-uploads
    /// every object, because mesh bind groups and pipelines reference the
    /// old renderer's layout and uniform buffer. Call once per frame; when
    /// nothing changed this is a cheap format comparison. The depth format
    /// and sample count come from `NativeOptions` (Depth24Plus, 1) and are
    /// constant for the app's lifetime, so the target format is the only
    /// rebuild trigger.
    ///
    /// A rebuild resets every appearance uniform to its upload default (the
    /// fresh handles replace the old ones), so the commit service replays
    /// the session appearance right behind each upload — the registry
    /// composite, or the selection flag of a selected object without an
    /// override (plan §3.5: rebuilds and re-uploads must not lose the mesh
    /// colors or the highlight).
    pub fn sync_renderer(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_format: wgpu::TextureFormat,
    ) {
        let matches = self
            .renderer
            .as_ref()
            .is_some_and(|renderer| renderer.target_format() == target_format);
        if matches {
            return;
        }
        tracing::info!(
            ?target_format,
            "building scene renderer and family pipelines"
        );
        let mut renderer = render::Renderer::new(
            Arc::new(device.clone()),
            Arc::new(queue.clone()),
            target_format,
            // egui-wgpu attaches Depth24Plus when depth_buffer=24 is set in
            // NativeOptions; pipelines and pass must agree exactly.
            wgpu::TextureFormat::Depth24Plus,
            1,
        );
        let mesh_pipeline = render::MeshPipeline::new(&renderer);
        let line_pipeline = render::LinePipeline::new(&renderer);
        // Re-upload every object so each GPU handle comes from the rebuilt
        // pipelines (and from the same device if eframe ever switches
        // adapters). Hidden objects upload too: visibility only skips
        // drawing, never releases resources. Text markers hold no GPU data
        // and are skipped by the upload dispatch. Each fresh handle
        // provisions the upload default appearance, so the session
        // appearance replays right behind it — the registry is read while
        // the scene is borrowed mutably, but the two are disjoint fields.
        // Multi-select covers more than the legacy single mirror: the
        // session replay consults the whole set, so precompute it before
        // the mutable scene borrow.
        let selected_set = self
            .selected_ids()
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        for object in self.scene.iter_mut() {
            upload_display(
                &mut renderer,
                &mesh_pipeline,
                &line_pipeline,
                &mut object.object,
            );
            if let Some(composite) = session_appearance(
                &self.appearances,
                // Only the flag of this object matters for its
                // composite; pass it as the single selected id so the
                // legacy helper stays unambiguous (005 A10 multi-set).
                if selected_set.contains(&object.id) {
                    Some(object.id)
                } else {
                    None
                },
                object,
            ) {
                write_appearance_uniform(
                    &renderer,
                    &mesh_pipeline,
                    &line_pipeline,
                    &object.object,
                    &composite,
                );
            }
        }
        // The helper meshes' bind groups reference the old renderer's
        // layout and uniform buffer (`with_capacity` builds them from the
        // pipeline), so a rebuild drops them; the next prepare
        // re-provisions and refreshes them lazily.
        self.grid_mesh = None;
        self.axes_meshes = None;
        self.axis_lines = None;
        self.grid_refresh_key = None;
        self.renderer = Some(renderer);
        self.mesh_pipeline = Some(mesh_pipeline);
        self.line_pipeline = Some(line_pipeline);
    }

    /// Upload `loaded` and append it to the scene as a new object named
    /// `name` (the file stem), per the spec §1 append declaration.
    ///
    /// The camera moves only when the scene was empty (display-types spec
    /// §6): the first object frames the union of the scene bounds; later
    /// adds keep the current view.
    ///
    /// The upload happens here, on the UI thread, through the pipeline that
    /// matches the object's kind — the same dispatch the renderer rebuild
    /// runs. The renderer must exist before this is called: main.rs guards
    /// with [`ViewportState::renderer_ready`] and only installs after
    /// `sync_renderer` ran for the frame, so the `false` return is a
    /// defensive invariant violation — the caller skips it by never calling
    /// early.
    pub fn install_object(&mut self, loaded: io::LoadedObject, name: &str) -> bool {
        let Some(renderer) = self.renderer.as_mut() else {
            return false;
        };
        // Invariant of sync_renderer: the renderer and both family
        // pipelines are created and rebuilt together, so a present renderer
        // implies present pipelines.
        let mesh_pipeline = self
            .mesh_pipeline
            .as_mut()
            .expect("mesh pipeline must exist whenever the renderer does");
        let line_pipeline = self
            .line_pipeline
            .as_mut()
            .expect("line pipeline must exist whenever the renderer does");

        let mut display = DisplayObject::from_loaded(loaded);
        upload_display(renderer, mesh_pipeline, line_pipeline, &mut display);
        let scene_was_empty = self.scene.is_empty();
        self.scene.add(display, name);
        if scene_was_empty {
            self.scene.camera = OrbitCamera::framing(self.scene.bounds_union().as_ref());
        }
        true
    }

    /// Add a UI-built coordinate frame (spec §7 F3): upload its three axis
    /// segments through the line pipeline and append the object under a
    /// generated name (`Frame N`). UI adds never move the camera, whatever
    /// the scene holds. The scene id of the new object is returned — the
    /// group-default injection of task T17 commits its color by that id
    /// right after the add.
    pub fn add_frame(&mut self, origin: Vec3, length: f32) -> u64 {
        let mut frame = displays::Frame::new(origin, length);
        if let Some(line_pipeline) = self.line_pipeline.as_ref() {
            frame.gpu = Some(line_pipeline.upload_frame(origin, length));
        } else {
            // Only reachable before the first renderer exists, when no UI
            // can be shown; the renderer rebuild re-uploads every object,
            // so the frame still gains its handle then.
            tracing::warn!("frame added before the line pipeline existed; upload deferred");
        }
        self.frame_serial += 1;
        let name = texts::default_frame_name(self.frame_serial);
        self.scene.add(DisplayObject::Frame(frame), name)
    }

    /// Add a UI-built marker (spec §7 F4): arrows are uploaded through the
    /// line pipeline, text labels hold no GPU data (they are painted by the
    /// overlay pass). Appended under a generated name (`Marker N`); UI
    /// adds never move the camera. The scene id of the new object is
    /// returned, same contract as [`ViewportState::add_frame`].
    pub fn add_marker(&mut self, mut marker: displays::Marker) -> u64 {
        if let displays::Marker::Arrow(arrow) = &mut marker {
            if let Some(line_pipeline) = self.line_pipeline.as_ref() {
                arrow.gpu = Some(line_pipeline.upload_arrow(arrow.start, arrow.end));
            } else {
                // Same reasoning as in add_frame: unreachable once the UI
                // exists, healed by the next renderer rebuild.
                tracing::warn!("arrow added before the line pipeline existed; upload deferred");
            }
        }
        self.marker_serial += 1;
        let name = texts::default_marker_name(self.marker_serial);
        self.scene.add(DisplayObject::Marker(marker), name)
    }

    // — Single-object commit service (004 spec §4 A3/A4, plan §3.5; T16) —

    /// Set the color override of one object — the mesh/point-cloud color
    /// row of the properties panel and the group-default injection of new
    /// members both route here (004 spec §4 A3, plan §3.5).
    ///
    /// The override is recorded in the appearance registry (the CPU bearer
    /// of the mesh color — core's data model stays untouched) and pushed
    /// into the object's GPU uniform in place: one 64-byte queue write
    /// through the existing handle, so no geometry is re-uploaded, no
    /// pipeline rebuilt, and no A6 ledger row moves (the uniform rides
    /// inside the handle the upload already counted). An object that is
    /// currently selected keeps its selection flag on top of the override —
    /// the registry composite is the union of both channels, and replays
    /// after a re-upload or renderer rebuild write exactly that union.
    /// Unknown ids are a no-op (the scene never reuses an id, so an object
    /// that left it can never need an override again).
    pub fn appearance_override(&mut self, id: u64, color: io::Color) {
        self.prune_appearance_registry();
        if self.scene.get(id).is_none() {
            return;
        }
        let mut appearance = Appearance::srgb_override(color);
        if self.selected_mirror == Some(id) {
            appearance = appearance.with_selected(true);
        }
        if self.appearances.get(&id) == Some(&appearance) {
            // The identical composite is already registered and was pushed
            // when it changed — a repeat submission writes nothing.
            return;
        }
        self.appearances.insert(id, appearance);
        self.push_appearance_uniform(id, &appearance);
    }

    /// Remove the color override of one object: the registry row is dropped
    /// and the uniform is restored to the object's upload default (the
    /// renderer's baked look — per-point colors, or the mesh face default
    /// albedo) plus the selection flag while the object is selected. Same
    /// cost and A6 story as [`ViewportState::appearance_override`]. A
    /// repeat clear without an active override is a no-op.
    #[allow(dead_code)] // reserved: color-row clear/readback (007 / property polish)
    pub fn clear_appearance_override(&mut self, id: u64) {
        self.prune_appearance_registry();
        if self.appearances.remove(&id).is_none() {
            return;
        }
        let Some(object) = self.scene.get(id) else {
            return;
        };
        let selected = self.selected_mirror == Some(id);
        let mut appearance = upload_default_appearance(&object.object);
        if selected {
            appearance = appearance.with_selected(true);
        }
        self.push_appearance_uniform(id, &appearance);
    }

    /// The registered appearance composite of one object while its color
    /// override is active — the properties panel's read for the current
    /// mesh/point-cloud color row (plan §3.5: this replaces the read-only
    /// token the panel displayed before the registry existed). The albedo
    /// is linear light, same as the uniform.
    #[allow(dead_code)] // reserved: properties color-row readback (T16-1 note)
    pub fn appearance_of(&self, id: u64) -> Option<Appearance> {
        self.appearances.get(&id).copied()
    }

    /// Mirror one object as the selected one — the highlight half of the
    /// commit service (004 spec §6: there is no picking in the viewport,
    /// the objects tree drives the selection). Sets the selection flag of
    /// the object's appearance uniform in place, or clears it from the
    /// previously selected object — at most two 64-byte queue writes per
    /// change, never a re-upload, never an A6 ledger row.
    ///
    /// Call once per frame with the tree's selection: the mirror compare
    /// makes an unchanged selection a zero-op (spec M9/A12 — a per-frame
    /// poll costs one `Option` compare and writes nothing), and a *changed*
    /// selection lands within the same frame, because the writes happen in
    /// `update`, ahead of the viewport's paint callback. `Some(id)` for an
    /// object that left the scene normalizes to a deselection — the scene
    /// never reuses ids — and override rows of removed objects are pruned
    /// here, so a tree-driven delete needs no registry bookkeeping at its
    /// call site.
    /// The world-point pick under `cursor` (005 A1/A2): ray from the
    /// pixel through the scene's visible objects in add order, δ/point
    /// radius semantics from `render/pick.rs`. `None` when nothing is hit
    /// (also: a degenerate rect or camera — never panics, spec A5).
    pub fn pick_at(&self, cursor: Vec2) -> Option<pick::PickHit> {
        let ctx = self.pick_context()?;
        let (origin, dir) = screen_to_ray(&ctx.view_proj, ctx.viewport, cursor)?;
        let objects: Vec<(u64, &roboview_core::displays::DisplayObject)> = self
            .scene
            .iter_visible()
            .map(|o| (o.id, &o.object))
            .collect();
        pick::pick_objects(&ctx, origin, dir, &objects)
    }

    /// Box-select the visible objects whose projected bounds touch `a..b`
    /// (005 A9, contact semantics) and return their ids in add order.
    fn box_pick(&self, a: Vec2, b: Vec2) -> Vec<u64> {
        let Some(ctx) = self.pick_context() else {
            return Vec::new();
        };
        let rect = roboview_core::render::camera_math::Rect2 {
            min: a.min(b),
            max: a.max(b),
        };
        let objects: Vec<(u64, &roboview_core::displays::DisplayObject)> = self
            .scene
            .iter_visible()
            .map(|o| (o.id, &o.object))
            .collect();
        pick::pick_rect(&ctx, rect, &objects)
    }

    /// Report a viewport resolved selection (click-pick or box-select;
    /// empty means clear) for the app to mirror onto the tree source.
    pub fn report_click_selection(&mut self, ids: Vec<u64>) {
        self.click_selection = Some(ids);
    }

    /// Take the most recent reported selection, if any since the last take.
    pub fn take_click_selection(&mut self) -> Option<Vec<u64>> {
        self.click_selection.take()
    }

    /// The screen rect of the in-flight box drag (normalized, clamped to
    /// the viewport), if any — the rubber band the overlay paints.
    pub fn box_drag_rect(&self, pointer: Vec2) -> Option<(Vec2, Vec2)> {
        let start = self.box_drag_start?;
        let a = start.min(pointer);
        let b = start.max(pointer);
        let size = Vec2::new(self.viewport_rect.width(), self.viewport_rect.height());
        let a = Vec2::new(a.x.clamp(0.0, size.x), a.y.clamp(0.0, size.y));
        let b = Vec2::new(b.x.clamp(0.0, size.x), b.y.clamp(0.0, size.y));
        Some((a, b))
    }

    /// The pick context of the current pose (pure viewport geometry — the
    /// shared construction of the click and box pickers).
    fn pick_context(&self) -> Option<pick::PickContext> {
        let size = Vec2::new(self.viewport_rect.width(), self.viewport_rect.height());
        if size.x <= 0.0 || size.y <= 0.0 {
            return None;
        }
        let view_proj = self.scene.camera.view_proj(self.aspect());
        Some(pick::PickContext {
            view_proj,
            viewport: size,
            // World length per pixel at one unit of ray depth
            // (005 plan §3.1): 2·tan(fov/2)/viewport_height.
            world_per_pixel_scale: 2.0 * (self.scene.camera.vertical_fov() / 2.0).tan() / size.y,
            // The marker-label font the overlay paints with (paint_label,
            // 005 pick.rs docs — hit boxes must match the painted pill).
            font_size_px: 14.0,
        })
    }

    /// Set the selection to exactly `ids` (005 A9/A10): the box-select and
    /// click-pick route. Each transition writes only the uniforms of the
    /// objects whose flag changed (symmetric difference), so a repeated
    /// frame with an unchanged set costs an `Option`-compare and writes
    /// nothing (004 M9/A12).
    pub fn set_selection(&mut self, ids: &[u64]) {
        match ids {
            [] => self.set_selected(None),
            [single] => self.set_selected(Some(*single)),
            _ => {
                // Multi-select: the primary stays in the legacy mirror so
                // the single-object paths (properties mirror, session
                // replay, tree focus) keep working; the extras live in
                // `selected_multi`.
                let previous: Vec<u64> = self
                    .selected_mirror
                    .into_iter()
                    .chain(self.selected_multi.iter().copied())
                    .collect();
                self.selected_mirror = Some(ids[0]);
                self.selected_multi = ids[1..].to_vec();
                self.prune_appearance_registry();
                self.selection_flag_diff(&previous, ids);
            }
        }
    }

    /// Apply the selection flag transitions of the symmetric difference
    /// between the previous selection and the new one (005 A10 — at most
    /// one uniform write per affected object).
    fn selection_flag_diff(&mut self, previous: &[u64], current: &[u64]) {
        use std::collections::HashSet;
        let previous_set: HashSet<u64> = previous.iter().copied().collect();
        let current_set: HashSet<u64> = current.iter().copied().collect();
        let mut affected: Vec<u64> = previous_set
            .symmetric_difference(&current_set)
            .copied()
            .collect();
        affected.sort_unstable();
        for id in affected {
            self.set_selection_flag(id, current_set.contains(&id));
        }
    }

    /// Write one object's selection flag at its current appearance composite
    /// (the shared-setup invariant the single route relies on, shared by
    /// the multi diff above).
    fn set_selection_flag(&mut self, id: u64, is_selected: bool) {
        let Some(object) = self.scene.get(id) else {
            return;
        };
        let appearance = match self.appearances.get(&id).copied() {
            Some(entry) => {
                let next = entry.with_selected(is_selected);
                if next != entry {
                    // Registry rows always carry the current selection
                    // bit, so a replay writes the composite exactly as
                    // stored.
                    self.appearances.insert(id, next);
                }
                next
            }
            None => {
                // No override active: the uniform holds the upload
                // default plus the flag of the last mirror. Write the
                // flag transition — for a face-carrying mesh this also
                // restores its baked albedo when the selection leaves
                // it, which is exactly why the entry-less composite is
                // the upload default, not an all-zero albedo.
                upload_default_appearance(&object.object).with_selected(is_selected)
            }
        };
        self.push_appearance_uniform(id, &appearance);
    }

    pub fn set_selected(&mut self, selected: Option<u64>) {
        // An id that left the scene cannot be selected: normalize it to a
        // deselection. The mirror stores only normalized values.
        let selected = selected.filter(|id| self.scene.get(*id).is_some());
        if self.selected_mirror == selected && self.selected_multi.is_empty() {
            return;
        }
        let previous = self.selected_mirror;
        self.selected_mirror = selected;
        self.selected_multi.clear();
        self.prune_appearance_registry();
        // Only the two affected objects can differ from their current
        // uniform state — every other object's flag is already the one it
        // should keep — so a change writes at most two uniforms.
        for candidate in [previous, selected] {
            let Some(id) = candidate else {
                continue;
            };
            self.set_selection_flag(id, selected == Some(id));
        }
    }

    /// Merge box/click hits with the existing selection per the 005 A10
    /// modifier protocol: shift adds, ctrl subtracts, neither replaces.
    /// The ambiguous shift+ctrl combo replaces (deterministic).
    fn mix_selection(&self, hits: &[u64], add: bool, subtract: bool) -> Vec<u64> {
        let current = self.selected_ids();
        if add && subtract {
            return hits.to_vec();
        }
        if add {
            let mut merged = current;
            for id in hits {
                if !merged.contains(id) {
                    merged.push(*id);
                }
            }
            return merged;
        }
        if subtract {
            return current
                .into_iter()
                .filter(|id| !hits.contains(id))
                .collect();
        }
        hits.to_vec()
    }

    /// Combined world bounds of the selection (005 A6): the bounds of the
    /// data-backed objects, folded with the anchor points of frames and
    /// markers (which report no bounds). `None` with an empty selection.
    fn selection_bounds(&self) -> Option<io::Aabb> {
        let mut merged: Option<io::Aabb> = None;
        let mut merge = |a: io::Aabb| {
            merged = Some(match merged {
                Some(m) => io::Aabb {
                    min: m.min.min(a.min),
                    max: m.max.max(a.max),
                },
                None => a,
            });
        };
        for id in self.selected_ids() {
            let Some(object) = self.scene.get(id) else {
                continue;
            };
            match object.object.bounds() {
                Some(bounds) => merge(bounds),
                None => {
                    let anchor = match &object.object {
                        roboview_core::displays::DisplayObject::Frame(frame) => Some(frame.origin),
                        roboview_core::displays::DisplayObject::Marker(marker) => match marker {
                            roboview_core::displays::Marker::Text(text) => Some(text.anchor),
                            roboview_core::displays::Marker::Arrow(arrow) => Some(arrow.start),
                        },
                        _ => None,
                    };
                    if let Some(point) = anchor {
                        merge(io::Aabb {
                            min: point,
                            max: point,
                        });
                    }
                }
            }
        }
        merged
    }

    /// 005 A6: focus the camera on the selection — framing to the combined
    /// bounds (or anchor points); with an empty selection this is a no-op.
    pub fn focus_selection(&mut self) {
        let Some(bounds) = self.selection_bounds() else {
            return;
        };
        self.scene.camera = roboview_core::scene::camera::OrbitCamera::framing(Some(&bounds));
    }

    /// The ids of the current selection (primary first, then the extras),
    /// in the order the selection was resolved.
    pub fn selected_ids(&self) -> Vec<u64> {
        self.selected_mirror
            .into_iter()
            .chain(self.selected_multi.iter().copied())
            .collect()
    }

    /// Commit one batch of field edits to a single scene object (004 spec
    /// §4 A3/A4 — the property panel's commit requests). The effect is
    /// within one frame: the edits, the possible re-upload, and the paint
    /// all run on the UI thread of a single update.
    ///
    /// CPU fields are updated in place; an edit that changed geometry —
    /// frame origin/length, arrow start/end — re-provisions the object's
    /// GPU handle through the shared upload dispatch (a full re-upload:
    /// frame and marker geometry is tiny, and the spec sanctions the
    /// retransmit, plan §3.5). Text-marker edits need no re-upload (labels
    /// hold no GPU data), and name/visibility touch the scene entry only.
    /// An edit whose variant does not fit the object's kind is a no-op, and
    /// a vanished id skips the whole batch. The object's appearance
    /// (override, selection) is untouched by field edits — the registry is
    /// orthogonal to the CPU fields.
    pub fn apply_object_edits(&mut self, id: u64, edits: &[ObjectEdit]) {
        let geometry_dirty = {
            let Some(object) = self.scene.get_mut(id) else {
                return;
            };
            let mut dirty = false;
            for edit in edits {
                match (edit, &mut object.object) {
                    (ObjectEdit::Rename(name), _) => {
                        let trimmed = name.trim();
                        if !trimmed.is_empty() {
                            // Same rule as the tree's inline rename: blank
                            // names are rejected, and the input is trimmed.
                            object.name = trimmed.to_owned();
                        }
                    }
                    (ObjectEdit::Visible(visible), _) => object.visible = *visible,
                    (ObjectEdit::Origin(origin), DisplayObject::Frame(frame)) => {
                        if frame.origin != *origin {
                            frame.origin = *origin;
                            dirty = true;
                        }
                    }
                    (ObjectEdit::Length(length), DisplayObject::Frame(frame)) => {
                        if frame.length != *length {
                            frame.length = *length;
                            dirty = true;
                        }
                    }
                    (ObjectEdit::Anchor(anchor), DisplayObject::Marker(Marker::Text(text))) => {
                        if text.anchor != *anchor {
                            text.anchor = *anchor;
                        }
                    }
                    (ObjectEdit::Text(text), DisplayObject::Marker(Marker::Text(marker))) => {
                        if marker.text != *text {
                            marker.text = text.clone();
                        }
                    }
                    (ObjectEdit::Start(start), DisplayObject::Marker(Marker::Arrow(arrow))) => {
                        if arrow.start != *start {
                            arrow.start = *start;
                            dirty = true;
                        }
                    }
                    (ObjectEdit::End(end), DisplayObject::Marker(Marker::Arrow(arrow))) => {
                        dirty |= arrow.end != *end;
                        arrow.end = *end;
                    }
                    // Anything else is a kind mismatch and no-ops: the tree
                    // column of the selected kind is the contract.
                    _ => {}
                }
            }
            dirty
        };
        if geometry_dirty {
            self.reupload_object(id);
        }
    }

    /// Set one group default color of the objects tree in the viewport
    /// (004 spec M4/D4 — the default colors *new* members of that kind).
    /// The tree's chips are the authoring side (the objects panel state);
    /// main.rs syncs them here under the same lock that adds objects, so
    /// creation can read the current value through
    /// [`ViewportState::appearance_default_for_new`]. No frame cost when
    /// the tree did not change: inserting the same value is idempotent.
    pub fn set_group_default_color(&mut self, kind: DisplayKind, color: io::Color) {
        self.group_default_colors.insert(kind, color);
    }

    /// The color a newly created object of `kind` starts with: the
    /// user-set group default, or the shared unset marker
    /// ([`GROUP_COLOR_UNSET`]) while the group has none. The caller applies
    /// the color (through [`ViewportState::appearance_override`]) only when
    /// it differs from the marker — the marker is "no default", and it
    /// numerically equals the objects panel's own unset marker.
    pub fn appearance_default_for_new(&self, kind: DisplayKind) -> io::Color {
        self.group_default_colors
            .get(&kind)
            .copied()
            .unwrap_or(GROUP_COLOR_UNSET)
    }

    /// Apply the group's configured default color to a freshly added member
    /// (D4: new members only). A no-op when the group has none — the unset
    /// sentinel equals the objects-panel marker. Colorable kinds only (the
    /// file-install caller never hands over frame/marker kinds).
    pub fn apply_new_member_default_color(&mut self, id: u64, kind: DisplayKind) {
        let default = self.appearance_default_for_new(kind);
        if default != GROUP_COLOR_UNSET {
            self.appearance_override(id, default);
        }
    }

    /// Write one appearance composite into the GPU uniform of `id` — an
    /// in-place 64-byte queue write through the object's existing handle
    /// (spec §6: an appearance change never rebuilds anything). A no-op
    /// when the renderer does not exist yet, when the handle is not
    /// provisioned, or for text markers (they hold no GPU data): the CPU
    /// session state is already current, and the next renderer build — or
    /// the next geometry re-upload — replays it.
    fn push_appearance_uniform(&self, id: u64, appearance: &Appearance) {
        let (Some(renderer), Some(mesh_pipeline), Some(line_pipeline)) = (
            self.renderer.as_ref(),
            self.mesh_pipeline.as_ref(),
            self.line_pipeline.as_ref(),
        ) else {
            return;
        };
        let Some(object) = self.scene.get(id) else {
            return;
        };
        write_appearance_uniform(
            renderer,
            mesh_pipeline,
            line_pipeline,
            &object.object,
            appearance,
        );
    }

    /// Drop the appearance rows of objects that left the scene. Deletions
    /// flow through the objects panel's actions, which `main.rs` applies
    /// straight to the scene — outside this file — so the pruning is
    /// self-healing: every mutation entry point of the commit service
    /// calls it. The scene never reuses ids, so a stale row could never be
    /// read again; pruning is memory hygiene and keeps the registry a true
    /// mirror of the scene.
    fn prune_appearance_registry(&mut self) {
        self.appearances
            .retain(|id, _| self.scene.get(*id).is_some());
    }

    /// Re-provision the GPU handle of one object from its current CPU
    /// fields through the shared upload dispatch — the re-upload arm of
    /// [`ViewportState::apply_object_edits`] (frame and marker-arrow
    /// geometry is tiny, so a full retransmit is the sanctioned path, plan
    /// §3.5).
    ///
    /// The new handle replaces the object's old one, so the A6 handle
    /// ledger sees one more created event of the object's kind — every
    /// upload arm records it — and the old buffers free through wgpu's
    /// deferred destruction. That is exactly the accounting the renderer
    /// rebuild of [`ViewportState::sync_renderer`] already runs for every
    /// object (render/counters.rs module docs: re-uploads count upload
    /// events against display removals, never against buffer destruction);
    /// no other ledger row exists anywhere in this service. The appearance
    /// channel then replays from the session state, because the fresh
    /// handle provisions the upload default: the registered override
    /// composite, or the selection flag of a selected object without an
    /// override.
    fn reupload_object(&mut self, id: u64) {
        let Some(renderer) = self.renderer.as_mut() else {
            // No renderer yet (egui's wgpu render state arrives with the
            // first frame): the CPU fields were updated above, and the
            // next renderer build re-uploads every object from them.
            return;
        };
        // Invariant of sync_renderer: the renderer and both family
        // pipelines are created and rebuilt together, so a present renderer
        // implies present pipelines.
        let mesh_pipeline = self
            .mesh_pipeline
            .as_mut()
            .expect("mesh pipeline must exist whenever the renderer does");
        let line_pipeline = self
            .line_pipeline
            .as_mut()
            .expect("line pipeline must exist whenever the renderer does");
        {
            let Some(object) = self.scene.get_mut(id) else {
                return;
            };
            upload_display(renderer, mesh_pipeline, line_pipeline, &mut object.object);
        }
        let selected_set = self
            .selected_ids()
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        let composite = self.scene.get(id).and_then(|object| {
            let selected = if selected_set.contains(&object.id) {
                Some(object.id)
            } else {
                None
            };
            session_appearance(&self.appearances, selected, object)
        });
        if let Some(composite) = composite {
            self.push_appearance_uniform(id, &composite);
        }
    }

    // — Helper-layer session switches (004 spec §6, A11) —

    /// Whether the ground grid of the helper layer draws (default on). The
    /// single source for every door — the menu check items, the toolbar
    /// Grid button, the HUD corner badges — so all of them read this
    /// getter and flip through [`ViewportState::toggle_grid`].
    pub fn grid_on(&self) -> bool {
        self.grid_on
    }

    /// Whether the world-origin axis trio of the helper layer draws
    /// (default on); same door contract as [`ViewportState::grid_on`].
    pub fn axes_on(&self) -> bool {
        self.axes_on
    }

    /// Flip the ground-grid switch. A pure session toggle: the persistent
    /// mesh keeps its buffers and simply stops being painted (visibility
    /// never releases resources, spec §6), so toggling back on resumes the
    /// refresh-on-camera-change flow without re-provisioning. Callers
    /// reconcile the native menu check items after the flip (main.rs).
    pub fn toggle_grid(&mut self) {
        self.grid_on = !self.grid_on;
    }

    /// Flip the world-origin axes switch; same semantics as
    /// [`ViewportState::toggle_grid`].
    pub fn toggle_axes(&mut self) {
        self.axes_on = !self.axes_on;
    }

    /// Pointer → world intersection of the frame being built (status bar
    /// world coordinate, spec M5). Plane: the Z=0 ground grid while the
    /// grid is on, the camera-target plane otherwise — the same
    /// reference-plane rule `pointer_world` documents.
    pub fn pointer_world(&self) -> Option<Vec3> {
        let pos = self.pointer_hover?;
        let rect = self.viewport_rect;
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return None;
        }
        let aspect = rect.width() / rect.height();
        let view_proj = self.scene.camera.view_proj(aspect);
        let plane = if self.grid_on {
            roboview_core::render::camera_math::WorldPlane::GroundZ0
        } else {
            roboview_core::render::camera_math::WorldPlane::CameraTargetPlane
        };
        roboview_core::render::camera_math::pointer_world(
            &view_proj,
            vec2(rect.width(), rect.height()),
            pos,
            plane,
        )
    }

    /// Refresh the helper layer for the frame being prepared: provision
    /// the persistent helper meshes on first need (or after a renderer
    /// rebuild dropped them) and regenerate the ground grid when the
    /// view-projection changed since the last refresh.
    ///
    /// Runs from `prepare` — the only callback stage with a queue — and
    /// refreshes through [`render::LinePipeline::update_mesh`], which
    /// writes the mesh's own buffers in place (zero allocation, and never
    /// through the upload ledger, A6).
    fn refresh_helper_layer(&mut self) {
        // The pipeline is taken out while the layer mutates itself through
        // methods — and put back untouched afterwards. That sidesteps the
        // "self.line_pipeline borrowed while self borrowed mutably" split
        // that the refresh calls otherwise trip (safe: the slot is
        // restored on every exit path).
        let mut slot = self.line_pipeline.take();
        let Some(line_pipeline) = slot.as_mut() else {
            return;
        };
        self.refresh_helper_layer_with(line_pipeline);
        self.line_pipeline = slot;
    }

    fn refresh_helper_layer_with(&mut self, line_pipeline: &mut render::line::LinePipeline) {
        // World-origin axes: three one-segment meshes at the fixed length.
        // World-fixed geometry needs no camera refresh, so this runs once
        // per renderer lifetime.
        if self.axes_on && self.axes_meshes.is_none() {
            let (axis_x, axis_y, axis_z) = theme::ORIGIN_AXIS;
            let axes = [(Vec3::X, axis_x), (Vec3::Y, axis_y), (Vec3::Z, axis_z)];
            let mut meshes = [
                line_pipeline.with_capacity(1),
                line_pipeline.with_capacity(1),
                line_pipeline.with_capacity(1),
            ];
            for (mesh, (axis, color)) in meshes.iter_mut().zip(axes) {
                line_pipeline.update_mesh(mesh, &[[Vec3::ZERO, axis * ORIGIN_AXIS_LENGTH]], color);
            }
            self.axes_meshes = Some(meshes);
        }
        if !self.grid_on && !self.axes_on {
            return;
        }
        let view_proj = self.scene.camera.view_proj(self.aspect());
        if self.grid_refresh_key == Some(view_proj) {
            return;
        }
        // The key is stored first, so an off-view ground (no window — e.g.
        // the plane beyond the far plane) is not recomputed every frame
        // while the camera sits still; its old strips are necessarily
        // off-view too, because no ground point is visible then.
        self.grid_refresh_key = Some(view_proj);
        if self.grid_mesh.is_none() {
            // Prebuild at the segment capacity of the capped window: the
            // grid module guarantees `grid_strips` of any radius up to the
            // options radius fits `segment_capacity_bound(options)`, so
            // the window clamped to the cap below can never outgrow the
            // prebuilt buffers (render/grid.rs).
            let options = render::grid::GridOptions::new(GRID_STEP, GRID_RADIUS_CAP, 2.0);
            let mesh = line_pipeline.with_capacity(render::grid::segment_capacity_bound(&options));
            self.grid_mesh = Some(mesh);
        }
        let window = grid_window(&view_proj, self.viewport_size());
        if self.grid_on {
            let Some(window) = window else {
                // The grid stays off-view; the axis lines refresh with the
                // same key below regardless (their own motion depends on
                // the same window).
                self.refresh_axis_lines(line_pipeline, None);
                return;
            };
            self.refresh_grid_mesh(line_pipeline, window);
            self.refresh_axis_lines(line_pipeline, Some(window));
        } else {
            self.refresh_axis_lines(line_pipeline, window);
        }
    }

    /// Refresh the through-origin axis reference lines (004 revision
    /// 2026-09-05, Blender-style): one segment per axis spanning the
    /// visible ground window, so X (red) and Y (green) cross the whole
    /// ground like Blender's axis lines; Z (blue) uses the same extent.
    /// Runs under the grid refresh key (any camera move), lazily on the
    /// first axes-on frame.
    fn refresh_axis_lines(
        &mut self,
        line_pipeline: &mut render::line::LinePipeline,
        window: Option<GridWindow>,
    ) {
        let (axis_x, axis_y, axis_z) = theme::ORIGIN_AXIS;
        let axes = [(Vec3::X, axis_x), (Vec3::Y, axis_y), (Vec3::Z, axis_z)];
        if self.axis_lines.is_none() {
            self.axis_lines = Some([
                line_pipeline.with_capacity(1),
                line_pipeline.with_capacity(1),
                line_pipeline.with_capacity(1),
            ]);
        }
        let Some(window) = window else {
            // Off-view ground: the old lines stay (they are off-view too).
            return;
        };
        let r = window.radius;
        let segs = [
            Vec3::new(-r, 0.0, 0.0)..Vec3::new(r, 0.0, 0.0),
            Vec3::new(0.0, -r, 0.0)..Vec3::new(0.0, r, 0.0),
            Vec3::new(0.0, 0.0, -r)..Vec3::new(0.0, 0.0, r),
        ];
        for (i, (_, color)) in axes.iter().enumerate() {
            let Some(mesh) = self.axis_lines.as_mut().map(|m| &mut m[i]) else {
                return;
            };
            let seg = [segs[i].start, segs[i].end];
            line_pipeline.update_mesh(mesh, &[seg], *color);
        }
    }

    /// Refresh the persistent ground grid (capacity prebuild + in-place
    /// update_mesh) for the current pose; `window` is the fresh visible-
    /// ground window.
    fn refresh_grid_mesh(
        &mut self,
        line_pipeline: &mut render::line::LinePipeline,
        window: GridWindow,
    ) {
        // Pixels-per-meter at the camera-target plane: the zoom metric of
        // the step ladder. It depends on the eye-to-target distance (and
        // the FOV/viewport) only, never on pitch or yaw — so rotating the
        // camera never reselects the grid step (004 revision 2026-09-04).
        let distance = self.scene.camera.distance();
        let px_per_m = self.viewport_size().y
            / (2.0 * distance * (self.scene.camera.vertical_fov() / 2.0).tan());
        let px_per_m = if px_per_m.is_finite() && px_per_m > 0.0 {
            px_per_m
        } else {
            1.0
        };
        let view = render::grid::GridView::new(
            Vec3::new(window.center.x, window.center.y, 0.0),
            render::grid::GridOptions::new(GRID_STEP, window.radius, px_per_m),
        );
        let strips = render::grid::grid_strips(&view);
        let mesh = self
            .grid_mesh
            .as_mut()
            .expect("grid mesh provisioned above");
        line_pipeline.update_mesh(mesh, &strips, theme::GRID_LINE);
    }

    /// Size (points) of the current frame's viewport rect — the grid
    /// window's pixel-space input. Egui painter space equals the core's
    /// pixel space up to a uniform scale, and the window math only uses
    /// ratios, so the point-space rect is the correct size.
    fn viewport_size(&self) -> Vec2 {
        Vec2::new(self.viewport_rect.width(), self.viewport_rect.height())
    }

    /// Whether the renderer exists yet. The renderer is created from
    /// eframe's wgpu render state at the top of every update, which exists
    /// from the first frame on native; the guard keeps the install path
    /// honest for hosts that never expose one.
    pub fn renderer_ready(&self) -> bool {
        self.renderer.is_some()
    }

    /// (Center, scale) pair the add dialogs start their fields from: the
    /// center and largest dimension of the visible bounds union, or a
    /// zero/default pair when the scene has nothing to measure (empty
    /// scene, or only frames/markers — which never participate in the
    /// union, displays/mod.rs).
    pub fn ui_defaults(&self) -> (Vec3, f32) {
        let Some(bounds) = self.scene.bounds_union() else {
            return (Vec3::ZERO, DEFAULT_UI_SCALE);
        };
        let extent = bounds.largest_dimension();
        if !extent.is_finite() || extent <= 0.0 {
            // Degenerate union (a single-point cloud): frame its center at
            // the default scale, mirroring the camera's own fallback.
            (bounds.center(), DEFAULT_UI_SCALE)
        } else {
            (bounds.center(), extent)
        }
    }

    /// Aspect ratio (width / height) of the current frame's viewport rect.
    fn aspect(&self) -> f32 {
        let rect = self.viewport_rect;
        if rect.is_finite() && rect.width() > 0.0 && rect.height() > 0.0 {
            rect.width() / rect.height()
        } else {
            // Degenerate rect (minimized window, first frame): the camera
            // itself falls back to aspect 1.0, keep the guard here too.
            1.0
        }
    }
}

impl Default for ViewportState {
    fn default() -> Self {
        Self::new()
    }
}

/// Provision (or re-provision) the GPU handle of one display object
/// through the pipeline that matches its kind. This is the single upload
/// dispatch of the scene: file installs and renderer rebuilds both call
/// it, so a new display kind needs exactly one arm here.
///
/// Text markers hold no GPU data (spec §7 F4) and are skipped.
fn upload_display(
    renderer: &mut render::Renderer,
    mesh_pipeline: &render::MeshPipeline,
    line_pipeline: &render::LinePipeline,
    display: &mut DisplayObject,
) {
    match display {
        DisplayObject::PointCloud(cloud) => {
            cloud.mesh = Some(renderer.upload(&cloud.data));
        }
        DisplayObject::Mesh(mesh) => {
            // Face-less files upload as a scatter through the mesh
            // pipeline, whose upload returns the matching GPU shape
            // (render/mesh.rs, spec §7 F1).
            mesh.gpu = Some(mesh_pipeline.upload(&mesh.data));
        }
        DisplayObject::Path(path) => {
            path.gpu = Some(line_pipeline.upload_path(&path.data));
        }
        DisplayObject::Frame(frame) => {
            frame.gpu = Some(line_pipeline.upload_frame(frame.origin, frame.length));
        }
        DisplayObject::Marker(Marker::Arrow(arrow)) => {
            arrow.gpu = Some(line_pipeline.upload_arrow(arrow.start, arrow.end));
        }
        DisplayObject::Marker(Marker::Text(_)) => {}
    }
}

/// The appearance the commit service must (re-)write for one object in the
/// current session: the registered override composite when one exists (its
/// flags always carry the current selection bit, kept in sync by
/// [`ViewportState::set_selected`]), else the object's upload default plus
/// the selection flag when the object is selected. `None` when neither
/// applies — a fresh upload already provisions exactly the default, so
/// nothing needs writing.
///
/// The mirror-less default composite matters for one subtle case: a
/// face-carrying mesh without an override that is *not* selected must be
/// written as its baked face default, not as an all-zero albedo — the mesh
/// shader always reads the albedo, and the flag alone would blacken it.
fn session_appearance(
    appearances: &HashMap<u64, Appearance>,
    selected: Option<u64>,
    object: &SceneObject<DisplayObject>,
) -> Option<Appearance> {
    if let Some(entry) = appearances.get(&object.id).copied() {
        return Some(entry);
    }
    if selected == Some(object.id) {
        Some(upload_default_appearance(&object.object).with_selected(true))
    } else {
        None
    }
}

/// The appearance a fresh GPU upload of `display` provisions: what the
/// uniform holds right after the upload, before any session state replays.
/// Everything uploads with [`Appearance::DEFAULT`] (albedo 0, no flags —
/// the point and line shaders gate on the override flag), except
/// face-carrying triangle meshes, whose shader always reads the albedo:
/// they provision their baked face color
/// ([`MESH_FACE_DEFAULT_ALBEDO`]). A face-less mesh uploads as a scatter
/// (through the point pipeline) and takes the default like any point
/// geometry.
fn upload_default_appearance(display: &DisplayObject) -> Appearance {
    match display {
        DisplayObject::Mesh(mesh) => match mesh.gpu.as_ref() {
            Some(render::MeshGpu::Faces(_)) => Appearance::new(MESH_FACE_DEFAULT_ALBEDO, 0),
            _ => Appearance::DEFAULT,
        },
        _ => Appearance::DEFAULT,
    }
}

/// Write one appearance composite into the GPU uniform of a display object
/// — the single in-place write dispatch of the commit service, mirroring
/// the handle lookup of [`upload_display`]. Each arm is one 64-byte queue
/// write through the object's existing handle (`set_appearance` of the
/// owning pipeline); nothing is re-uploaded and no A6 ledger row moves.
///
/// Objects without a provisioned handle are skipped — a text marker holds
/// no GPU data at all, and every other kind gains its handle on the next
/// renderer build, which replays the session appearance then.
fn write_appearance_uniform(
    renderer: &render::Renderer,
    mesh_pipeline: &render::MeshPipeline,
    line_pipeline: &render::LinePipeline,
    display: &DisplayObject,
    appearance: &Appearance,
) {
    match display {
        DisplayObject::PointCloud(cloud) => {
            if let Some(mesh) = cloud.mesh.as_deref() {
                renderer.set_appearance(mesh, appearance);
            }
        }
        DisplayObject::Mesh(mesh) => {
            // The same split as the paint pass: a face-less mesh file
            // uploaded as a scatter and owns a point-pipeline mesh.
            match mesh.gpu.as_ref() {
                Some(render::MeshGpu::Faces(faces)) => {
                    mesh_pipeline.set_appearance(faces, appearance);
                }
                Some(render::MeshGpu::Scatter(scatter)) => {
                    renderer.set_appearance(scatter, appearance);
                }
                None => {}
            }
        }
        DisplayObject::Path(path) => {
            if let Some(mesh) = path.gpu.as_deref() {
                line_pipeline.set_appearance(mesh, appearance);
            }
        }
        DisplayObject::Frame(frame) => {
            if let Some(mesh) = frame.gpu.as_deref() {
                line_pipeline.set_appearance(mesh, appearance);
            }
        }
        DisplayObject::Marker(Marker::Arrow(arrow)) => {
            if let Some(mesh) = arrow.gpu.as_deref() {
                line_pipeline.set_appearance(mesh, appearance);
            }
        }
        DisplayObject::Marker(Marker::Text(_)) => {}
    }
}

// — Ground-grid generation window (pure math: no GPU types, so the tests
//   at the end of this file exercise it headlessly) —

/// A square generation window of the ground grid on the world Z=0 plane:
/// `center` holds the world (x, y) coordinates of the window center, and
/// the grid spans `center ± radius` — every ground point visible from the
/// camera lies inside the square.
#[derive(Debug, Clone, Copy, PartialEq)]
struct GridWindow {
    center: Vec2,
    radius: f32,
}

/// The ground-grid window covering exactly the ground visible from
/// `view_proj` on a `viewport` of egui points (any uniform scale works —
/// the math only uses ratios):
///
/// - the visible ground is the intersection of the Z=0 plane with the
///   frustum — a convex polygon whose vertices are the crossings of the
///   plane with the twelve frustum edges (four eye-to-far-corner rays,
///   four near-rectangle edges, four far-rectangle edges);
/// - the window is the square `center ± radius` around it: `center` is the
///   hit of the view axis on the plane — the ground point the user looks
///   at (`pointer_world` on the center pixel) — or, when the axis misses
///   the plane (an eye-level camera looking along it), the center of the
///   visible polygon itself; `radius` is the farthest polygon-vertex
///   distance, so the disc — and therefore the square — contains every
///   visible ground point (a convex set reaches its extremes at polygon
///   vertices);
/// - `None` when no ground is visible at all (beyond the far plane, an
///   eye sitting on the plane looking along it, …) or the inputs are
///   degenerate.
///
/// The returned radius is pre-clamped to [`GRID_RADIUS_CAP`], with the
/// [`GRID_RADIUS_MARGIN`] slack folded in.
fn grid_window(view_proj: &Mat4, viewport: Vec2) -> Option<GridWindow> {
    if !view_proj.is_finite() || !viewport.is_finite() || viewport.x <= 0.0 || viewport.y <= 0.0 {
        return None;
    }
    let inv = view_proj.inverse();
    if !inv.is_finite() {
        return None;
    }
    // World corners of the near and far viewport rectangles: the near
    // points come from the corner pixel rays, the far points from the same
    // rays unprojected at clip depth 1 — the identical pair the ray math
    // uses (render/camera_math.rs).
    let mut near = [Vec3::ZERO; 4];
    let mut far = [Vec3::ZERO; 4];
    let corner_pixels = [
        Vec2::new(0.0, 0.0),
        Vec2::new(viewport.x, 0.0),
        Vec2::new(viewport.x, viewport.y),
        Vec2::new(0.0, viewport.y),
    ];
    for (i, pixel) in corner_pixels.into_iter().enumerate() {
        let (n, _dir) = render::camera_math::screen_to_ray(view_proj, viewport, pixel)?;
        let ndc = Vec2::new(
            pixel.x / viewport.x * 2.0 - 1.0,
            1.0 - pixel.y / viewport.y * 2.0,
        );
        let far_h = inv * Vec4::new(ndc.x, ndc.y, 1.0, 1.0);
        if !far_h.is_finite() || far_h.w <= 0.0 {
            return None;
        }
        near[i] = n;
        far[i] = far_h.truncate() / far_h.w;
    }
    // Cross the plane with the twelve frustum edges.
    let mut vertices = Vec::<Vec3>::with_capacity(12);
    for k in 0..4 {
        let j = (k + 1) % 4;
        push_plane_crossing(near[k], far[k], &mut vertices);
        push_plane_crossing(near[k], near[j], &mut vertices);
        push_plane_crossing(far[k], far[j], &mut vertices);
    }
    // Window center: the view axis' ground hit when it exists (inside the
    // frustum — pointer_world validates), otherwise the visible polygon's
    // own center; with no vertices at all the plane is off-view.
    let center = match render::camera_math::pointer_world(
        view_proj,
        viewport,
        viewport * 0.5,
        render::camera_math::WorldPlane::GroundZ0,
    ) {
        Some(hit) => hit.truncate(),
        None => {
            if vertices.is_empty() {
                return None;
            }
            let mut min = Vec2::splat(f32::INFINITY);
            let mut max = Vec2::splat(f32::NEG_INFINITY);
            for vertex in &vertices {
                min = min.min(vertex.truncate());
                max = max.max(vertex.truncate());
            }
            (min + max) * 0.5
        }
    };
    if !center.is_finite() {
        return None;
    }
    let radius = vertices
        .iter()
        .map(|vertex| (vertex.truncate() - center).length())
        .fold(0.0_f32, f32::max);
    if !radius.is_finite() || radius <= 0.0 {
        return None;
    }
    Some(GridWindow {
        center,
        radius: (radius * GRID_RADIUS_MARGIN).min(GRID_RADIUS_CAP),
    })
}

/// Push the crossing of the segment `a → b` with the world plane z = 0
/// into `out`. A segment parallel to the plane never crosses it — an
/// entire edge lying *on* the plane contributes no vertex either, because
/// the crossings of the adjacent edges bound that stretch. The acceptance
/// window `s ∈ [−ε, 1+ε]` admits crossings a hair outside the segment so
/// float rounding at an endpoint cannot drop a real vertex (the window
/// margin absorbs the slack).
fn push_plane_crossing(a: Vec3, b: Vec3, out: &mut Vec<Vec3>) {
    let dz = b.z - a.z;
    if dz == 0.0 {
        return;
    }
    let s = -a.z / dz;
    if !s.is_finite() || !(-1.0e-3..=1.0 + 1.0e-3).contains(&s) {
        return;
    }
    let hit = a + (b - a) * s;
    if hit.is_finite() {
        out.push(hit);
    }
}

/// egui-wgpu paint callback drawing every visible object of a
/// [`ViewportState`].
///
/// Registered per frame by [`show_viewport`] through
/// `egui_wgpu::Callback::new_paint_callback`; see the module docs for the
/// prepare/paint flow and for the three-pass draw order.
struct ViewportCallback {
    state: Arc<Mutex<ViewportState>>,
}

impl egui_wgpu::CallbackTrait for ViewportCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        _callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let mut state = lock_state(&self.state);
        let Some(renderer) = state.renderer.as_ref() else {
            return Vec::new();
        };
        // The shared view-proj uniform is written once per frame in prepare
        // (the only callback stage with a queue). The aspect comes from the
        // viewport rect that `show_viewport` recorded for this same frame
        // (update runs before the callbacks, so the rect is never stale).
        let view_proj = state.scene.camera.view_proj(state.aspect());
        renderer.update_uniform(queue, view_proj);
        // Helper layer (spec §6): the same prepare provisions the
        // persistent grid/axes meshes and regenerates the ground grid on
        // camera change. `update_mesh` writes in place through the line
        // pipeline's own queue, so no queue access is needed here.
        state.refresh_helper_layer();
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        _callback_resources: &egui_wgpu::CallbackResources,
    ) {
        // egui-wgpu scopes the render pass viewport to this callback's rect
        // before painting and restores it afterwards, so drawing here never
        // leaks outside the viewport.
        let state = lock_state(&self.state);
        let (Some(renderer), Some(mesh_pipeline), Some(line_pipeline)) = (
            state.renderer.as_ref(),
            state.mesh_pipeline.as_ref(),
            state.line_pipeline.as_ref(),
        ) else {
            return;
        };

        // Three passes, one per pipeline, over the visible objects in add
        // order. The grouping keeps the shared-depth policy of the family
        // (render/mod.rs): the point pass writes the reference surface, the
        // mesh pass writes depth-biased triangles on top, the line pass
        // depth-tests without writing. Each paint call re-sets its group's
        // pipeline — redundant for consecutive objects but cheap at the
        // scene sizes this app draws; grouping still caps the pipeline
        // switches at three per frame.
        for object in state.scene.iter_visible() {
            match &object.object {
                // Pass 1 — point geometry: point clouds and face-less mesh
                // files (scatter), drawn by the point pipeline.
                DisplayObject::PointCloud(cloud) => {
                    if let Some(mesh) = cloud.mesh.as_deref() {
                        renderer.paint(render_pass, mesh);
                    }
                }
                DisplayObject::Mesh(mesh) => {
                    // A face-less mesh file uploads a scatter (spec §7 F1)
                    // and is drawn through the point pipeline; the handles
                    // are Arc-wrapped, the pipeline paints the inner mesh.
                    if let Some(render::MeshGpu::Scatter(scatter)) = mesh.gpu.as_ref() {
                        renderer.paint(render_pass, scatter);
                    }
                }
                _ => {}
            }
        }
        // Pass 2 — triangle meshes, drawn by the mesh pipeline.
        for object in state.scene.iter_visible() {
            if let DisplayObject::Mesh(mesh) = &object.object
                && let Some(render::MeshGpu::Faces(faces)) = mesh.gpu.as_ref()
            {
                mesh_pipeline.paint(render_pass, faces);
            }
        }
        // Pass 3a — the helper layer at the head of the line family (plan
        // §3.3, spec §6): the ground grid and the world-origin axes draw
        // before every scene line object, so paths, frames, and arrows
        // always overdraw them. Both depth-test against the reference the
        // two previous passes wrote and neither writes depth — a flat mesh
        // lying on Z=0 was pushed away by its own depth bias, which is
        // what lets the grid overdraw it, while content in front still
        // hides the grid. The switches are session state (`grid_on` /
        // `axes_on`); a switched-off helper keeps its meshes — visibility
        // never releases resources (spec §6).
        if state.grid_on
            && let Some(grid) = state.grid_mesh.as_ref()
        {
            line_pipeline.paint(render_pass, grid);
        }
        if state.axes_on
            && let Some(axes) = state.axes_meshes.as_ref()
        {
            for axis in axes {
                line_pipeline.paint(render_pass, axis);
            }
        }
        // The through-origin axis reference lines (Blender-style, 004
        // revision 2026-09-05) paint right after the origin short axes,
        // same semantic colors, same switch — long X/Y lines crossing the
        // whole ground beside the short labels.
        if state.axes_on
            && let Some(lines) = state.axis_lines.as_ref()
        {
            for line in lines {
                line_pipeline.paint(render_pass, line);
            }
        }
        // Pass 3b — the scene's line geometry: paths, frames, and marker
        // arrows, drawn by the line pipeline. Text markers hold no GPU data
        // and are painted by the overlay pass instead.
        for object in state.scene.iter_visible() {
            match &object.object {
                DisplayObject::Path(path) => {
                    if let Some(mesh) = path.gpu.as_deref() {
                        line_pipeline.paint(render_pass, mesh);
                    }
                }
                DisplayObject::Frame(frame) => {
                    if let Some(mesh) = frame.gpu.as_deref() {
                        line_pipeline.paint(render_pass, mesh);
                    }
                }
                DisplayObject::Marker(Marker::Arrow(arrow)) => {
                    if let Some(mesh) = arrow.gpu.as_deref() {
                        line_pipeline.paint(render_pass, mesh);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Draw the central 3D viewport into `ui`: allocate the remaining space,
/// feed pointer input to the camera, register this frame's paint callback,
/// and paint the overlay labels — or the empty-state/loading placeholders.
///
/// `locale` resolves the in-viewport copy (loading label, empty hint) per
/// frame; the state itself is locale-free (003 spec §6.3: `ViewportState`
/// has zero locale dependency — generated names are data, axis letters are
/// invariants).
pub fn show_viewport(
    ui: &mut egui::Ui,
    state: &Arc<Mutex<ViewportState>>,
    loading: bool,
    locale: Locale,
) {
    let (rect, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::drag());

    let (scene_empty, has_content) = {
        let mut viewport = lock_state(state);
        // The paint callback and the overlay pass of this frame read this
        // rect (aspect, label placement).
        viewport.viewport_rect = rect;
        viewport.pointer_hover = ui
            .ctx()
            .input(|i| i.pointer.hover_pos())
            .map(|p| Vec2::new(p.x, p.y));
        let scene_empty = viewport.scene.is_empty();
        // Camera input and the paint callback are gated on "content or
        // helper layer" (spec §6, A11): with the grid or the axes on, an
        // empty scene stays navigable and draws its helpers under the
        // placeholder text; switching both helpers off restores the plain
        // empty state. The camera pose is re-framed when the first object
        // loads (spec §6) and kept afterwards.
        let has_content = !scene_empty || viewport.grid_on() || viewport.axes_on();
        if has_content {
            camera_input::apply_pointer_events(
                &response,
                ui.ctx(),
                rect,
                &mut viewport.scene.camera,
            );
        }
        (scene_empty, has_content)
    };

    // The picking gestures run AFTER the lock block above: they take
    // their own short locks and a nested lock_state here would deadlock
    // the frame (the viewport mutex is not reentrant — the same trap as
    // the 004 install path and the 005 panel reads).
    if has_content {
        // 005 A9/A11: left click = pick (egui's click means the press
        // never crossed the drag threshold — below 4 px, so the same
        // button's drag stays box-select); a hit highlights at once
        // through the appearance channel and the app mirrors it.
        // With alt held the primary button simulates the middle button
        // (Blender-style, Magic Mouse users) — picking and box-select
        // stand down for that gesture (005 A11 revision).
        let alt_gesture = ui.ctx().input(|i| i.modifiers.alt);
        if response.clicked() && !alt_gesture {
            if let Some(pos) = response.interact_pointer_pos() {
                let cursor = Vec2::new(pos.x - rect.min.x, pos.y - rect.min.y);
                let (add, subtract) = ui.ctx().input(|i| (i.modifiers.shift, i.modifiers.ctrl));
                let clicked = {
                    let mut viewport = lock_state(state);
                    let clicked = viewport.pick_at(cursor).map(|hit| hit.id);
                    let ids = viewport.mix_selection(
                        &clicked.into_iter().collect::<Vec<_>>(),
                        add,
                        subtract,
                    );
                    viewport.set_selection(&ids);
                    ids
                };
                lock_state(state).report_click_selection(clicked);
            }
        }
        // 005 A9: primary drag = box-select behind the pointer
        // threshold; the camera is frozen for its duration (the drag
        // never reaches camera_input) and the rubber band is painted
        // by the overlay pass.
        if response.drag_started_by(egui::PointerButton::Primary) && !alt_gesture {
            if let Some(pos) = response.interact_pointer_pos() {
                lock_state(state).box_drag_start =
                    Some(Vec2::new(pos.x - rect.min.x, pos.y - rect.min.y));
            }
        }
        if response.drag_stopped_by(egui::PointerButton::Primary) && !alt_gesture {
            let (start, ids) = {
                let mut viewport = lock_state(state);
                let start = viewport.box_drag_start.take();
                let ids = start
                    .map(|s| {
                        let now = Vec2::new(rect.max.x - rect.min.x, rect.max.y - rect.min.y);
                        // The stopped pointer position is the last
                        // interact position of this drag.
                        let end = response
                            .interact_pointer_pos()
                            .map(|p| Vec2::new(p.x - rect.min.x, p.y - rect.min.y))
                            .unwrap_or(now);
                        let hits = viewport.box_pick(s, end);
                        let (add, subtract) =
                            ui.ctx().input(|i| (i.modifiers.shift, i.modifiers.ctrl));
                        viewport.mix_selection(&hits, add, subtract)
                    })
                    .unwrap_or_default();
                (start, ids)
            };
            if start.is_some() {
                lock_state(state).set_selection(&ids);
                lock_state(state).report_click_selection(ids);
                lock_state(state).box_drag_start = None;
                ui.ctx().request_repaint();
            }
        }
    }

    if !rect.is_finite() || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }

    let painter = ui.painter_at(rect);

    if has_content {
        // wgpu draws the scene and the helper layer itself; register the
        // paint callback for it.
        let callback = ViewportCallback {
            state: Arc::clone(state),
        };
        painter.add(egui_wgpu::Callback::new_paint_callback(rect, callback));

        // Overlay pass: marker text labels, frame axis letters, the world-
        // origin axis letters, and the corner orientation indicator,
        // painted after the 3D callback so they always sit on top (spec §6).
        paint_labels(&painter, rect, state);
        paint_origin_axis_labels(&painter, rect, state);
        paint_indicator(&painter, rect, state);

        // 005 A9: the box-select rubber band, painted after the 3D
        // callback like the labels so it always sits on top — a thin
        // select-highlight outline with a translucent interior.
        let hover = ui.ctx().input(|i| i.pointer.hover_pos());
        paint_box_band(&painter, rect, state, hover);
    }

    // Corner HUD switches of the helper layer (spec §6: the toolbar
    // buttons and these corner toggles are two doors onto the same state —
    // both read the ViewportState accessors and flip through toggles).
    paint_aux_switches(ui, rect, state, locale);

    if scene_empty {
        // Placeholder text above whatever the helper layer draws (spec
        // §6: the empty-state copy coexists with the helpers, which run
        // beneath it whenever the gate above passed).
        if loading {
            // No content yet: a spinner with the loading label, centered.
            let center = rect.center();
            let spinner_rect = egui::Rect::from_center_size(
                center - egui::vec2(0.0, 16.0),
                egui::vec2(28.0, 28.0),
            );
            ui.put(spinner_rect, egui::Spinner::new().size(28.0));
            painter.text(
                center + egui::vec2(0.0, 26.0),
                egui::Align2::CENTER_CENTER,
                texts::viewport_loading(locale),
                egui::FontId::proportional(16.0),
                ui.visuals().weak_text_color(),
            );
        }
        // (no empty-scene hint text — the helper layer alone reads as a
        // scene; the empty-state copy surface stays in texts.rs for the
        // 007 message center)
    } else if loading {
        // Content is present and a load keeps running: a subtle hint in
        // the corner while the scene stays interactive.
        let corner = egui::pos2(rect.left() + 12.0, rect.bottom() - 12.0);
        painter.text(
            corner,
            egui::Align2::LEFT_BOTTOM,
            texts::viewport_loading(locale),
            egui::FontId::proportional(13.0),
            ui.visuals().weak_text_color(),
        );
    }
}

/// Paint the overlay pass of one viewport frame: every visible text-marker
/// label, and the axis letters of every visible frame, projected through
/// the same view-projection matrix the 3D pass uses this frame
/// ([`render::anchor_to_screen`], spec §7 F3/F4). Anchors that project
/// outside the frustum are culled by the pure function, so labels appear
/// and disappear cleanly at the viewport edges.
/// Corner HUD switches of the helper layer (spec §6: the toolbar buttons
/// and these top-left corner toggles are two doors onto the same state —
/// both read the [`ViewportState`] accessors and flip through the
/// toggles). Drawn after the 3D callback like the other overlays, so the
/// switches always sit on top of the viewport.
fn paint_aux_switches(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    state: &Arc<Mutex<ViewportState>>,
    locale: Locale,
) {
    if !rect.width().is_finite() || rect.height() <= 0.0 {
        return;
    }
    let grid_on = lock_state(state).grid_on();
    let axes_on = lock_state(state).axes_on();
    egui::Area::new(egui::Id::new("viewport_aux_switches"))
        .fixed_pos(rect.left_top() + egui::vec2(8.0, 8.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(grid_on, texts::toggle_grid(locale))
                        .on_hover_text(texts::grid_toggle_tooltip(locale))
                        .clicked()
                    {
                        lock_state(state).toggle_grid();
                    }
                    if ui
                        .selectable_label(axes_on, texts::toggle_axes(locale))
                        .on_hover_text(texts::axes_toggle_tooltip(locale))
                        .clicked()
                    {
                        lock_state(state).toggle_axes();
                    }
                });
            });
        });
}

fn paint_labels(painter: &egui::Painter, rect: egui::Rect, state: &Arc<Mutex<ViewportState>>) {
    let viewport = lock_state(state);
    let view_proj = viewport.scene.camera.view_proj(viewport.aspect());
    for object in viewport.scene.iter_visible() {
        match &object.object {
            DisplayObject::Marker(Marker::Text(text)) => {
                if text.text.is_empty() {
                    continue;
                }
                if let Some(pos) = anchor_pos(&view_proj, rect, text.anchor) {
                    paint_label(
                        painter,
                        pos,
                        &text.text,
                        egui::FontId::proportional(14.0),
                        egui::Color32::WHITE,
                    );
                }
            }
            DisplayObject::Frame(frame) => {
                paint_frame_axis_labels(painter, &view_proj, rect, frame);
            }
            _ => {}
        }
    }
}

/// Paint the box-select rubber band (005 A9): the normalized drag rect
/// from the state, drawn with the selection-highlight token so it matches
/// the pick highlight response; the interior is translucent and never
/// blocks the 3D content (it is painted with the egui painter, not the
/// scene graph).
fn paint_box_band(
    painter: &egui::Painter,
    rect: egui::Rect,
    state: &Arc<Mutex<ViewportState>>,
    hover: Option<egui::Pos2>,
) {
    let Some(hover) = hover else {
        return;
    };
    let pointer = Vec2::new(hover.x - rect.min.x, hover.y - rect.min.y);
    let Some((a, b)) = lock_state(state).box_drag_rect(pointer) else {
        return;
    };
    let band = egui::Rect::from_min_max(
        egui::pos2(a.x + rect.min.x, a.y + rect.min.y),
        egui::pos2(b.x + rect.min.x, b.y + rect.min.y),
    );
    // The selection-highlight token (theme::SELECT_HIGHLIGHT, spec A9
    // palette) is shared with the pick highlight; the interior is
    // translucent so the scene stays readable under the band.
    let (r, g, b) = (255, 128, 0);
    painter.rect_filled(
        band,
        0.0,
        egui::Color32::from_rgba_unmultiplied(r, g, b, 31),
    );
    painter.rect_stroke(
        band,
        0.0,
        egui::Stroke::new(1.0_f32, egui::Color32::from_rgba_unmultiplied(r, g, b, 204)),
        egui::StrokeKind::Inside,
    );
}

/// Paint the orientation indicator in the top-right corner of the
/// viewport (spec §6, A11): a translucent disc — the theme's neutral
/// indicator base — under the on-screen directions of the world axes
/// +X/+Y/+Z at a fixed pixel size, with the axis letter at each arm tip.
/// The directions are read from the view-projection's linear columns every
/// frame ([`render::camera_math::orientation_gizmo_dirs`]: pure, state-less
/// math), so the arms rotate with the camera and an axis pointing straight
/// at the view reports itself invisible instead of drawing a spurious arm.
/// The indicator has no switch — it draws whenever the viewport has
/// content or a helper layer (A11).
fn paint_indicator(painter: &egui::Painter, rect: egui::Rect, state: &Arc<Mutex<ViewportState>>) {
    if rect.width() < INDICATOR_INSET * 2.0 || rect.height() < INDICATOR_INSET * 2.0 {
        // A viewport too small for the corner widget (the squeezed minimum
        // window keeps only a sliver): nothing is painted, and the disc
        // would hang outside the clipped rect anyway.
        return;
    }
    let viewport = lock_state(state);
    let view_proj = viewport.scene.camera.view_proj(viewport.aspect());
    let viewport_size = Vec2::new(rect.width(), rect.height());
    let center = egui::pos2(rect.right() - INDICATOR_INSET, rect.top() + INDICATOR_INSET);
    // The gizmo's own square is the reserved placement rect of the pure
    // direction function; its `rect`/`len` parameters belong to that
    // placement step and the math leaves them unused (camera_math.rs).
    let widget = egui::Rect::from_center_size(
        center,
        egui::vec2(INDICATOR_RADIUS * 2.0, INDICATOR_RADIUS * 2.0),
    );
    let gizmo_rect = render::camera_math::Rect2 {
        min: Vec2::new(widget.min.x, widget.min.y),
        max: Vec2::new(widget.max.x, widget.max.y),
    };
    let dirs = render::camera_math::orientation_gizmo_dirs(
        &view_proj,
        viewport_size,
        gizmo_rect,
        INDICATOR_ARM_LEN,
    );
    // The arm and letter colors are the 002 semantic axis colors of the
    // theme (ORIGIN_AXIS — X red / Y green / Z blue), bridged into egui at
    // this paint edge.
    let (axis_x, axis_y, axis_z) = theme::ORIGIN_AXIS;
    let colors = [
        theme::to_color32(axis_x),
        theme::to_color32(axis_y),
        theme::to_color32(axis_z),
    ];
    let letters = [texts::AXIS_X, texts::AXIS_Y, texts::AXIS_Z];
    painter.circle_filled(center, INDICATOR_RADIUS, theme::INDICATOR_BACKGROUND);
    // Hairline rim so the neutral disc reads over the dark viewport floor
    // (the base token alone is invisible on it — labels get their contrast
    // from scene content, the corner widget often sits on the floor).
    painter.circle_stroke(
        center,
        INDICATOR_RADIUS,
        egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(36)),
    );
    let letter_font = egui::FontId::proportional(13.0);
    for (i, (dir, visible)) in dirs.iter().enumerate() {
        if !*visible {
            continue;
        }
        // `dir` is a unit vector in egui painter space (y down) — the
        // directions are used as-is, never negated (camera_math.rs: a w≤0
        // flip would mirror the arm onto the axis's negative half).
        let arm = egui::vec2(dir.x, dir.y);
        painter.line_segment(
            [center, center + arm * INDICATOR_ARM_LEN],
            egui::Stroke::new(INDICATOR_ARM_WIDTH, colors[i]),
        );
        painter.text(
            center + arm * INDICATOR_LETTER_RADIUS,
            egui::Align2::CENTER_CENTER,
            letters[i],
            letter_font.clone(),
            colors[i],
        );
    }
}

/// Project one world-space anchor onto the viewport's egui coordinates.
/// [`render::anchor_to_screen`] takes the viewport size in core space and
/// returns top-left-origin points whose axes match the painter's (y down),
/// so only the offset by `rect.min` and the glam→egui conversion remain.
fn anchor_pos(view_proj: &Mat4, rect: egui::Rect, anchor: Vec3) -> Option<egui::Pos2> {
    render::anchor_to_screen(view_proj, Vec2::new(rect.width(), rect.height()), anchor)
        .map(|screen| egui::pos2(rect.min.x + screen.x, rect.min.y + screen.y))
}

/// Paint one overlay label above its anchor point: a translucent rounded
/// backdrop pill with the text on it. The pill keeps the white text
/// readable over bright scene geometry — the simplified contrast treatment
/// of the spec's overlay policy (labels are a viewport overlay, spec §6).
fn paint_label(
    painter: &egui::Painter,
    anchor: egui::Pos2,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
) {
    let galley = painter.layout_no_wrap(text.to_owned(), font.clone(), color);
    let text_size = galley.size();
    // The label floats above the projected anchor: its bottom edge sits
    // `lift` points above the point the anchor marks.
    let lift = 3.0;
    let text_pos = anchor - egui::vec2(text_size.x * 0.5, text_size.y + lift);
    let pill = egui::Rect::from_min_size(text_pos, text_size).expand(3.0);
    painter.rect_filled(pill, 4.0, egui::Color32::from_black_alpha(150));
    painter.galley(text_pos, galley, color);
}

/// Paint the X/Y/Z letters at the tips of a frame's three axis segments
/// (spec §7 F3 / A4), in the axis colors of the segments themselves. The
/// geometry guard mirrors the line pipeline's upload guard: frames with
/// non-finite origins or non-positive lengths draw no geometry and get no
/// letters either.
///
/// Overlay letters of the world-origin axis trio (X red / Y green / Z
/// blue, the 002 semantic colors at the fixed `ORIGIN_AXIS_LENGTH`),
/// projected per frame like the frame labels — so the default view reads
/// its three axes at a glance.
fn paint_origin_axis_labels(
    painter: &egui::Painter,
    rect: egui::Rect,
    state: &Arc<Mutex<ViewportState>>,
) {
    let lock = lock_state(state);
    if !lock.axes_on() {
        return;
    }
    let (view_proj, view_rect) = (
        lock.scene
            .camera
            .view_proj(rect.width() / rect.height().max(1.0)),
        rect,
    );
    drop(lock);
    let tips = [
        (
            Vec3::X * ORIGIN_AXIS_LENGTH,
            texts::AXIS_X,
            theme::to_color32(theme::ORIGIN_AXIS.0),
        ),
        (
            Vec3::Y * ORIGIN_AXIS_LENGTH,
            texts::AXIS_Y,
            theme::to_color32(theme::ORIGIN_AXIS.1),
        ),
        (
            Vec3::Z * ORIGIN_AXIS_LENGTH,
            texts::AXIS_Z,
            theme::to_color32(theme::ORIGIN_AXIS.2),
        ),
    ];
    let axis_font = egui::FontId::proportional(13.0);
    for (tip, letter, color) in tips {
        if let Some(pos) = anchor_pos(&view_proj, view_rect, tip) {
            painter.text(
                pos + egui::vec2(3.0, 0.0),
                egui::Align2::LEFT_CENTER,
                letter,
                axis_font.clone(),
                color,
            );
        }
    }
}

fn paint_frame_axis_labels(
    painter: &egui::Painter,
    view_proj: &Mat4,
    rect: egui::Rect,
    frame: &displays::Frame,
) {
    if !frame.origin.is_finite() || !frame.length.is_finite() || frame.length <= 0.0 {
        return;
    }
    let tips = [
        (
            frame.origin + Vec3::X * frame.length,
            texts::AXIS_X,
            AXIS_X_COLOR,
        ),
        (
            frame.origin + Vec3::Y * frame.length,
            texts::AXIS_Y,
            AXIS_Y_COLOR,
        ),
        (
            frame.origin + Vec3::Z * frame.length,
            texts::AXIS_Z,
            AXIS_Z_COLOR,
        ),
    ];
    let axis_font = egui::FontId::proportional(13.0);
    for (tip, letter, color) in tips {
        if let Some(pos) = anchor_pos(view_proj, rect, tip) {
            // The letter sits just right of its axis tip, vertically
            // centered on the tip.
            painter.text(
                pos + egui::vec2(3.0, 0.0),
                egui::Align2::LEFT_CENTER,
                letter,
                axis_font.clone(),
                color,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roboview_core::render::camera_math::{WorldPlane, pointer_world};
    use roboview_core::render::grid::{GridOptions, GridView, grid_strips, segment_capacity_bound};

    /// A camera pose from the default pose — yaw 0, pitch 0.6, distance
    /// 10 m, the documented constants of scene/camera.rs: absolute yaw and
    /// elevation (radians) with the eye-target distance in meters.
    fn pose(target: Vec3, yaw: f32, pitch: f32, distance: f32) -> OrbitCamera {
        let mut camera = OrbitCamera::new(target);
        camera.orbit(yaw, pitch - 0.6);
        camera.zoom((10.0 / distance).log2());
        camera
    }

    /// Sample `viewport` on a dense pixel grid (plus the four corners) in
    /// egui painter coordinates, y down.
    fn sample_pixels(viewport: Vec2, steps: u32) -> impl Iterator<Item = Vec2> {
        (0..=steps).flat_map(move |i| {
            (0..=steps).map(move |j| {
                Vec2::new(
                    viewport.x * i as f32 / steps as f32,
                    viewport.y * j as f32 / steps as f32,
                )
            })
        })
    }

    #[test]
    fn helper_switches_default_on_and_toggle_independently() {
        let mut state = ViewportState::new();
        assert!(state.grid_on(), "the ground grid defaults to on (spec §6)");
        assert!(state.axes_on(), "the origin axes default to on (spec §6)");
        state.toggle_grid();
        assert!(!state.grid_on());
        assert!(
            state.axes_on(),
            "the axes switch is independent of the grid"
        );
        state.toggle_axes();
        state.toggle_grid();
        assert!(state.grid_on());
        assert!(
            !state.axes_on(),
            "the grid switch is independent of the axes"
        );
        state.toggle_axes();
        assert!(state.grid_on() && state.axes_on());
    }

    #[test]
    fn grid_window_covers_the_visible_ground_and_fits_the_capacity() {
        // A sweep across the navigable pose range — yaw all around, pitch
        // from near-horizon to steep, distances across three orders of
        // magnitude, world-aligned and panned targets — and every viewport
        // shape the app can reach down to the 480×360 minimum window (spec
        // A13). The targets sit on the Z=0 plane, and every pose of the
        // sweep faces it (the eye lands on the side the camera looks toward
        // at any yaw), so the generation window must exist, cover every
        // ground point the pointer math reports, and stay inside the
        // prebuilt mesh capacity.
        let yaws = [0.0_f32, 0.7, 1.8, 3.0, 4.6];
        let pitches = [0.05_f32, 0.3, 0.6, 0.9, 1.15];
        let distances = [0.5_f32, 10.0, 800.0];
        let targets = [Vec3::ZERO, Vec3::new(4.0, 0.0, 0.0)];
        let viewports = [
            Vec2::new(1920.0, 1080.0),
            Vec2::new(800.0, 600.0),
            Vec2::new(480.0, 360.0),
        ];
        let prebuild = segment_capacity_bound(&GridOptions::new(GRID_STEP, GRID_RADIUS_CAP, 2.0));
        let mut windows = 0usize;
        for &target in &targets {
            for &yaw in &yaws {
                for &pitch in &pitches {
                    for &distance in &distances {
                        for &viewport in &viewports {
                            let camera = pose(target, yaw, pitch, distance);
                            let view_proj = camera.view_proj(viewport.x / viewport.y);
                            let window = grid_window(&view_proj, viewport).unwrap_or_else(|| {
                                panic!(
                                    "the ground is visible at every sweep pose \
                                     (yaw {yaw}, pitch {pitch}, distance {distance}, \
                                     target {target:?}, viewport {viewport:?})"
                                )
                            });
                            assert!(window.center.is_finite());
                            assert!(
                                (0.0..=GRID_RADIUS_CAP).contains(&window.radius),
                                "window radius {} outside the capped range",
                                window.radius
                            );
                            windows += 1;
                            // Determinism: an identical pose yields the
                            // identical window — bit-exact, because the
                            // viewport refresh key of ViewportState relies
                            // on this to skip regeneration.
                            assert_eq!(
                                window,
                                grid_window(&view_proj, viewport)
                                    .expect("an identical pose yields the identical window")
                            );
                            // The generated strips fit the prebuilt mesh
                            // capacity and stay inside the window square
                            // (the module clamps every endpoint to the
                            // window before the f32 cast).
                            let view = GridView::new(
                                Vec3::new(window.center.x, window.center.y, 0.0),
                                GridOptions::new(GRID_STEP, window.radius, 2.0),
                            );
                            let strips = grid_strips(&view);
                            assert!(
                                !strips.is_empty(),
                                "a window of radius ≥ 0.5 m always contains a line"
                            );
                            assert!(
                                strips.len() <= prebuild,
                                "{} strips exceed the prebuilt capacity {prebuild}",
                                strips.len()
                            );
                            let eps = window.radius * 1.0e-4 + 1.0e-6;
                            for [a, b] in strips {
                                for p in [a, b] {
                                    assert!(
                                        (p.x - window.center.x).abs() <= window.radius + eps
                                            && (p.y - window.center.y).abs() <= window.radius + eps,
                                        "strip endpoint {p:?} outside the window {window:?}"
                                    );
                                }
                            }
                            // Dense pixel sampling: every ground hit the
                            // pointer math reports lies inside the window
                            // square — the coverage property the grid
                            // generation guarantees.
                            let mut hits = 0usize;
                            for pos in sample_pixels(viewport, 33) {
                                if let Some(hit) =
                                    pointer_world(&view_proj, viewport, pos, WorldPlane::GroundZ0)
                                {
                                    hits += 1;
                                    assert!(
                                        (hit.x - window.center.x).abs() <= window.radius + eps
                                            && (hit.y - window.center.y).abs()
                                                <= window.radius + eps,
                                        "ground hit {hit:?} outside the window {window:?} \
                                         (yaw {yaw}, pitch {pitch}, distance {distance}, \
                                         viewport {viewport:?})"
                                    );
                                }
                            }
                            assert!(
                                hits > 0,
                                "no sampled ground hit for a pose with a generation window \
                                 (yaw {yaw}, pitch {pitch}, distance {distance}, \
                                 viewport {viewport:?})"
                            );
                        }
                    }
                }
            }
        }
        assert_eq!(
            windows,
            targets.len() * yaws.len() * pitches.len() * distances.len() * viewports.len()
        );
    }

    #[test]
    fn grid_window_none_when_the_ground_stays_beyond_the_far_plane() {
        // A distant target viewed from up close, almost level: the far plane
        // sits at 100·distance = 10 m, while the Z=0 plane lies ~90 m in
        // front of the eye. Even the corner rays — whose far end dips below
        // the naive far distance because they travel off-axis to reach the
        // far plane — carry a z-numerator of at most ~2.4
        // (tan(30°)·aspect + 1 + tan(30°) at the 4:3 viewport), so the
        // whole far ring stays ≥ 66 m above the plane: no frustum edge can
        // cross it, no ground is visible. The pointer math agrees — zero
        // hits across the dense sample.
        let camera = pose(Vec3::new(0.0, 0.0, 90.0), 0.0, 0.05, 0.1);
        let view_proj = camera.view_proj(4.0 / 3.0);
        let viewport = Vec2::new(480.0, 360.0);
        assert_eq!(grid_window(&view_proj, viewport), None);
        for pos in sample_pixels(viewport, 32) {
            assert!(
                pointer_world(&view_proj, viewport, pos, WorldPlane::GroundZ0).is_none(),
                "the ground is beyond the far plane: {pos:?} must not hit"
            );
        }
    }

    #[test]
    fn grid_window_handles_degenerate_inputs() {
        let camera = pose(Vec3::ZERO, 0.0, 0.6, 10.0);
        let view_proj = camera.view_proj(16.0 / 9.0);
        // Degenerate viewports.
        assert_eq!(grid_window(&view_proj, Vec2::new(0.0, 0.0)), None);
        assert_eq!(grid_window(&view_proj, Vec2::new(-100.0, 100.0)), None);
        assert_eq!(grid_window(&view_proj, Vec2::new(f32::NAN, 100.0)), None);
        // Degenerate matrices: a non-finite inverse cannot project, and an
        // all-zero matrix has no inverse at all.
        assert_eq!(grid_window(&Mat4::NAN, Vec2::new(1920.0, 1080.0)), None);
        assert_eq!(grid_window(&Mat4::ZERO, Vec2::new(1920.0, 1080.0)), None);
    }

    // — Single-object commit service (004 plan §3.5, T16): headless tests.
    //   No renderer exists here, so the GPU-uniform arms are compile-checked
    //   (and exercised by the integration smoke tests); the CPU session
    //   state — registry rows, the selection mirror, scene fields — is
    //   exactly what runs below. The A6 handle ledger is untestable from
    //   this crate (its counters are `pub(crate)` to roboview-core), so the
    //   A6 stance is structural: the service only re-uploads through the
    //   existing upload arms and never counts anything itself.

    use roboview_core::render::renderer::{APPEARANCE_FLAG_OVERRIDE, APPEARANCE_FLAG_SELECTED};

    /// A fresh viewport whose scene holds one object per kind — frame, text
    /// marker, arrow marker, point cloud, path — in that order; returns the
    /// state and the ids. No renderer: only the CPU session state exists.
    fn scene_state() -> (ViewportState, Vec<u64>) {
        let mut state = ViewportState::new();
        let frame = state.scene.add(
            DisplayObject::Frame(displays::Frame::new(Vec3::new(1.0, 2.0, 3.0), 4.0)),
            "frame",
        );
        let text = state.scene.add(
            DisplayObject::Marker(Marker::text(Vec3::new(5.0, 6.0, 7.0), "label")),
            "text marker",
        );
        let arrow = state.scene.add(
            DisplayObject::Marker(Marker::arrow(Vec3::ZERO, Vec3::X * 2.0)),
            "arrow marker",
        );
        let cloud = state.scene.add(
            DisplayObject::PointCloud(displays::PointCloud::from_data(io::PointCloudData {
                positions: vec![Vec3::ZERO, Vec3::X],
                colors: None,
                bounds: Some(io::Aabb {
                    min: Vec3::ZERO,
                    max: Vec3::X,
                }),
                format: io::Format::Ply,
            })),
            "cloud",
        );
        let path = state.scene.add(
            DisplayObject::Path(displays::Path::from_data(io::PathData {
                points: vec![Vec3::ZERO, Vec3::X],
                bounds: Some(io::Aabb {
                    min: Vec3::ZERO,
                    max: Vec3::X,
                }),
            })),
            "path",
        );
        (state, vec![frame, text, arrow, cloud, path])
    }

    fn frame_of(state: &ViewportState, id: u64) -> &displays::Frame {
        match &state.scene.get(id).expect("object present").object {
            DisplayObject::Frame(frame) => frame,
            other => panic!("expected a frame at id {id}, got {:?}", other.kind()),
        }
    }

    fn text_of(state: &ViewportState, id: u64) -> &displays::MarkerText {
        match &state.scene.get(id).expect("object present").object {
            DisplayObject::Marker(Marker::Text(text)) => text,
            other => panic!("expected a text marker at id {id}, got {:?}", other.kind()),
        }
    }

    fn arrow_of(state: &ViewportState, id: u64) -> &displays::MarkerArrow {
        match &state.scene.get(id).expect("object present").object {
            DisplayObject::Marker(Marker::Arrow(arrow)) => arrow,
            other => panic!(
                "expected an arrow marker at id {id}, got {:?}",
                other.kind()
            ),
        }
    }

    #[test]
    fn appearance_override_registers_clears_and_is_idempotent() {
        let (mut state, ids) = scene_state();
        let frame = ids[0];
        // No renderer: the call must not panic — the uniform write is a
        // deferred no-op, and the registry is the state that replays later.
        let color = io::Color {
            r: 10,
            g: 20,
            b: 30,
        };
        state.appearance_override(frame, color);
        let stored = state.appearance_of(frame).expect("override registered");
        assert_ne!(
            stored.flags & APPEARANCE_FLAG_OVERRIDE,
            0,
            "the registry row carries the override flag"
        );
        assert_eq!(
            stored.flags & APPEARANCE_FLAG_SELECTED,
            0,
            "nothing is selected yet"
        );
        assert_eq!(stored.albedo[0], render::Renderer::srgb_to_linear(10));
        assert_eq!(stored.albedo[1], render::Renderer::srgb_to_linear(20));
        assert_eq!(stored.albedo[2], render::Renderer::srgb_to_linear(30));
        assert_eq!(stored.albedo[3], 1.0, "the override is fully opaque");
        // A repeat submission of the identical color writes nothing new.
        state.appearance_override(frame, color);
        assert_eq!(state.appearances.len(), 1, "one row, not a duplicate");
        // A different color replaces the row in place.
        state.appearance_override(frame, io::Color { r: 1, g: 2, b: 3 });
        assert_eq!(state.appearances.len(), 1);
        assert_eq!(
            state.appearance_of(frame).expect("row").albedo[0],
            render::Renderer::srgb_to_linear(1)
        );
        // Clear restores the no-override state; a repeat clear is a no-op.
        state.clear_appearance_override(frame);
        assert_eq!(state.appearance_of(frame), None);
        state.clear_appearance_override(frame);
        assert_eq!(state.appearance_of(frame), None);
        // Unknown ids are no-ops on both sides.
        state.appearance_override(999, color);
        state.clear_appearance_override(999);
        assert_eq!(state.appearance_of(999), None);
        assert!(state.appearances.is_empty(), "no row for an unknown id");
    }

    #[test]
    fn set_selected_toggles_only_the_affected_flags_and_idles_on_repeats() {
        let (mut state, ids) = scene_state();
        let (a, b) = (ids[0], ids[1]);
        state.appearance_override(a, io::Color { r: 5, g: 6, b: 7 });
        // Select a: its row gains the selection flag.
        state.set_selected(Some(a));
        assert_eq!(state.selected_mirror, Some(a));
        let entry_a = state.appearance_of(a).expect("row for a");
        assert_ne!(entry_a.flags & APPEARANCE_FLAG_SELECTED, 0);
        assert_eq!(state.appearance_of(b), None, "b has no override yet");
        // Repeat of the same selection: the mirror is equal, zero-op.
        state.set_selected(Some(a));
        assert_eq!(state.selected_mirror, Some(a));
        assert_eq!(state.appearances.len(), 1, "nothing new registered");
        // Swap the selection to b: a loses the flag while keeping its
        // override; b — selected without an override — registers no row
        // (a bare selection lives in the uniform only, not the registry).
        state.set_selected(Some(b));
        assert_eq!(state.selected_mirror, Some(b));
        let entry_a = state.appearance_of(a).expect("row for a kept");
        assert_eq!(entry_a.flags & APPEARANCE_FLAG_SELECTED, 0, "flag left a");
        assert_ne!(
            entry_a.flags & APPEARANCE_FLAG_OVERRIDE,
            0,
            "the override on a is kept"
        );
        assert_eq!(
            state.appearance_of(b),
            None,
            "a selection without an override registers nothing"
        );
        // Deselect: the mirror clears and the rows lose the flag.
        state.set_selected(None);
        assert_eq!(state.selected_mirror, None);
        let entry_a = state.appearance_of(a).expect("row for a kept");
        assert_eq!(entry_a.flags & APPEARANCE_FLAG_SELECTED, 0);
        state.set_selected(None);
        assert_eq!(state.selected_mirror, None, "a repeat deselect idles too");
    }

    #[test]
    fn set_selected_normalizes_removed_ids_and_prunes_their_rows() {
        let (mut state, ids) = scene_state();
        let frame = ids[0];
        state.appearance_override(frame, io::Color { r: 1, g: 1, b: 1 });
        state.set_selected(Some(frame));
        // The object leaves the scene through the public scene API — the
        // same path the objects panel's delete action takes, outside this
        // file. The next selection change must not resurrect its state.
        state.scene.remove(frame);
        state.set_selected(Some(frame));
        assert_eq!(
            state.selected_mirror, None,
            "a stale id normalizes to a deselection"
        );
        assert_eq!(
            state.appearance_of(frame),
            None,
            "the row of the removed object is pruned"
        );
        assert!(state.appearances.is_empty());
    }

    #[test]
    fn override_and_clear_preserve_the_selection_flag() {
        let (mut state, ids) = scene_state();
        let frame = ids[0];
        state.set_selected(Some(frame));
        // An override applied while selected keeps the highlight on top.
        state.appearance_override(frame, io::Color { r: 9, g: 8, b: 7 });
        let stored = state.appearance_of(frame).expect("row registered");
        assert_ne!(stored.flags & APPEARANCE_FLAG_SELECTED, 0);
        assert_ne!(stored.flags & APPEARANCE_FLAG_OVERRIDE, 0);
        // Clearing the override keeps the selection mirror untouched.
        state.clear_appearance_override(frame);
        assert_eq!(state.appearance_of(frame), None);
        assert_eq!(state.selected_mirror, Some(frame));
    }

    #[test]
    fn apply_object_edits_updates_cpu_fields_kind_by_kind() {
        let (mut state, ids) = scene_state();
        let (frame, text, arrow, cloud) = (ids[0], ids[1], ids[2], ids[3]);
        // Common rows: rename trims and rejects blanks, visibility sticks.
        state.apply_object_edits(frame, &[ObjectEdit::Rename("  renamed frame  ".into())]);
        assert_eq!(state.scene.get(frame).expect("frame").name, "renamed frame");
        state.apply_object_edits(frame, &[ObjectEdit::Visible(false)]);
        assert!(!state.scene.get(frame).expect("frame").visible);
        state.apply_object_edits(frame, &[ObjectEdit::Visible(true)]);
        assert!(state.scene.get(frame).expect("frame").visible);
        state.apply_object_edits(text, &[ObjectEdit::Rename("T".into())]);
        assert_eq!(state.scene.get(text).expect("text marker").name, "T");
        state.apply_object_edits(text, &[ObjectEdit::Rename("   ".into())]);
        assert_eq!(
            state.scene.get(text).expect("text marker").name,
            "T",
            "a blank rename is a no-op"
        );
        // Frame rows: the geometry edits update the CPU fields; without a
        // renderer the re-upload arm no-ops silently (the next renderer
        // build re-uploads from these very fields).
        state.apply_object_edits(
            frame,
            &[
                ObjectEdit::Origin(Vec3::new(9.0, 9.0, 9.0)),
                ObjectEdit::Length(42.0),
            ],
        );
        assert_eq!(frame_of(&state, frame).origin, Vec3::new(9.0, 9.0, 9.0));
        assert_eq!(frame_of(&state, frame).length, 42.0);
        // Text marker rows: CPU-only, no re-upload (labels hold no GPU data).
        state.apply_object_edits(
            text,
            &[
                ObjectEdit::Anchor(Vec3::X),
                ObjectEdit::Text("hello".into()),
            ],
        );
        assert_eq!(text_of(&state, text).anchor, Vec3::X);
        assert_eq!(text_of(&state, text).text, "hello");
        // Arrow rows.
        state.apply_object_edits(
            arrow,
            &[ObjectEdit::Start(Vec3::ONE), ObjectEdit::End(Vec3::Y * 3.0)],
        );
        assert_eq!(arrow_of(&state, arrow).start, Vec3::ONE);
        assert_eq!(arrow_of(&state, arrow).end, Vec3::Y * 3.0);
        // Kind mismatches no-op: frame rows on an arrow, an arrow row on a
        // text marker, geometry rows on a point cloud (whose color row
        // commits through the appearance channel instead).
        state.apply_object_edits(
            arrow,
            &[ObjectEdit::Origin(Vec3::X), ObjectEdit::Length(1.0)],
        );
        assert_eq!(
            arrow_of(&state, arrow).start,
            Vec3::ONE,
            "frame rows do not touch an arrow"
        );
        state.apply_object_edits(text, &[ObjectEdit::Start(Vec3::X)]);
        assert_eq!(
            text_of(&state, text).anchor,
            Vec3::X,
            "an arrow row does not touch a text marker"
        );
        state.apply_object_edits(
            cloud,
            &[ObjectEdit::Origin(Vec3::X), ObjectEdit::Anchor(Vec3::X)],
        );
        assert_eq!(
            state.scene.get(cloud).expect("cloud").name,
            "cloud",
            "mismatched edits leave the object untouched"
        );
        // The common rows do apply to point clouds.
        state.apply_object_edits(cloud, &[ObjectEdit::Rename("cloud v2".into())]);
        assert_eq!(state.scene.get(cloud).expect("cloud").name, "cloud v2");
    }

    #[test]
    fn apply_object_edits_skips_vanished_and_unknown_objects() {
        let (mut state, ids) = scene_state();
        let frame = ids[0];
        let gone = state.scene.add(
            DisplayObject::Frame(displays::Frame::new(Vec3::ZERO, 1.0)),
            "soon gone",
        );
        state.scene.remove(gone);
        // A batch for a removed id (and for an id that never existed)
        // changes nothing and panics nothing.
        state.apply_object_edits(
            gone,
            &[ObjectEdit::Rename("x".into()), ObjectEdit::Length(5.0)],
        );
        state.apply_object_edits(999, &[ObjectEdit::Visible(false)]);
        assert_eq!(frame_of(&state, frame).length, 4.0);
        assert!(
            state.scene.get(gone).is_none(),
            "the removed object is gone"
        );
    }

    #[test]
    fn group_default_colors_are_kind_scoped_and_fall_back_to_the_unset_marker() {
        let mut state = ViewportState::new();
        assert_eq!(
            state.appearance_default_for_new(DisplayKind::Mesh),
            GROUP_COLOR_UNSET
        );
        let orange = io::Color {
            r: 255,
            g: 128,
            b: 0,
        };
        state.set_group_default_color(DisplayKind::Mesh, orange);
        assert_eq!(state.appearance_default_for_new(DisplayKind::Mesh), orange);
        assert_eq!(
            state.appearance_default_for_new(DisplayKind::PointCloud),
            GROUP_COLOR_UNSET,
            "the kinds are independent"
        );
        assert_eq!(
            state.appearance_default_for_new(DisplayKind::Frame),
            GROUP_COLOR_UNSET
        );
        // The fallback numerically mirrors the objects panel's own unset
        // marker (ui/objects_panel.rs) — the lockstep is by comment, and
        // this assertion pins it against the sibling panel module.
        assert_eq!(
            GROUP_COLOR_UNSET,
            crate::ui::objects_panel::GROUP_COLOR_UNSET
        );
    }

    #[test]
    fn upload_default_appearance_is_plain_off_gpu_and_replays_follow_the_session() {
        let (mut state, ids) = scene_state();
        // Headless, no handle exists for any object: every reachable shape
        // uploads with the plain default (the face-mesh branch — a handle
        // carrying `Faces` — is compile-checked here and exercised by the
        // integration smoke tests; the albedo mirror at the definition site
        // is pinned to the core const by its comment).
        for id in &ids {
            let object = state.scene.get(*id).expect("object present");
            assert_eq!(
                upload_default_appearance(&object.object),
                Appearance::DEFAULT
            );
            // Nothing registered and nothing selected: no replay write.
            assert_eq!(
                session_appearance(&state.appearances, state.selected_mirror, object),
                None,
                "a fresh upload already provisions the default"
            );
        }
        // Selected without an override: the replay composite is the default
        // plus the selection flag — the highlight must not blacken a mesh
        // face, which is why the composite starts from the upload default.
        let frame = ids[0];
        state.set_selected(Some(frame));
        let object = state.scene.get(frame).expect("frame present");
        let composite = session_appearance(&state.appearances, state.selected_mirror, object)
            .expect("a selected object replays");
        assert_ne!(composite.flags & APPEARANCE_FLAG_SELECTED, 0);
        assert_eq!(composite.flags & APPEARANCE_FLAG_OVERRIDE, 0);
        assert_eq!(composite.albedo, Appearance::DEFAULT.albedo);
        // Override registered on top: the registry row wins the replay and
        // carries the synced selection bit.
        state.appearance_override(frame, io::Color { r: 1, g: 2, b: 3 });
        let object = state.scene.get(frame).expect("frame present");
        let composite = session_appearance(&state.appearances, state.selected_mirror, object)
            .expect("an overridden object replays");
        assert_ne!(composite.flags & APPEARANCE_FLAG_OVERRIDE, 0);
        assert_ne!(composite.flags & APPEARANCE_FLAG_SELECTED, 0);
        // Deselecting elsewhere drops the flag from the stored row.
        let text = ids[1];
        state.set_selected(Some(text));
        let object = state.scene.get(frame).expect("frame present");
        let composite = session_appearance(&state.appearances, state.selected_mirror, object)
            .expect("row kept");
        assert_ne!(composite.flags & APPEARANCE_FLAG_OVERRIDE, 0);
        assert_eq!(composite.flags & APPEARANCE_FLAG_SELECTED, 0);
    }
}
