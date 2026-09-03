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
//! and the LOD ladder keeps a minimum spacing — so no alpha blending is
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

use std::sync::{Arc, Mutex, MutexGuard};

use eframe::egui;
use egui_wgpu::wgpu;
use glam::{Mat4, Vec2, Vec3, Vec4};

use roboview_core::displays::{self, DisplayObject, Marker};
use roboview_core::io;
use roboview_core::render;
use roboview_core::scene::Scene;
use roboview_core::scene::camera::OrbitCamera;

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

/// Minor step (m) of the ground grid — the app's fixed pair, mirroring the
/// grid module's defaults (render/grid.rs), whose LOD ladder is calibrated
/// to exactly these values (spec §6: minor 0.2 m, major 1 m).
const GRID_MINOR_STEP: f32 = 0.2;
const GRID_MAJOR_STEP: f32 = 1.0;
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
/// "示意长度固定" — one major grid cell). World-fixed geometry never needs
/// a camera-driven refresh, unlike the windowed grid.
const ORIGIN_AXIS_LENGTH: f32 = 1.0;
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

/// Acquire the viewport state lock, recovering from poisoning: a poisoned
/// mutex still holds the state (the panicking thread unwound before any
/// invariant broke), so `into_inner` is safe here.
pub fn lock_state(state: &Arc<Mutex<ViewportState>>) -> MutexGuard<'_, ViewportState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
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
    /// View-projection of the last grid refresh — the refresh key. A
    /// bitwise-equal `view_proj` means the identical window and strips, so
    /// nothing regenerates while the camera sits still (A11: no crawl, no
    /// per-frame rework).
    grid_refresh_key: Option<Mat4>,
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
            grid_on: true,
            axes_on: true,
            grid_mesh: None,
            axes_meshes: None,
            grid_refresh_key: None,
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
        // and are skipped by the upload dispatch.
        for object in self.scene.iter_mut() {
            upload_display(
                &mut renderer,
                &mesh_pipeline,
                &line_pipeline,
                &mut object.object,
            );
        }
        // The helper meshes' bind groups reference the old renderer's
        // layout and uniform buffer (`with_capacity` builds them from the
        // pipeline), so a rebuild drops them; the next prepare
        // re-provisions and refreshes them lazily.
        self.grid_mesh = None;
        self.axes_meshes = None;
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
    /// the scene holds.
    pub fn add_frame(&mut self, origin: Vec3, length: f32) {
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
        self.scene.add(DisplayObject::Frame(frame), name);
    }

    /// Add a UI-built marker (spec §7 F4): arrows are uploaded through the
    /// line pipeline, text labels hold no GPU data (they are painted by the
    /// overlay pass). Appended under a generated name (`Marker N`); UI
    /// adds never move the camera.
    pub fn add_marker(&mut self, mut marker: displays::Marker) {
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
        self.scene.add(DisplayObject::Marker(marker), name);
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
        let Some(line_pipeline) = self.line_pipeline.as_ref() else {
            return;
        };
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
        if !self.grid_on {
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
            let options =
                render::grid::GridOptions::new(GRID_MINOR_STEP, GRID_MAJOR_STEP, GRID_RADIUS_CAP);
            let mesh = line_pipeline.with_capacity(render::grid::segment_capacity_bound(&options));
            self.grid_mesh = Some(mesh);
        }
        let Some(window) = grid_window(&view_proj, self.viewport_size()) else {
            return;
        };
        let view = render::grid::GridView::new(
            Vec3::new(window.center.x, window.center.y, 0.0),
            render::grid::GridOptions::new(GRID_MINOR_STEP, GRID_MAJOR_STEP, window.radius),
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

        // Overlay pass: marker text labels, frame axis letters, and the
        // corner orientation indicator, painted after the 3D callback so
        // they always sit on top (spec §6).
        paint_labels(&painter, rect, state);
        paint_indicator(&painter, rect, state);
    }

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
        } else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                texts::viewport_empty_hint(locale),
                egui::FontId::proportional(18.0),
                ui.visuals().weak_text_color(),
            );
        }
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
        let prebuild = segment_capacity_bound(&GridOptions::new(
            GRID_MINOR_STEP,
            GRID_MAJOR_STEP,
            GRID_RADIUS_CAP,
        ));
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
                                GridOptions::new(GRID_MINOR_STEP, GRID_MAJOR_STEP, window.radius),
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
}
