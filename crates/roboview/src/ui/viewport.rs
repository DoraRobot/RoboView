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
//! Frame flow (egui is single-threaded and immediate mode, so `update` and
//! the callbacks of the same frame never race):
//!
//! 1. `update` (the app) draws the viewport panel with [`show_viewport`];
//!    the state records the viewport rect of *this* frame and pointer
//!    input mutates the camera, then — when the scene holds any object —
//!    one [`egui_wgpu::Callback`] is registered inside the frame's shape
//!    list.
//! 2. egui-wgpu later calls `ViewportCallback::prepare` for every such
//!    shape: it locks the state, recomputes the view-projection from the
//!    stored rect of this same frame, and writes the scene's single
//!    uniform (one queue write per frame reaches every pipeline).
//! 3. During the render pass `ViewportCallback::paint` locks the state
//!    again and records the draws of every visible object, grouped into
//!    three passes by pipeline — points first (the depth reference
//!    surface), then mesh faces (pushed away by their depth bias), then
//!    line work (strict Less, no depth writes) — so the draw order keeps
//!    the shared-depth policy of the family and the pipeline switches stay
//!    at three per frame regardless of the object count.
//! 4. The overlay pass paints the viewport labels through the egui
//!    painter on top of the 3D content: the text markers' labels and the
//!    frames' axis letters, projected per frame with
//!    [`render::anchor_to_screen`] (spec §7 F3/F4, A4).
//!
//! The state lives behind an `Arc<Mutex<…>>` because the callback objects
//! must be `Send + Sync` (egui-wgpu requirement) and because wgpu access
//! must stay exclusive; on the UI thread the lock is uncontended. The
//! meshes and the scene data owned by the state make the whole structure
//! `Send`, which also future-proofs offloading uploads to a worker thread.

use std::sync::{Arc, Mutex, MutexGuard};

use eframe::egui;
use egui_wgpu::wgpu;
use glam::{Mat4, Vec2, Vec3};

use roboview_core::displays::{self, DisplayObject, Marker};
use roboview_core::io;
use roboview_core::render;
use roboview_core::scene::Scene;
use roboview_core::scene::camera::OrbitCamera;

use super::camera_input;
use super::texts;

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
        let state = lock_state(&self.state);
        let Some(renderer) = state.renderer.as_ref() else {
            return Vec::new();
        };
        // The shared view-proj uniform is written once per frame in prepare
        // (the only callback stage with a queue). The aspect comes from the
        // viewport rect that `show_viewport` recorded for this same frame
        // (update runs before the callbacks, so the rect is never stale).
        let view_proj = state.scene.camera.view_proj(state.aspect());
        renderer.update_uniform(queue, view_proj);
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
            if let DisplayObject::Mesh(mesh) = &object.object {
                if let Some(render::MeshGpu::Faces(faces)) = mesh.gpu.as_ref() {
                    mesh_pipeline.paint(render_pass, faces);
                }
            }
        }
        for object in state.scene.iter_visible() {
            match &object.object {
                // Pass 3 — line geometry: paths, frames, and marker arrows,
                // drawn by the line pipeline. Text markers hold no GPU data
                // and are painted by the overlay pass instead.
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
pub fn show_viewport(ui: &mut egui::Ui, state: &Arc<Mutex<ViewportState>>, loading: bool) {
    let (rect, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::drag());

    let has_content = {
        let mut viewport = lock_state(state);
        // The paint callback and the overlay pass of this frame read this
        // rect (aspect, label placement).
        viewport.viewport_rect = rect;
        let has_content = !viewport.scene.is_empty();
        if has_content {
            // Orbit/pan/zoom only make sense once there is content; the
            // camera pose is re-framed when the first object loads (spec
            // §6) and kept afterwards.
            camera_input::apply_pointer_events(
                &response,
                ui.ctx(),
                rect,
                &mut viewport.scene.camera,
            );
        }
        has_content
    };

    if !rect.is_finite() || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }

    let painter = ui.painter_at(rect);

    if has_content {
        // wgpu draws the scene itself; register the paint callback for it.
        let callback = ViewportCallback {
            state: Arc::clone(state),
        };
        painter.add(egui_wgpu::Callback::new_paint_callback(rect, callback));

        // Overlay pass: marker text labels and frame axis letters, painted
        // after the 3D callback so they always sit on top (spec §6).
        paint_labels(&painter, rect, state);

        if loading {
            // Subtle in-viewport hint while the scene stays interactive.
            let corner = egui::pos2(rect.left() + 12.0, rect.bottom() - 12.0);
            painter.text(
                corner,
                egui::Align2::LEFT_BOTTOM,
                texts::VIEWPORT_LOADING,
                egui::FontId::proportional(13.0),
                ui.visuals().weak_text_color(),
            );
        }
    } else if loading {
        // No content yet: a spinner with the loading label, centered.
        let center = rect.center();
        let spinner_rect =
            egui::Rect::from_center_size(center - egui::vec2(0.0, 16.0), egui::vec2(28.0, 28.0));
        ui.put(spinner_rect, egui::Spinner::new().size(28.0));
        painter.text(
            center + egui::vec2(0.0, 26.0),
            egui::Align2::CENTER_CENTER,
            texts::VIEWPORT_LOADING,
            egui::FontId::proportional(16.0),
            ui.visuals().weak_text_color(),
        );
    } else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            texts::VIEWPORT_EMPTY_HINT,
            egui::FontId::proportional(18.0),
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
