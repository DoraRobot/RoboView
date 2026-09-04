//! GPU rendering core: shaders, pipelines, overlay projection, and the
//! render-handle ledger.
//!
//! # Shared-depth rendering contract (display-types plan §3.3 / spec §6)
//!
//! wgpu-core's `check_compatible` requires the render pass the host opens
//! and every pipeline that records into it to agree exactly on the depth
//! format and the sample count. [`renderer::Renderer`] therefore receives
//! both from the host — `Renderer::new(device, queue, target_format,
//! depth_format, sample_count)` — and keeps them as the single source for
//! the whole scene, together with the target format, the scene-wide bind
//! group layout, and the single view-projection uniform buffer. The host
//! rebuilds the renderer (and re-uploads the meshes) when any format or the
//! sample count changes, exactly as for a target format change.
//!
//! The scene shares one view-projection uniform: a single 64-byte buffer
//! with one bind group layout (`@group(0) @binding(0) view_proj:
//! mat4x4<f32>`) that every mesh's bind group and every pipeline of the
//! family references. The host writes it once per frame in its prepare
//! stage via [`renderer::Renderer::update_uniform`] — geometry is in world
//! coordinates and there are no per-object transforms this stage, so one
//! write per frame suffices for any number of objects.
//!
//! # Pipeline family (display-types plan §5 P3/P4)
//!
//! The scene pipelines join the family under these rules, which this module
//! is the agreed home of:
//!
//! - every pipeline is built from a [`renderer::Renderer`] — the family
//!   constructors ([`mesh::MeshPipeline::new`], [`line::LinePipeline::new`])
//!   read the renderer's `depth_format`/`sample_count`/`target_format`
//!   accessors and reuse its bind group layout and uniform buffer verbatim —
//!   never per-pipeline format values, because pipeline and pass must match
//!   exactly;
//! - every pipeline shares the one bind group layout above (binding 0 =
//!   the view-proj uniform buffer); its WGSL declares exactly that binding
//!   and nothing else in group 0;
//! - depth policy per geometry class: the point pipeline writes depth with
//!   a strict `Less` compare and zero bias (the reference surface); the
//!   mesh pipeline writes depth with strict `Less` plus a
//!   [`wgpu::DepthBiasState`] polygon offset that pushes the mesh away so
//!   surface-hugging clouds and lines stay visible (constant table in
//!   mesh.rs, the M3 calibration entry point); line primitives (paths,
//!   frames, marker arrows) use strict `Less` and never write depth —
//!   polygon offsets do not apply to line primitives in any case.
//!
//! Per-kind render-handle counting for acceptance A6 lives in
//! [`counters`], and the pure projection the app's overlay labels are
//! painted through is [`anchor_to_screen`] ([`camera_math`]).
//!
//! # Uploads and mesh ownership
//!
//! The display types hold only CPU data plus an optional handle to uploaded
//! geometry ([`renderer::PointCloudMesh`], [`mesh::MeshGpu`],
//! [`line::LineMesh`]); upload happens once per data replacement through
//! the pipeline objects, and dropping a display drops its handle, freeing
//! the buffers through wgpu's deferred destruction semantics.

pub mod camera_math;
pub mod counters;
pub mod grid;
pub mod line;
pub mod mesh;
pub mod pick;
pub mod renderer;

pub use camera_math::anchor_to_screen;
pub use line::{LineMesh, LinePipeline};
pub use mesh::{MeshGpu, MeshMesh, MeshPipeline};
pub use renderer::{PointCloudMesh, Renderer};
