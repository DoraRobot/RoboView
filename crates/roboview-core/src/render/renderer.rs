//! Point cloud rendering: pipeline ownership, buffer upload, and draw calls.
//!
//! The renderer owns everything GPU-side that the display types do not: the
//! point cloud pipeline, and the scene-wide bind group layout plus its
//! single view-projection uniform buffer shared by every uploaded mesh and
//! by the later pipelines of the scene (extension rules in `render/mod.rs`).
//! It never creates an instance, adapter, device, queue, surface, or
//! swapchain — all wgpu objects come from the [`wgpu::Device`] and
//! [`wgpu::Queue`] injected by the host (egui-wgpu, see the rendering
//! contract in `docs/specs/display-types/plan.md` §3.3). Drawing happens
//! inside an externally opened render pass: this module records commands
//! only and never starts, ends, or submits a pass or encoder.

use std::borrow::Cow;
use std::sync::Arc;

use super::counters;
use crate::displays::DisplayKind;
use crate::io;

/// Embedded WGSL source of the point cloud pipeline. Compiled headlessly
/// against naga in the unit tests; the same naga major version validates it
/// again inside wgpu when the pipeline is created.
const POINT_CLOUD_SHADER_SOURCE: &str = include_str!("../../assets/shaders/point_cloud.wgsl");

/// Default point color as sRGB 8-bit bytes with opaque alpha, used when the
/// data has no per-point colors. Derived from the sRGB floats (0.8, 0.8, 0.9):
/// 0.8 * 255 = 204 and 0.9 * 255 = 229.5 rounds to 229.
const DEFAULT_POINT_COLOR_SRGB: [u8; 4] = [204, 204, 229, 255];

/// Byte stride of one position vertex: x, y, z as three `f32`. Shared with
/// the mesh and line pipelines of the display-type family (crate-internal).
pub(crate) const POSITION_STRIDE_BYTES: u64 = 12;

/// Byte stride of one color vertex: a single packed Rgba8Unorm texel.
/// Shared with the line pipeline (crate-internal).
pub(crate) const COLOR_STRIDE_BYTES: u64 = 4;

/// Byte size of the scene's single view-projection uniform buffer holding
/// one `mat4x4<f32>`.
const UNIFORM_SIZE_BYTES: u64 = 64;

/// Byte size of one object's appearance uniform. Fixed at 64 bytes
/// (ui-blueprint spec §6, plan §3.1): albedo 16 + flags 4 + 12 implicit
/// padding + 32 reserved bytes. Every uploaded object provisions one such
/// buffer at group(1) binding(0); the WGSL `ObjectAppearance` struct of the
/// three shaders and the CPU packer are pinned to this layout by tests.
pub const APPEARANCE_SIZE_BYTES: u64 = 64;

/// Appearance flag bit 0: replace the baked per-vertex colors with
/// [`Appearance::albedo`]. Points and lines carry per-vertex colors, so
/// this bit is their override switch; mesh faces have no per-vertex color
/// and always take their color from the uniform, so the mesh shader does
/// not read this bit. WGSL mirror: `APPEARANCE_FLAG_OVERRIDE` in all three
/// shaders (pinned by a unit test).
pub const APPEARANCE_FLAG_OVERRIDE: u32 = 1 << 0;

/// Appearance flag bit 1: selection highlight — the drawn color is
/// multiplied by 1.25 in linear light and clamped. ui-blueprint spec §6:
/// the selection marker of 004 rides this bit, and 005 picking reuses the
/// same channel by setting (and clearing) marker bits. WGSL mirror:
/// `APPEARANCE_FLAG_SELECTED` in all three shaders (pinned by a test).
pub const APPEARANCE_FLAG_SELECTED: u32 = 1 << 1;

/// Per-object appearance channel (ui-blueprint spec §6 "视口高亮机制",
/// plan §3.1): the group(1)/binding(0) override color and marker flags
/// every uploaded object carries — one fixed 64-byte uniform buffer plus
/// one bind group per object, provisioned together with the geometry
/// handles and dropped with them (see [`AppearanceGpu`]).
///
/// Colors are linear light RGBA, the same space the pipelines' fragment
/// stages write to an sRGB target in (per-vertex sRGB colors are converted
/// to linear in the vertex stage before this channel mixes with them). For
/// mesh faces the albedo *is* the flat face color (the former WGSL
/// `FACE_COLOR` constant); for points and lines it replaces the per-vertex
/// colors only while [`APPEARANCE_FLAG_OVERRIDE`] is set.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Appearance {
    /// Linear-light RGBA override color (see the type docs for when it
    /// applies). Callers converting sRGB palette tokens to this space use
    /// [`Renderer::srgb_to_linear`] per channel.
    pub albedo: [f32; 4],
    /// Marker flags: [`APPEARANCE_FLAG_OVERRIDE`], [`APPEARANCE_FLAG_SELECTED`].
    pub flags: u32,
}

impl Appearance {
    /// Neutral appearance: per-vertex colors stay untouched (points,
    /// lines) and no marker is set. Mesh uploads provision their own
    /// default albedo on top of this (mesh.rs).
    pub const DEFAULT: Self = Self {
        albedo: [0.0, 0.0, 0.0, 1.0],
        flags: 0,
    };

    /// Builds an appearance from raw fields.
    pub const fn new(albedo: [f32; 4], flags: u32) -> Self {
        Self { albedo, flags }
    }

    /// A color override from sRGB 8-bit bytes (converted to linear light)
    /// with [`APPEARANCE_FLAG_OVERRIDE`] set — the palette-token entry
    /// point for point/line coloring.
    pub fn srgb_override(color: io::Color) -> Self {
        Self::new(
            [
                Renderer::srgb_to_linear(color.r),
                Renderer::srgb_to_linear(color.g),
                Renderer::srgb_to_linear(color.b),
                1.0,
            ],
            APPEARANCE_FLAG_OVERRIDE,
        )
    }

    /// This appearance with the selection marker set or cleared
    /// ([`APPEARANCE_FLAG_SELECTED`]) — the 004 selection / 005 picking
    /// update path: one in-place queue write, nothing rebuilt.
    pub const fn with_selected(&self, selected: bool) -> Self {
        let flags = if selected {
            self.flags | APPEARANCE_FLAG_SELECTED
        } else {
            self.flags & !APPEARANCE_FLAG_SELECTED
        };
        Self {
            albedo: self.albedo,
            flags,
        }
    }
}

/// Serialize one [`Appearance`] into the fixed 64-byte layout the WGSL
/// `ObjectAppearance` uniform struct declares: `albedo` at offset 0 as
/// four little-endian `f32`, `flags` at offset 16 as a little-endian `u32`,
/// all remaining bytes (padding and the reserved region) zero. A unit test
/// pins the shader-side member offsets against this packer.
fn pack_appearance(appearance: &Appearance) -> [u8; APPEARANCE_SIZE_BYTES as usize] {
    let mut bytes = [0u8; APPEARANCE_SIZE_BYTES as usize];
    for (index, channel) in appearance.albedo.iter().enumerate() {
        let start = index * 4;
        bytes[start..start + 4].copy_from_slice(&channel.to_le_bytes());
    }
    bytes[16..20].copy_from_slice(&appearance.flags.to_le_bytes());
    bytes
}

/// Entries of the per-object appearance bind group layout shared by every
/// family pipeline: exactly one — binding 0, the 64-byte appearance uniform
/// (`@group(1) @binding(0) var<uniform> appearance: ObjectAppearance` in
/// WGSL). Visible to both shader stages: today the fragment stage mixes the
/// channel in, and the layout leaves the vertex stage available to later
/// work (005 and beyond) without relayout.
fn appearance_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 1] {
    [wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }]
}

/// Vertex attribute of the position buffer (vertex slot 0): `x y z` at
/// shader location 0. Shared with the mesh and line pipelines
/// (crate-internal).
pub(crate) const POSITION_ATTRIBUTES: [wgpu::VertexAttribute; 1] = [wgpu::VertexAttribute {
    format: wgpu::VertexFormat::Float32x3,
    offset: 0,
    shader_location: 0,
}];

/// Vertex attribute of the color buffer (vertex slot 1): one packed
/// Rgba8Unorm texel at shader location 1. The hardware unorm-decodes the
/// four bytes into [0, 1] floats; the shader therefore declares the input
/// as `vec4<f32>` and converts sRGB to linear itself (wgpu-core maps
/// Unorm8x4 to a float vector: "the shader always sees data as float").
/// Shared with the line pipeline (crate-internal).
pub(crate) const COLOR_ATTRIBUTES: [wgpu::VertexAttribute; 1] = [wgpu::VertexAttribute {
    format: wgpu::VertexFormat::Unorm8x4,
    offset: 0,
    shader_location: 1,
}];

/// Serialize positions into tightly packed little-endian `f32` triples
/// (GPU vertex data is little-endian on every supported backend). Shared
/// with the scatter upload of the mesh pipeline (crate-internal).
pub(crate) fn pack_positions(positions: &[glam::Vec3]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(positions.len() * POSITION_STRIDE_BYTES as usize);
    for point in positions {
        bytes.extend_from_slice(&point.x.to_le_bytes());
        bytes.extend_from_slice(&point.y.to_le_bytes());
        bytes.extend_from_slice(&point.z.to_le_bytes());
    }
    bytes
}

/// Serialize colors into one packed Rgba8Unorm (4 bytes) per point. File
/// colors are 3 sRGB bytes, padded with an opaque alpha; when the data has
/// no colors, every point gets [`DEFAULT_POINT_COLOR_SRGB`]. Shared with the
/// scatter upload of the mesh pipeline (crate-internal).
pub(crate) fn pack_colors(count: usize, colors: Option<&[io::Color]>) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(count * COLOR_STRIDE_BYTES as usize);
    match colors {
        Some(colors) => {
            // Invariant of `io::PointCloudData`: colors are same-length as
            // positions when present (io/mod.rs).
            debug_assert_eq!(colors.len(), count);
            for color in colors {
                bytes.extend_from_slice(&[color.r, color.g, color.b, 255]);
            }
        }
        None => {
            for _ in 0..count {
                bytes.extend_from_slice(&DEFAULT_POINT_COLOR_SRGB);
            }
        }
    }
    bytes
}

/// Serialize the view-projection matrix into the byte layout a WGSL
/// `mat4x4<f32>` uniform requires: column-major little-endian `f32`, 64
/// bytes total — exactly what the shared uniform buffer holds.
fn pack_view_proj(view_proj: glam::Mat4) -> [u8; 64] {
    // glam stores matrices column-wise, and WGSL `mat4x4<f32>` uniform
    // memory layout is column-major, so the column array maps directly.
    bytemuck::cast(view_proj.to_cols_array())
}

/// GPU side of the per-object appearance channel, provisioned together with
/// the geometry handles by every upload path (renderer.rs, mesh.rs, line.rs)
/// and stored as fields of the same mesh struct ([`PointCloudMesh`],
/// [`crate::render::mesh::MeshMesh`], [`crate::render::line::LineMesh`]):
/// the channel lives and dies with its mesh handle, so the A6 ledger
/// semantics of the geometry handle transfer to it unchanged and the
/// counters module gains no row (plan §3.1 "uniform 与几何句柄同生共死",
/// §5).
pub(crate) struct AppearanceGpu {
    /// The object's fixed 64-byte uniform buffer holding one [`Appearance`].
    pub uniform_buffer: wgpu::Buffer,
    /// Bind group referencing `uniform_buffer` at group(1) binding(0),
    /// created against the renderer's shared appearance bind group layout.
    pub bind_group: wgpu::BindGroup,
}

/// Provision one object's appearance uniform buffer plus its group(1) bind
/// group, initialized to `appearance`. `label` names both resources (the
/// scene convention: uploads label resources after their kind, e.g.
/// "line.appearance"). Queue-writes the initial 64 bytes; afterwards only
/// [`write_appearance`] ever touches the buffer.
pub(crate) fn create_appearance_gpu(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    label: &'static str,
    appearance: &Appearance,
) -> AppearanceGpu {
    let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: APPEARANCE_SIZE_BYTES,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&uniform_buffer, 0, &pack_appearance(appearance));
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buffer.as_entire_binding(),
        }],
    });
    AppearanceGpu {
        uniform_buffer,
        bind_group,
    }
}

/// In-place refresh of one object's appearance uniform: a single 64-byte
/// queue write into the preallocated buffer. Never creates a buffer or
/// bind group and never touches a pipeline — appearance changes are object
/// data, not a rebuild trigger (plan §3.1).
pub(crate) fn write_appearance(queue: &wgpu::Queue, gpu: &AppearanceGpu, appearance: &Appearance) {
    queue.write_buffer(&gpu.uniform_buffer, 0, &pack_appearance(appearance));
}

/// Entries of the scene-wide bind group layout shared by every pipeline and
/// every mesh of the scene: exactly one — binding 0, the view-projection
/// uniform (`@group(0) @binding(0) view_proj: mat4x4<f32>` in WGSL).
fn scene_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 1] {
    [wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::VERTEX,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }]
}

/// The point pipeline's vertex buffer layouts, in slot order: slot 0
/// positions (tightly packed `f32` triples), slot 1 colors (packed
/// Rgba8Unorm). The slot index is the `set_vertex_buffer` slot used in
/// [`Renderer::paint`].
fn point_vertex_buffer_layouts() -> [wgpu::VertexBufferLayout<'static>; 2] {
    [
        wgpu::VertexBufferLayout {
            array_stride: POSITION_STRIDE_BYTES,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &POSITION_ATTRIBUTES,
        },
        wgpu::VertexBufferLayout {
            array_stride: COLOR_STRIDE_BYTES,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &COLOR_ATTRIBUTES,
        },
    ]
}

/// GPU handles of one uploaded point cloud: its two vertex buffers, the
/// bind group that references the renderer's scene-wide view-projection
/// uniform buffer, and the per-object appearance channel ([`AppearanceGpu`]).
///
/// The uniform data itself is not per-mesh: [`Renderer`] owns the one
/// buffer and rewrites it once per frame through [`Renderer::update_uniform`],
/// so uploading or dropping a cloud never touches the matrix every mesh
/// sees. The appearance uniform, by contrast, *is* per-mesh — the two
/// appearance resources ride inside this struct, so they are provisioned
/// and dropped together with the geometry (plan §3.1). Owned by the caller
/// (typically a display type holding it behind an [`Arc`]); replacing a
/// cloud drops the old mesh and wgpu destroys its buffers after the frame
/// using them has finished, which satisfies the safe-replacement requirement
/// of the rendering contract.
pub struct PointCloudMesh {
    positions: wgpu::Buffer,
    colors: wgpu::Buffer,
    count: u32,
    bind_group: wgpu::BindGroup,
    appearance: AppearanceGpu,
}

impl PointCloudMesh {
    /// Assemble a point cloud mesh from already-created buffers (crate-
    /// internal). The face-less scatter form of a mesh display (spec §7 F1)
    /// is drawn through the point pipeline, so `render/mesh.rs` provisions
    /// the same geometry shape and hands it to [`Renderer::paint`]; this
    /// constructor is that path's only entry point, keeping the fields
    /// private to the renderer module.
    pub(crate) fn from_parts(
        positions: wgpu::Buffer,
        colors: wgpu::Buffer,
        count: u32,
        bind_group: wgpu::BindGroup,
        appearance: AppearanceGpu,
    ) -> Self {
        Self {
            positions,
            colors,
            count,
            bind_group,
            appearance,
        }
    }
}

/// Owns the point cloud pipeline, the scene-wide view-projection uniform,
/// and uploads point cloud meshes.
///
/// The device, queue, target format, depth format, and sample count are
/// injected: the host owns the adapter/device/surface and opens the render
/// pass, and wgpu-core's `check_compatible` requires the pass and every
/// pipeline recording into it to agree exactly on the depth format and the
/// sample count — so those two come from the host too (display-types spec
/// §6: Depth24Plus, samples = 1 for now). The renderer is created once per
/// render target and is the single source of these values for the whole
/// scene; when the host notices a format or sample count change it rebuilds
/// the renderer and re-uploads the meshes.
pub struct Renderer {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    target_format: wgpu::TextureFormat,
    depth_format: wgpu::TextureFormat,
    sample_count: u32,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    /// The per-object appearance bind group layout (group 1, binding 0 —
    /// the 64-byte [`Appearance`] uniform), shared verbatim by every family
    /// pipeline, like the scene bind group layout. Per-object uniform
    /// buffers and bind groups are provisioned against it by the upload
    /// paths of the family (renderer.rs, mesh.rs, line.rs).
    appearance_bind_group_layout: wgpu::BindGroupLayout,
    /// The scene's single view-projection uniform buffer, referenced by
    /// every mesh bind group; `update_uniform` rewrites it once per frame.
    uniform_buffer: wgpu::Buffer,
}

impl Renderer {
    /// Create the point cloud pipeline against an injected device/queue.
    ///
    /// The WGSL is embedded and naga-validated in CI (see the unit tests), so
    /// a failure here is a shader/pipeline inconsistency with the wgpu
    /// runtime, surfaced by wgpu through its error handling; no device,
    /// adapter, or surface is created by this type. `depth_format` and
    /// `sample_count` must equal those of the render pass the host opens for
    /// the scene (wgpu-core `check_compatible` enforces the equality); the
    /// host rebuilds the renderer when either changes, exactly as for a
    /// target format change.
    pub fn new(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        target_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        sample_count: u32,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("point_cloud"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(POINT_CLOUD_SHADER_SOURCE)),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene"),
            entries: &scene_bind_group_layout_entries(),
        });
        let appearance_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("appearance"),
                entries: &appearance_bind_group_layout_entries(),
            });

        // Two bind groups per pipeline: group 0 is the scene-wide
        // view-projection uniform, group 1 the per-object appearance
        // uniform every uploaded mesh carries (ui-blueprint spec §6).
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("point_cloud"),
            bind_group_layouts: &[&bind_group_layout, &appearance_bind_group_layout],
            push_constant_ranges: &[],
        });

        let vertex_buffer_layouts = point_vertex_buffer_layouts();

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("point_cloud"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &vertex_buffer_layouts,
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::PointList,
                ..Default::default()
            },
            // Shared depth (display-types spec §6): the point pipeline
            // writes depth with a strict Less compare and no bias — points
            // are the reference surface that later mesh pipelines are
            // depth-biased against.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            // Must equal the sample count of the render pass the host opens
            // (wgpu-core `check_compatible`): 1 for now, per display-types
            // spec §6; when MSAA is enabled the renderer rebuilds with the
            // pass's count.
            multisample: wgpu::MultisampleState {
                count: sample_count,
                ..Default::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        // One uniform buffer for the whole scene: every mesh's bind group
        // references it, and `update_uniform` rewrites it once per frame.
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene.view_proj"),
            size: UNIFORM_SIZE_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Defined initial state: frames that somehow draw before the first
        // prepare still see the identity transform instead of stale memory.
        queue.write_buffer(&uniform_buffer, 0, &pack_view_proj(glam::Mat4::IDENTITY));

        Self {
            device,
            queue,
            target_format,
            depth_format,
            sample_count,
            pipeline,
            bind_group_layout,
            appearance_bind_group_layout,
            uniform_buffer,
        }
    }

    /// The render target format the pipeline was built for. The host checks
    /// it against the current surface format and rebuilds the renderer when
    /// they diverge (e.g. after moving the window across screens).
    pub fn target_format(&self) -> wgpu::TextureFormat {
        self.target_format
    }

    /// The depth format every scene pipeline is built for. The host must
    /// open the render pass with this same format (wgpu-core
    /// `check_compatible`) and rebuild the renderer when it changes.
    pub fn depth_format(&self) -> wgpu::TextureFormat {
        self.depth_format
    }

    /// The multisample count every scene pipeline is built with. The host's
    /// render pass must use the same count (wgpu-core `check_compatible`
    /// enforces the equality) and the renderer must be rebuilt when it
    /// changes.
    pub fn sample_count(&self) -> u32 {
        self.sample_count
    }

    /// The device every scene pipeline is built from — the injected host
    /// device (see [`Renderer::new`]). The mesh and line pipelines of the
    /// display-type family (`render/mesh.rs`, `render/line.rs`) are created
    /// from this renderer and read the device, queue, formats, bind group
    /// layout, and uniform buffer through these accessors, so the renderer
    /// stays the single source of the shared-depth parameters for the whole
    /// scene (plan §3.3): no family pipeline can be built with a depth
    /// format or sample count that differs from the pass the host opens.
    pub fn device(&self) -> &Arc<wgpu::Device> {
        &self.device
    }

    /// The queue scene uploads write through; see [`Renderer::device`].
    pub fn queue(&self) -> &Arc<wgpu::Queue> {
        &self.queue
    }

    /// The scene-wide bind group layout (binding 0: the view-projection
    /// uniform), shared verbatim by every family pipeline. Reusing this
    /// exact layout object — rather than a structurally identical copy —
    /// makes every mesh bind group layout-compatible with every pipeline.
    pub fn scene_bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.bind_group_layout
    }

    /// The per-object appearance bind group layout (binding 0 of group 1:
    /// the 64-byte [`Appearance`] uniform), shared verbatim by every family
    /// pipeline — see [`Renderer::scene_bind_group_layout`]. The upload
    /// paths provision each object's appearance uniform buffer and bind
    /// group against this exact layout object, so every mesh bind group is
    /// layout-compatible with every pipeline.
    pub fn appearance_bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.appearance_bind_group_layout
    }

    /// The scene's single view-projection uniform buffer (one 64-byte
    /// `mat4x4<f32>`, rewritten once per frame by [`Renderer::update_uniform`]).
    /// Family pipeline uploads bind their meshes against this same buffer,
    /// so one queue write per frame reaches every pipeline of the scene.
    pub fn uniform_buffer(&self) -> &wgpu::Buffer {
        &self.uniform_buffer
    }

    /// Write the view-projection matrix into the renderer's single uniform
    /// buffer, which every uploaded mesh's bind group references.
    ///
    /// Called once per frame from the host's prepare stage, before the
    /// render pass that records the draws: the scene shares one matrix, so
    /// this replaces the per-mesh uniform uploads with one 64-byte queue
    /// write per frame. Stack-only, no per-frame allocation.
    pub fn update_uniform(&self, queue: &wgpu::Queue, view_proj: glam::Mat4) {
        queue.write_buffer(&self.uniform_buffer, 0, &pack_view_proj(view_proj));
    }

    /// Upload one cloud to the GPU and return its mesh.
    ///
    /// Called from the host's prepare stage once per data replacement — the
    /// data is static, so there is no per-frame upload. Positions are packed
    /// tightly (12 bytes per point); colors are one Rgba8Unorm u32 per point
    /// with an opaque alpha and the sRGB file bytes unchanged, so a 3-byte
    /// file color and the stride-4 requirement of WebGPU vertex buffers are
    /// both satisfied. The mesh's bind group binds the renderer's shared
    /// view-projection buffer at binding 0; [`Renderer::update_uniform`]
    /// refreshes that buffer once per frame.
    pub fn upload(&mut self, data: &io::PointCloudData) -> Arc<PointCloudMesh> {
        // A6 ledger (spec §4): provisioning a point cloud handle counts one
        // created event; the display type's Drop counts the matching
        // destroyed event when the object leaves the scene (counters.rs).
        counters::note_uploaded(DisplayKind::PointCloud);
        let count = u32::try_from(data.positions.len()).expect(
            "more than u32::MAX points cannot be drawn in one call; holding that many \
             positions needs 51 GiB of RAM, far above what the io size guards admit",
        );
        let position_bytes = pack_positions(&data.positions);
        let color_bytes = pack_colors(data.positions.len(), data.colors.as_deref());

        let positions = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("point_cloud.positions"),
            size: position_bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let colors = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("point_cloud.colors"),
            size: color_bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        self.queue.write_buffer(&positions, 0, &position_bytes);
        self.queue.write_buffer(&colors, 0, &color_bytes);

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("point_cloud"),
            layout: &self.bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.uniform_buffer.as_entire_binding(),
            }],
        });

        // The per-object appearance channel rides in the same handle: one
        // uniform buffer + one bind group, created here and dropped with
        // the mesh (plan §3.1: uniform 与几何句柄同生共死).
        let appearance = create_appearance_gpu(
            &self.device,
            &self.queue,
            &self.appearance_bind_group_layout,
            "point_cloud.appearance",
            &Appearance::DEFAULT,
        );

        Arc::new(PointCloudMesh {
            positions,
            colors,
            count,
            bind_group,
            appearance,
        })
    }

    /// Record the draw of one cloud into an externally opened render pass.
    ///
    /// This never creates, ends, or submits a pass, encoder, or queue
    /// submission: the host (egui-wgpu) opens one pass per frame and submits
    /// once, which the rendering contract requires. Sets the pipeline, the
    /// mesh's bind groups (group 0: the scene-wide view-projection uniform;
    /// group 1: the mesh's appearance uniform), the two vertex buffers
    /// (slot 0 positions, slot 1 colors), and issues a single draw of all
    /// points as one instance.
    pub fn paint(&self, pass: &mut wgpu::RenderPass<'static>, mesh: &PointCloudMesh) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &mesh.bind_group, &[]);
        pass.set_bind_group(1, &mesh.appearance.bind_group, &[]);
        pass.set_vertex_buffer(0, mesh.positions.slice(..));
        pass.set_vertex_buffer(1, mesh.colors.slice(..));
        pass.draw(0..mesh.count, 0..1);
    }

    /// Refresh the appearance of one uploaded point cloud in place (plan
    /// §3.1): one 64-byte queue write into the mesh's preallocated uniform
    /// buffer. Never creates a buffer or bind group and never triggers a
    /// renderer or scene rebuild; the effect is visible on the next frame.
    /// The neutral default is [`Appearance::DEFAULT`].
    pub fn set_appearance(&self, mesh: &PointCloudMesh, appearance: &Appearance) {
        write_appearance(&self.queue, &mesh.appearance, appearance);
    }

    /// Convert one sRGB 8-bit channel to linear light using the standard
    /// piecewise EOTF (IEC 61966-2-1): linear below the 0.04045 knee, gamma
    /// 2.4 above it.
    ///
    /// Mirrored by `srgb_to_linear` in `wgsl/point_cloud.wgsl` — the GPU
    /// conversion is the one that runs for rendered points; this CPU copy
    /// exists as the reference implementation for headless tests and for
    /// future CPU-side color work, and a unit test pins the two together.
    pub fn srgb_to_linear(c: u8) -> f32 {
        let v = f32::from(c) / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The line and mesh shader sources, embedded here so the appearance
    /// channel layout can be pinned once against all three pipelines of the
    /// scene (their own test modules compile each shader headlessly; this
    /// module owns the layout parity check).
    const LINE_SHADER_SOURCE: &str = include_str!("../../assets/shaders/line.wgsl");
    const MESH_SHADER_SOURCE: &str = include_str!("../../assets/shaders/mesh.wgsl");

    /// Compile-time assertion that a type is `bytemuck::Pod`.
    fn assert_pod<T: bytemuck::Pod>() {}

    #[test]
    fn srgb_to_linear_endpoints_and_midpoint() {
        // Black maps to black and full byte value to full linear light.
        assert_eq!(Renderer::srgb_to_linear(0), 0.0);
        assert_eq!(Renderer::srgb_to_linear(255), 1.0);
        // Mid-gray 128/255 = 0.5019608... under the standard curve is
        // ≈ 0.2158605 (a plain 2.2 gamma would give ≈ 0.2196).
        let mid = Renderer::srgb_to_linear(128);
        assert!(
            (mid - 0.215_860_5).abs() < 1e-6,
            "srgb byte 128 -> {mid}, expected ~0.2158605"
        );
    }

    #[test]
    fn srgb_to_linear_is_finite_monotonic_and_in_range() {
        let mut previous = f32::NEG_INFINITY;
        for byte in 0..=255u8 {
            let value = Renderer::srgb_to_linear(byte);
            assert!(
                value.is_finite() && (0.0..=1.0).contains(&value),
                "srgb byte {byte} -> {value}, expected a value in [0, 1]"
            );
            assert!(
                value >= previous,
                "srgb curve must not decrease: byte {byte} -> {value} < {previous}"
            );
            previous = value;
        }
    }

    #[test]
    fn positions_pack_as_little_endian_f32_triples() {
        let points = [glam::Vec3::new(1.0, 2.0, -3.5)];
        let bytes = pack_positions(&points);
        assert_eq!(bytes.len(), 12);
        assert_eq!(&bytes[0..4], &1.0f32.to_le_bytes());
        assert_eq!(&bytes[4..8], &2.0f32.to_le_bytes());
        assert_eq!(&bytes[8..12], &(-3.5f32).to_le_bytes());
    }

    #[test]
    fn file_colors_pack_as_rgba8_bytes_with_opaque_alpha() {
        let colors = [io::Color {
            r: 255,
            g: 0,
            b: 128,
        }];
        let bytes = pack_colors(1, Some(&colors));
        assert_eq!(bytes, [255, 0, 128, 255]);
    }

    #[test]
    fn missing_colors_pack_the_default_color_per_point() {
        let bytes = pack_colors(2, None);
        assert_eq!(bytes.len(), 8);
        assert_eq!(&bytes[0..4], &DEFAULT_POINT_COLOR_SRGB);
        assert_eq!(&bytes[4..8], &DEFAULT_POINT_COLOR_SRGB);
    }

    #[test]
    fn shader_and_cpu_color_curve_share_constants() {
        // Both srgb_to_linear implementations must move together; the
        // constants of the piecewise EOTF pin the WGSL copy to the CPU one.
        for constant in ["0.04045", "12.92", "0.055", "1.055", "2.4"] {
            assert!(
                POINT_CLOUD_SHADER_SOURCE.contains(constant),
                "point_cloud.wgsl is missing sRGB constant {constant}"
            );
        }
    }

    #[test]
    fn point_cloud_wgsl_compiles_headlessly() {
        // CI has no GPU: naga compiles and fully validates the embedded
        // shader. wgpu uses the same naga major version at pipeline
        // creation, so this test is a faithful proxy for the runtime step.
        let module = naga::front::wgsl::parse_str(POINT_CLOUD_SHADER_SOURCE)
            .unwrap_or_else(|error| panic!("point_cloud.wgsl failed to parse:\n{error}"));
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator
            .validate(&module)
            .unwrap_or_else(|error| panic!("point_cloud.wgsl failed naga validation:\n{error}"));
    }

    #[test]
    fn vertex_layout_matches_the_bytemuck_types() {
        // Uniform upload casts one [f32; 16] column array into the [u8; 64]
        // of the uniform buffer; colors are raw [u8; 4] texels; the CPU
        // helper functions are the only producers of position bytes. These
        // assertions pin the Pod casts to the layout.
        assert_pod::<f32>();
        assert_pod::<[f32; 16]>();
        assert_pod::<[u8; 4]>();
        assert_pod::<[u8; 64]>();

        assert_eq!(size_of::<f32>(), 4);
        assert_eq!(align_of::<f32>(), 4);
        assert_eq!(size_of::<[u8; 64]>(), 64);
        assert_eq!(wgpu::VertexFormat::Float32x3.size(), POSITION_STRIDE_BYTES);
        assert_eq!(wgpu::VertexFormat::Unorm8x4.size(), COLOR_STRIDE_BYTES);
        assert_eq!(UNIFORM_SIZE_BYTES, 64);
    }

    #[test]
    fn view_proj_packs_column_major_little_endian() {
        // WGSL `mat4x4<f32>` uniform memory layout is column-major: each
        // column is a contiguous 16-byte run, in column order. The matrix
        // below is built so element (row, column) equals `column * 4 + row`,
        // which pins the byte offset to (column * 16 + row * 4) with
        // little-endian f32 — the layout wgpu uploads to the GPU.
        let matrix = glam::Mat4::from_cols_array(&[
            0.0, 1.0, 2.0, 3.0, // column 0
            4.0, 5.0, 6.0, 7.0, // column 1
            8.0, 9.0, 10.0, 11.0, // column 2
            12.0, 13.0, 14.0, 15.0, // column 3
        ]);
        let bytes = pack_view_proj(matrix);
        assert_eq!(bytes.len(), 64);
        for column in 0..4u32 {
            for row in 0..4u32 {
                let value = (column * 4 + row) as f32;
                let offset = (column * 16 + row * 4) as usize;
                assert_eq!(
                    &bytes[offset..offset + 4],
                    &value.to_le_bytes(),
                    "element (row {row}, column {column}) must sit at byte offset {offset}"
                );
            }
        }
    }

    #[test]
    fn shared_bind_group_layout_binds_one_uniform_at_binding_zero() {
        // The whole scene shares ONE view-projection uniform (display-types
        // plan §3.3): the bind group layout has exactly one entry — binding
        // 0, a vertex-stage uniform buffer — and every mesh bind group binds
        // the renderer's single 64-byte buffer there. Shader-side this is
        // `@group(0) @binding(0) var<uniform> view_proj: mat4x4<f32>;`.
        let entries = scene_bind_group_layout_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].binding, 0);
        assert_eq!(entries[0].visibility, wgpu::ShaderStages::VERTEX);
        assert_eq!(entries[0].count, None);
        assert_eq!(
            entries[0].ty,
            wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            }
        );
    }

    #[test]
    fn point_pipeline_takes_two_vertex_buffers_positions_then_colors() {
        // A mesh's GPU footprint is two vertex buffers plus the one shared
        // uniform: the pipeline consumes exactly two vertex buffer layouts,
        // in the slot order `paint` sets them (slot 0 positions, slot 1
        // colors), and the shader reads them through attribute locations 0
        // and 1 respectively.
        let layouts = point_vertex_buffer_layouts();
        assert_eq!(layouts.len(), 2);

        let positions = &layouts[0];
        assert_eq!(positions.array_stride, POSITION_STRIDE_BYTES);
        assert_eq!(positions.step_mode, wgpu::VertexStepMode::Vertex);
        assert_eq!(positions.attributes, &POSITION_ATTRIBUTES[..]);

        let colors = &layouts[1];
        assert_eq!(colors.array_stride, COLOR_STRIDE_BYTES);
        assert_eq!(colors.step_mode, wgpu::VertexStepMode::Vertex);
        assert_eq!(colors.attributes, &COLOR_ATTRIBUTES[..]);
    }

    #[test]
    fn appearance_packs_into_the_fixed_64_byte_uniform_layout() {
        // The CPU packer and the WGSL `ObjectAppearance` struct of the three
        // shaders must agree byte for byte (the shader side is pinned by
        // `three_shaders_declare_the_same_fixed_appearance_layout`): albedo
        // at 0 as four little-endian f32, flags at 16 as one little-endian
        // u32, everything else — the 12-byte implicit padding after flags
        // and the 32 reserved bytes — zero.
        let appearance = Appearance::new(
            [0.1, 0.2, 0.3, 1.0],
            APPEARANCE_FLAG_OVERRIDE | APPEARANCE_FLAG_SELECTED,
        );
        let bytes = pack_appearance(&appearance);
        assert_eq!(bytes.len(), 64);
        assert_eq!(bytes.len(), APPEARANCE_SIZE_BYTES as usize);
        assert_eq!(&bytes[0..4], &0.1f32.to_le_bytes());
        assert_eq!(&bytes[4..8], &0.2f32.to_le_bytes());
        assert_eq!(&bytes[8..12], &0.3f32.to_le_bytes());
        assert_eq!(&bytes[12..16], &1.0f32.to_le_bytes());
        assert_eq!(
            &bytes[16..20],
            &(APPEARANCE_FLAG_OVERRIDE | APPEARANCE_FLAG_SELECTED).to_le_bytes()
        );
        assert!(
            bytes[20..].iter().all(|&b| b == 0),
            "the padding and reserved region must stay zero"
        );

        // The neutral default: zero albedo channels, opaque alpha, no flags.
        let default = pack_appearance(&Appearance::DEFAULT);
        assert!(default[0..12].iter().all(|&b| b == 0));
        assert_eq!(&default[12..16], &1.0f32.to_le_bytes());
        assert!(default[16..].iter().all(|&b| b == 0));
    }

    #[test]
    fn appearance_bind_group_layout_binds_one_uniform_at_group_one() {
        // Every family pipeline carries the per-object channel as exactly
        // one entry at binding 0 — a uniform buffer visible to both shader
        // stages (the fragment stage mixes the channel today; VERTEX in the
        // visibility leaves the layout ready for later work without a
        // relayout). WGSL side: `@group(1) @binding(0)`.
        let entries = appearance_bind_group_layout_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].binding, 0);
        assert_eq!(entries[0].visibility, wgpu::ShaderStages::VERTEX_FRAGMENT);
        assert_eq!(entries[0].count, None);
        assert_eq!(
            entries[0].ty,
            wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            }
        );
        assert_eq!(APPEARANCE_SIZE_BYTES, 64);
    }

    #[test]
    fn srgb_override_converts_to_linear_light_and_sets_the_flag() {
        // The palette-token entry point (spec §6): a token's sRGB bytes
        // reach the shader as linear-light albedo plus the override bit, so
        // the fragment mixer substitutes them for the per-vertex colors.
        let color = io::Color {
            r: 128,
            g: 64,
            b: 255,
        };
        let appearance = Appearance::srgb_override(color);
        assert_eq!(appearance.flags, APPEARANCE_FLAG_OVERRIDE);
        assert_eq!(appearance.albedo[3], 1.0);
        for (channel, byte) in appearance.albedo[..3].iter().zip([128, 64, 255]) {
            let expected = Renderer::srgb_to_linear(byte);
            assert!(
                (channel - expected).abs() < 1e-6,
                "albedo channel {channel} must equal the CPU srgb_to_linear({byte}) = {expected}"
            );
        }
    }

    #[test]
    fn with_selected_toggles_only_the_selection_bit() {
        let base = Appearance::srgb_override(io::Color {
            r: 10,
            g: 20,
            b: 30,
        });
        let selected = base.with_selected(true);
        assert_eq!(
            selected.flags,
            APPEARANCE_FLAG_OVERRIDE | APPEARANCE_FLAG_SELECTED,
            "the override must survive setting the selection marker"
        );
        assert_eq!(selected.albedo, base.albedo);
        assert_eq!(selected.with_selected(false), base, "clearing is lossless");
        assert_eq!(base.with_selected(false), base);
    }

    #[test]
    fn shader_appearance_flags_mirror_the_cpu_constants() {
        // The WGSL mixers of the scene declare the same flag values and the
        // same group(1)/binding(0) uniform as the CPU constants above them
        // (per-file pinning style). Point and line colors are per-vertex, so
        // their mixers read the override bit; mesh faces have no per-vertex
        // color, so mesh.wgsl deliberately omits the override path and the
        // former WGSL `FACE_COLOR` constant is gone entirely.
        for source in [POINT_CLOUD_SHADER_SOURCE, LINE_SHADER_SOURCE] {
            assert!(source.contains("const APPEARANCE_FLAG_OVERRIDE: u32 = 1u;"));
            assert!(source.contains("const APPEARANCE_FLAG_SELECTED: u32 = 2u;"));
            assert!(source.contains("const HIGHLIGHT_GAIN: f32 = 1.25;"));
            assert!(source.contains("@group(1) @binding(0)"));
            assert!(source.contains("var<uniform> appearance: ObjectAppearance;"));
        }
        assert!(MESH_SHADER_SOURCE.contains("const APPEARANCE_FLAG_SELECTED: u32 = 2u;"));
        assert!(MESH_SHADER_SOURCE.contains("const HIGHLIGHT_GAIN: f32 = 1.25;"));
        assert!(MESH_SHADER_SOURCE.contains("@group(1) @binding(0)"));
        assert!(
            !MESH_SHADER_SOURCE.contains("const APPEARANCE_FLAG_OVERRIDE"),
            "a mesh has no per-vertex color to override — the bit must never be declared \
             (prose mentions in the comments are fine)"
        );
        assert!(
            !MESH_SHADER_SOURCE.contains("const FACE_COLOR"),
            "the face color moved into the appearance uniform; a WGSL constant would be a \
             second color path"
        );

        assert_eq!(APPEARANCE_FLAG_OVERRIDE, 1);
        assert_eq!(APPEARANCE_FLAG_SELECTED, 2);
        assert_eq!(APPEARANCE_FLAG_OVERRIDE & APPEARANCE_FLAG_SELECTED, 0);
    }

    /// Parse, validate, and walk one shader: exactly one struct must match
    /// the appearance channel's member list, spanning the fixed 64 bytes
    /// with the packer's member offsets (albedo 0, flags 16, reserved_a 32,
    /// reserved_b 48) — the WGSL half of the layout parity `pack_appearance`
    /// tests on the CPU side.
    fn assert_appearance_uniform_layout(source: &str) {
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|error| panic!("shader failed to parse:\n{error}"));
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator
            .validate(&module)
            .unwrap_or_else(|error| panic!("shader failed naga validation:\n{error}"));

        let member_names = [
            Some("albedo"),
            Some("flags"),
            Some("reserved_a"),
            Some("reserved_b"),
        ];
        let mut matches = 0;
        for (_, ty) in module.types.iter() {
            let naga::TypeInner::Struct { members, span } = &ty.inner else {
                continue;
            };
            if !members
                .iter()
                .map(|member| member.name.as_deref())
                .eq(member_names.iter().copied())
            {
                continue;
            }
            matches += 1;
            assert_eq!(
                *span, APPEARANCE_SIZE_BYTES as u32,
                "the appearance struct must span the fixed 64 bytes"
            );
            let offsets: Vec<u32> = members.iter().map(|member| member.offset).collect();
            assert_eq!(offsets, [0, 16, 32, 48]);
        }
        assert_eq!(
            matches, 1,
            "each shader declares exactly one appearance struct"
        );
    }

    #[test]
    fn three_shaders_declare_the_same_fixed_appearance_layout() {
        for source in [
            POINT_CLOUD_SHADER_SOURCE,
            LINE_SHADER_SOURCE,
            MESH_SHADER_SOURCE,
        ] {
            assert_appearance_uniform_layout(source);
        }
    }

    #[test]
    fn appearance_channel_fifty_round_cycle_stays_byte_deterministic() {
        // Headless 50-round regression of the appearance lifecycle the A6
        // ledger test models with real handles (counters.rs): a scene object
        // is added with a neutral appearance, toggles its selection marker
        // and gets recolored, and is dropped back to neutral. This file has
        // no device (CI runs GPU-less), so the cycle runs over the CPU
        // packer — the byte stream every `create_appearance_gpu`/
        // `write_appearance` queue-writes — and pins that the 50 rounds
        // never accumulate state: each round's bytes are bit-identical to
        // round zero's, flags are exactly the two documented bits, and the
        // packed size never leaves 64 bytes. The created==destroyed ledger
        // balance itself stays the counters.rs test's job.
        let neutral = pack_appearance(&Appearance::DEFAULT);
        assert_eq!(neutral.len(), APPEARANCE_SIZE_BYTES as usize);
        let neutral_flags = u32::from_le_bytes(neutral[16..20].try_into().unwrap());
        assert_eq!(neutral_flags, 0);

        for round in 0..50u32 {
            // "Add": an object appears with a neutral appearance — the byte
            // stream of round zero, exactly, every round.
            assert_eq!(pack_appearance(&Appearance::DEFAULT), neutral);

            // Ten marker toggles plus one recolor (the frame-rate poll of
            // the A12 performance protocol is a same-channel cycle): every
            // write is a valid 64-byte stream carrying exactly the two
            // documented bits.
            let mut appearance = Appearance::new(
                [
                    Renderer::srgb_to_linear((round % 251) as u8),
                    Renderer::srgb_to_linear(77),
                    Renderer::srgb_to_linear(149),
                    1.0,
                ],
                APPEARANCE_FLAG_OVERRIDE,
            );
            for step in 0..10u32 {
                appearance = appearance.with_selected(step % 2 == 0);
                let bytes = pack_appearance(&appearance);
                assert_eq!(bytes.len(), APPEARANCE_SIZE_BYTES as usize);
                let flags = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
                let expected = APPEARANCE_FLAG_OVERRIDE
                    | if step % 2 == 0 {
                        APPEARANCE_FLAG_SELECTED
                    } else {
                        0
                    };
                assert_eq!(flags, expected, "round {round} step {step}");
            }

            // "Drop": the object leaves — back to the identical neutral
            // stream; 50 rounds leave no residue behind.
            assert_eq!(pack_appearance(&Appearance::DEFAULT), neutral);
        }
    }
}
