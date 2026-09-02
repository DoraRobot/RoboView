//! Point cloud rendering: pipeline ownership, buffer upload, and draw calls.
//!
//! The renderer owns everything GPU-side that the display types do not: the
//! point cloud pipeline and the bind group layout shared by every uploaded
//! mesh. It never creates an instance, adapter, device, queue, surface, or
//! swapchain — all wgpu objects come from the [`wgpu::Device`] and
//! [`wgpu::Queue`] injected by the host (egui-wgpu, see the rendering
//! contract in `docs/specs/point-cloud-viewport/plan.md` §3.2). Drawing
//! happens inside an externally opened render pass: this module records
//! commands only and never starts, ends, or submits a pass or encoder.

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

/// Byte size of the per-mesh uniform buffer holding one `mat4x4<f32>`.
const UNIFORM_SIZE_BYTES: u64 = 64;

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

/// GPU handles of one uploaded point cloud: two vertex buffers, the shared
/// view-projection uniform, and the bind group that references it.
///
/// Owned by the caller (typically a display type holding it behind an
/// [`Arc`]); replacing a cloud drops the old mesh and wgpu destroys its
/// buffers after the frame using them has finished, which satisfies the
/// safe-replacement requirement of the rendering contract.
pub struct PointCloudMesh {
    positions: wgpu::Buffer,
    colors: wgpu::Buffer,
    count: u32,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

/// Owns the point cloud pipeline and uploads point cloud meshes.
///
/// The device, queue, and target format are injected (the host owns the
/// adapter/device/surface and the swapchain format); the renderer derives
/// everything it needs from them. It is created once per render target —
/// when the host notices a target format change it rebuilds the renderer.
pub struct Renderer {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    target_format: wgpu::TextureFormat,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
}

impl Renderer {
    /// Create the point cloud pipeline against an injected device/queue.
    ///
    /// The WGSL is embedded and naga-validated in CI (see the unit tests), so
    /// a failure here is a shader/pipeline inconsistency with the wgpu
    /// runtime, surfaced by wgpu through its error handling; no device,
    /// adapter, or surface is created by this type.
    pub fn new(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        target_format: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("point_cloud"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(POINT_CLOUD_SHADER_SOURCE)),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("point_cloud"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("point_cloud"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let position_attributes = [wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 0,
            shader_location: 0,
        }];
        let position_buffer = wgpu::VertexBufferLayout {
            array_stride: POSITION_STRIDE_BYTES,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &position_attributes,
        };
        let color_attributes = [wgpu::VertexAttribute {
            // The hardware unorm-decodes the four bytes into [0, 1] floats;
            // the shader therefore declares the input as `vec4<f32>` and
            // converts sRGB to linear itself (wgpu-core maps Unorm8x4 to a
            // float vector: "the shader always sees data as float").
            format: wgpu::VertexFormat::Unorm8x4,
            offset: 0,
            shader_location: 1,
        }];
        let color_buffer = wgpu::VertexBufferLayout {
            array_stride: COLOR_STRIDE_BYTES,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &color_attributes,
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("point_cloud"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[position_buffer, color_buffer],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::PointList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
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

        Self {
            device,
            queue,
            target_format,
            pipeline,
            bind_group_layout,
        }
    }

    /// The render target format the pipeline was built for. The host checks
    /// it against the current surface format and rebuilds the renderer when
    /// they diverge (e.g. after moving the window across screens).
    pub fn target_format(&self) -> wgpu::TextureFormat {
        self.target_format
    }

    /// Upload one cloud to the GPU and return its mesh.
    ///
    /// Called from the host's prepare stage once per data replacement — the
    /// data is static, so there is no per-frame upload. Positions are packed
    /// tightly (12 bytes per point); colors are one Rgba8Unorm u32 per point
    /// with an opaque alpha and the sRGB file bytes unchanged, so a 3-byte
    /// file color and the stride-4 requirement of WebGPU vertex buffers are
    /// both satisfied. The uniform starts out as the identity matrix and is
    /// refreshed per frame by [`Renderer::prepare_uniform`].
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
        let uniform = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("point_cloud.uniform"),
            size: UNIFORM_SIZE_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        self.queue.write_buffer(&positions, 0, &position_bytes);
        self.queue.write_buffer(&colors, 0, &color_bytes);

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("point_cloud"),
            layout: &self.bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });

        let mesh = Arc::new(PointCloudMesh {
            positions,
            colors,
            count,
            uniform,
            bind_group,
        });
        // Defined initial state: frames that somehow draw before the first
        // prepare still see the identity transform instead of stale memory.
        self.prepare_uniform(&mesh, glam::Mat4::IDENTITY);
        mesh
    }

    /// Upload the view-projection matrix of one mesh's uniform buffer.
    ///
    /// Called every frame from the host's prepare stage, before the render
    /// pass that records the draw. Stack-only, no per-frame allocation.
    pub fn prepare_uniform(&self, mesh: &PointCloudMesh, view_proj: glam::Mat4) {
        // glam stores matrices column-wise, and WGSL `mat4x4<f32>` uniform
        // memory layout is column-major, so the column array maps directly.
        let matrix = view_proj.to_cols_array();
        let bytes: &[u8] = bytemuck::cast_slice(&matrix);
        self.queue.write_buffer(&mesh.uniform, 0, bytes);
    }

    /// Record the draw of one cloud into an externally opened render pass.
    ///
    /// This never creates, ends, or submits a pass, encoder, or queue
    /// submission: the host (egui-wgpu) opens one pass per frame and submits
    /// once, which the rendering contract requires. Sets the pipeline, bind
    /// group, the two vertex buffers, and issues a single indexed-range draw
    /// of all points as one instance.
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
        // Uniform upload casts one [f32; 16] column array; colors are raw
        // [u8; 4] texels; the CPU helper functions are the only producers of
        // position bytes. These assertions pin the Pod casts to the layout.
        assert_pod::<f32>();
        assert_pod::<[f32; 16]>();
        assert_pod::<[u8; 4]>();

        assert_eq!(size_of::<f32>(), 4);
        assert_eq!(align_of::<f32>(), 4);
        assert_eq!(wgpu::VertexFormat::Float32x3.size(), POSITION_STRIDE_BYTES);
        assert_eq!(wgpu::VertexFormat::Unorm8x4.size(), COLOR_STRIDE_BYTES);
        assert_eq!(UNIFORM_SIZE_BYTES, 64);
    }
}
