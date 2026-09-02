//! 3D viewport: the shared scene/render state and its per-frame wgpu
//! paint callback.
//!
//! The scene is a container of point-cloud objects appended in open order
//! (display-types spec §1 replaces the first feature's single-slot swap
//! with append; plan §3.1, §3.4): each successful load adds one object and
//! never touches the existing ones. The camera follows the spec §6 ruling —
//! the first object added to an empty scene frames the whole scene, later
//! adds never move the camera.
//!
//! Rendering is in an interim state while the renderer still draws a
//! single object per frame: the callback paints only the most recently
//! added object, so opening a second file replaces the *picture* while the
//! scene keeps both objects (append semantics live in the data layer
//! already). Drawing every visible object lands with the multi-object
//! rendering of later display-types stages (plan §5 P2–P3); until then
//! nothing else consumes the older objects' GPU handles except rebuilds
//! (see [`ViewportState::sync_renderer`]).
//!
//! Frame flow (egui is single-threaded and immediate mode, so `update` and
//! the callbacks of the same frame never race):
//!
//! 1. `update` (the app) draws the viewport panel with
//!    [`show_viewport`]; the state records the viewport rect of *this*
//!    frame and pointer input mutates the camera, then — when an object is
//!    present — one [`egui_wgpu::Callback`] is registered inside the
//!    frame's shape list.
//! 2. egui-wgpu later calls `ViewportCallback::prepare` for every such
//!    shape: it locks the state, recomputes the view-projection from the
//!    stored rect of this same frame, and writes the mesh uniform.
//! 3. During the render pass `ViewportCallback::paint` locks the state
//!    again and records the draw of the last added object.
//!
//! The state lives behind an `Arc<Mutex<…>>` because the callback objects
//! must be `Send + Sync` (egui-wgpu requirement) and because wgpu access
//! must stay exclusive; on the UI thread the lock is uncontended. The
//! meshes and the scene data owned by the state make the whole structure
//! `Send`, which also future-proofs offloading uploads to a worker thread.

use std::sync::{Arc, Mutex, MutexGuard};

use eframe::egui;
use egui_wgpu::wgpu;

use roboview_core::displays;
use roboview_core::io;
use roboview_core::render;
use roboview_core::scene::Scene;
use roboview_core::scene::camera::OrbitCamera;

use super::camera_input;
use super::texts;

/// Acquire the viewport state lock, recovering from poisoning: a poisoned
/// mutex still holds the state (the panicking thread unwound before any
/// invariant broke), so `into_inner` is safe here.
pub fn lock_state(state: &Arc<Mutex<ViewportState>>) -> MutexGuard<'_, ViewportState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Shared state of the 3D viewport: the scene (camera + the appended point
/// cloud objects), the point cloud renderer, and the per-frame viewport
/// rect.
pub struct ViewportState {
    /// Scene: camera plus one object per successfully loaded file, in open
    /// order (display-types plan §3.1).
    scene: Scene<displays::PointCloud>,
    /// Renderer for the current target format; `None` before the first
    /// frame in which eframe exposes its wgpu `RenderState`
    /// (egui_wgpu::RenderState), i.e. before any GPU work is possible.
    renderer: Option<render::Renderer>,
    /// Viewport rect of the frame currently being built, in points.
    /// `show_viewport` records it while drawing; the paint callback of the
    /// same frame reads it as the aspect-ratio source. Rect proportions
    /// are scale-invariant, so using the point-space rect equals using the
    /// physical-pixel rect of the callback info.
    viewport_rect: egui::Rect,
}

impl ViewportState {
    /// Create an empty viewport: no objects, no renderer, a default camera
    /// that the first successful load replaces with a framing pose.
    pub fn new() -> Self {
        Self {
            scene: Scene::new(OrbitCamera::framing(None)),
            renderer: None,
            viewport_rect: egui::Rect::NOTHING,
        }
    }

    /// Align the renderer with eframe's current wgpu render state.
    ///
    /// The first call (once `frame.wgpu_render_state()` is available)
    /// creates the renderer; any later call whose target format differs —
    /// the window moved across screens with different surface capabilities
    /// — rebuilds it and re-uploads every object, because mesh bind groups
    /// reference the old renderer's layout. Call once per frame; when
    /// nothing changed this is a cheap format comparison.
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
        tracing::info!(?target_format, "building point cloud renderer");
        let mut renderer = render::Renderer::new(
            Arc::new(device.clone()),
            Arc::new(queue.clone()),
            target_format,
            // egui-wgpu attaches Depth24Plus when depth_buffer=24 is set in
            // NativeOptions; pipeline and pass must agree exactly.
            wgpu::TextureFormat::Depth24Plus,
            1,
        );
        // Re-upload every object so each GPU handle comes from the rebuilt
        // renderer (and from the same device if eframe ever switches
        // adapters). Hidden objects upload too: visibility only skips
        // drawing, never releases resources.
        for object in self.scene.iter_mut() {
            object.object.mesh = Some(renderer.upload(&object.object.data));
        }
        self.renderer = Some(renderer);
    }

    /// Upload `data` and append it to the scene as a new object named
    /// `name` (the file stem), per the spec §1 replacement declaration.
    ///
    /// The camera moves only when the scene was empty (display-types spec
    /// §6): the first object frames the union of the scene bounds; later
    /// adds keep the current view.
    ///
    /// Returns `false` (leaving `data` untouched) when no renderer exists
    /// yet; the caller retries on a later frame. The one
    /// [`io::PointCloudData`] clone here happens only on this success
    /// path, once per load.
    pub fn install_cloud(&mut self, data: &io::PointCloudData, name: &str) -> bool {
        let Some(renderer) = self.renderer.as_mut() else {
            return false;
        };
        let mesh = renderer.upload(data);
        let mut cloud = displays::PointCloud::from_data(data.clone());
        cloud.mesh = Some(mesh);
        let scene_was_empty = self.scene.is_empty();
        self.scene.add(cloud, name);
        if scene_was_empty {
            self.scene.camera = OrbitCamera::framing(self.scene.bounds_union().as_ref());
        }
        true
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

/// egui-wgpu paint callback drawing the most recently added object of a
/// [`ViewportState`].
///
/// Registered per frame by [`show_viewport`] through
/// `egui_wgpu::Callback::new_paint_callback`; see the module docs for the
/// prepare/paint flow and for why only the last added object is drawn
/// while rendering is single-object.
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
        // before painting and restores it afterwards, so drawing the full
        // cloud here never leaks outside the viewport.
        let state = lock_state(&self.state);
        let Some(renderer) = state.renderer.as_ref() else {
            return;
        };
        let Some(object) = state.scene.last() else {
            return;
        };
        if let Some(mesh) = object.object.mesh.as_ref() {
            renderer.paint(render_pass, mesh);
        }
    }
}

/// Draw the central 3D viewport into `ui`: allocate the remaining space,
/// feed pointer input to the camera, and register this frame's paint
/// callback — or the empty-state/loading placeholders.
pub fn show_viewport(ui: &mut egui::Ui, state: &Arc<Mutex<ViewportState>>, loading: bool) {
    let (rect, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::drag());

    let has_content = {
        let mut viewport = lock_state(state);
        // The paint callback of this frame reads this rect (aspect).
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
        // wgpu draws the cloud itself; register the paint callback for it.
        let callback = ViewportCallback {
            state: Arc::clone(state),
        };
        painter.add(egui_wgpu::Callback::new_paint_callback(rect, callback));

        if loading {
            // Subtle in-viewport hint while the cloud stays interactive.
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
