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

use crate::io;

/// Embedded WGSL source of the point cloud pipeline. Compiled headlessly
/// against naga in the unit tests; the same naga major version validates it
/// again inside wgpu when the pipeline is created.
const POINT_CLOUD_SHADER_SOURCE: &str = include_str!("../../assets/shaders/point_cloud.wgsl");

/// Default point color as sRGB 8-bit bytes with opaque alpha, used when the
/// data has no per-point colors. Derived from the sRGB floats (0.8, 0.8, 0.9):
/// 0.8 * 255 = 204 and 0.9 * 255 = 229.5 rounds to 229.
const DEFAULT_POINT_COLOR_SRGB: [u8; 4] = [204, 204, 229, 255];

/// Byte stride of one position vertex: x, y, z as three `f32`.
const POSITION_STRIDE_BYTES: u64 = 12;

/// Byte stride of one color vertex: a single packed Rgba8Unorm texel.
const COLOR_STRIDE_BYTES: u64 = 4;

/// Byte size of the scene's single view-projection uniform buffer holding
/// one `mat4x4<f32>`.
const UNIFORM_SIZE_BYTES: u64 = 64;

/// Vertex attribute of the position buffer (vertex slot 0): `x y z` at
/// shader location 0.
const POSITION_ATTRIBUTES: [wgpu::VertexAttribute; 1] = [wgpu::VertexAttribute {
    format: wgpu::VertexFormat::Float32x3,
    offset: 0,
    shader_location: 0,
}];

/// Vertex attribute of the color buffer (vertex slot 1): one packed
/// Rgba8Unorm texel at shader location 1. The hardware unorm-decodes the
/// four bytes into [0, 1] floats; the shader therefore declares the input
/// as `vec4<f32>` and converts sRGB to linear itself (wgpu-core maps
/// Unorm8x4 to a float vector: "the shader always sees data as float").
const COLOR_ATTRIBUTES: [wgpu::VertexAttribute; 1] = [wgpu::VertexAttribute {
    format: wgpu::VertexFormat::Unorm8x4,
    offset: 0,
    shader_location: 1,
}];

/// Serialize positions into tightly packed little-endian `f32` triples
/// (GPU vertex data is little-endian on every supported backend).
fn pack_positions(positions: &[glam::Vec3]) -> Vec<u8> {
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
/// no colors, every point gets [`DEFAULT_POINT_COLOR_SRGB`].
fn pack_colors(count: usize, colors: Option<&[io::Color]>) -> Vec<u8> {
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

/// GPU handles of one uploaded point cloud: its two vertex buffers plus the
/// bind group that references the renderer's scene-wide view-projection
/// uniform buffer.
///
/// The uniform data itself is not per-mesh: [`Renderer`] owns the one
/// buffer and rewrites it once per frame through [`Renderer::update_uniform`],
/// so uploading or dropping a cloud never touches the matrix every mesh
/// sees. Owned by the caller (typically a display type holding it behind an
/// [`Arc`]); replacing a cloud drops the old mesh and wgpu destroys its
/// buffers after the frame using them has finished, which satisfies the
/// safe-replacement requirement of the rendering contract.
pub struct PointCloudMesh {
    positions: wgpu::Buffer,
    colors: wgpu::Buffer,
    count: u32,
    bind_group: wgpu::BindGroup,
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("point_cloud"),
            bind_group_layouts: &[&bind_group_layout],
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

        Arc::new(PointCloudMesh {
            positions,
            colors,
            count,
            bind_group,
        })
    }

    /// Record the draw of one cloud into an externally opened render pass.
    ///
    /// This never creates, ends, or submits a pass, encoder, or queue
    /// submission: the host (egui-wgpu) opens one pass per frame and submits
    /// once, which the rendering contract requires. Sets the pipeline, the
    /// mesh's bind group (binding 0: the scene-wide view-projection uniform),
    /// the two vertex buffers (slot 0 positions, slot 1 colors), and issues
    /// a single draw of all points as one instance.
    pub fn paint(&self, pass: &mut wgpu::RenderPass<'static>, mesh: &PointCloudMesh) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &mesh.bind_group, &[]);
        pass.set_vertex_buffer(0, mesh.positions.slice(..));
        pass.set_vertex_buffer(1, mesh.colors.slice(..));
        pass.draw(0..mesh.count, 0..1);
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
}
