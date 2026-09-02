//! GPU rendering core: camera, shaders, and pipelines.
//!
//! # Shared-depth rendering contract (display-types plan §3.3 / spec §6)
//!
//! wgpu-core's `check_compatible` requires the render pass the host opens
//! and every pipeline that records into it to agree exactly on the depth
//! format and the sample count. [`renderer::Renderer`] therefore receives
//! both from the host — `Renderer::new(device, queue, target_format,
//! depth_format, sample_count)` — and keeps them as the single source for
//! the whole scene: the point cloud pipeline today, and every pipeline of
//! the family that later display-type stages add (mesh, lines). The host
//! rebuilds the renderer (and re-uploads the meshes) when any of the three
//! formats or the sample count changes, exactly as for a target format
//! change.
//!
//! The scene shares one view-projection uniform: a single 64-byte buffer
//! with one bind group layout (`@group(0) @binding(0) view_proj:
//! mat4x4<f32>`) that every mesh's bind group and every future pipeline
//! references. The host writes it once per frame in its prepare stage via
//! [`renderer::Renderer::update_uniform`] — geometry is in world
//! coordinates and there are no per-object transforms this stage, so one
//! write per frame suffices for any number of objects.
//!
//! # Pipeline family extension rules (later display-type tasks)
//!
//! Subsequent scene pipelines (mesh, line) join the family under these
//! rules, which this module is the agreed home of:
//!
//! - every pipeline is built with the renderer's `depth_format` and
//!   `sample_count` fields — never per-pipeline values — because pipeline
//!   and pass must match exactly;
//! - every pipeline shares the one bind group layout above (binding 0 =
//!   the view-proj uniform buffer); its WGSL declares exactly that binding
//!   and nothing else in group 0;
//! - depth policy per geometry class: the point pipeline writes depth with
//!   a strict `Less` compare and zero bias (the reference surface); the
//!   future mesh pipeline adds a [`wgpu::DepthBiasState`] polygon offset
//!   that pushes the mesh away so surface-hugging clouds and lines stay
//!   visible; line primitives (paths, coordinate axes) are not affected by
//!   polygon offset and use strict `Less` like points.
//!
//! Mesh and line pipelines themselves are implemented by their own display
//! type tasks (display-types plan §5, P3).

pub mod renderer;

pub use renderer::{PointCloudMesh, Renderer};
