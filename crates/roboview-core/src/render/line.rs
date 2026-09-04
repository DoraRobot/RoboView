//! Line pipeline (display-types spec §7 F2/F3/F4, §6): open polylines
//! (paths), coordinate axes (frames), and marker arrows, drawn as
//! `LineStrip` runs.
//!
//! The upload side takes CPU geometry as colored strips (`CpuStrip`) and
//! packs them into one positions buffer and one per-vertex sRGB color buffer
//! (the same packed layout the point pipeline uses, decoded and converted to
//! linear light in the shader). Each strip is one `LineStrip` primitive, so
//! strips are drawn as separate draw calls with (first vertex, count) ranges
//! — no primitive restart and no duplicated endpoints between strips.
//!
//! # CPU geometry policy (line classes)
//!
//! - **Paths** (upload_path): file points in order. Non-finite points (spec
//!   G1: kept in the data) split the polyline into finite runs; every run of
//!   at least two points becomes a strip, so segments that would touch a
//!   non-finite endpoint are dropped instead of drawing an escape line to an
//!   undefined position (a `LineStrip` cannot be clipped per-vertex the way
//!   the point shader escapes points — a strip vertex outside the clip
//!   volume drags its segments across the viewport).
//! - **Frames** (upload_frame): three axis segments from the origin along
//!   +X/+Y/+Z, each its own two-vertex strip, colored X red / Y green /
//!   Z blue (spec §7 F3). Orientation is fixed to the world axes (non-goal:
//!   no frame pose editing).
//! - **Marker arrows** (upload_arrow): a shaft segment plus a two-line arrow
//!   head, three strips of two vertices (spec §7 F4: the head is drawn by
//!   core as short line segments — the app has no triangle mesh for it).
//!
//! Generated line geometry (frames, arrows) is validated at build time:
//! non-finite or non-positive input produces no strips at all (nothing to
//! draw), and every strip that reaches the GPU has at least two finite
//! vertices.
//!
//! # Persistent line meshes and the appearance channel (004 ui-blueprint)
//!
//! Besides the upload forms above, the pipeline provisions two 004 viewport
//! capabilities:
//!
//! - **Persistent meshes** ([`LinePipeline::with_capacity`],
//!   [`LinePipeline::update_mesh`]): vertex buffers preallocated once and
//!   refreshed in place by queue writes — the ground-grid refresh path
//!   (plan §3.3) that must never provision GPU objects per frame. Helper
//!   layers are not scene objects: their mesh is owned by the viewport and
//!   records no A6 ledger rows (spec §6: 不入场景树、不参与台账).
//! - **Appearance channel** ([`Appearance`], plan §3.1): every line mesh
//!   carries one fixed 64-byte uniform buffer plus bind group at
//!   group(1)/binding(0), provisioned and dropped together with the
//!   geometry handle; the shader mixes it over the per-vertex colors
//!   (color-override and selection-highlight flag bits).
//!
//! # Shared depth policy (spec §6, plan §3.3)
//!
//! The pipeline depth-tests with a strict Less compare, never writes depth,
//! and carries no polygon offset (line primitives are unaffected by polygon
//! offset in any case — the spec pairs their strict Less with the mesh
//! pipeline's bias, which pushes the mesh *away* so coplanar lines and
//! points keep winning). Depth writes are disabled so overlapping line work
//! — a self-crossing path, an axis crossing a path — resolves purely by
//! depth compare against the reference surfaces (points, mesh) and never
//! fights at equal depth, and lines never punch holes into the depth buffer
//! that would hide geometry drawn later in the frame.

use std::borrow::Cow;
use std::sync::Arc;

use glam::Vec3;

use super::counters;
use super::renderer::{
    Appearance, AppearanceGpu, COLOR_ATTRIBUTES, COLOR_STRIDE_BYTES, POSITION_ATTRIBUTES,
    POSITION_STRIDE_BYTES, Renderer, create_appearance_gpu, write_appearance,
};
use crate::displays::DisplayKind;
use crate::io;

/// Embedded WGSL source of the line pipeline. Compiled headlessly against
/// naga in the unit tests; the same naga major version validates it again
/// inside wgpu when the pipeline is created.
const LINE_SHADER_SOURCE: &str = include_str!("../../assets/shaders/line.wgsl");

/// Default line color of paths and marker arrows, as sRGB bytes: a soft
/// amber, chosen to read clearly against the lavender default point color
/// and the gray mesh faces.
const LINE_AMBER_SRGB: io::Color = io::Color {
    r: 255,
    g: 196,
    b: 100,
};

/// Frame axis color of X as sRGB bytes: X red (display-types spec §7 F3,
/// semantic color A4). Pure red `(255, 0, 0)` — no gamma or alpha deviation
/// from the spec semantics. `pub` so the app's semantic palette (ui-blueprint
/// spec §6 A9) can assert its origin-axes token against the exact color the
/// frame pipeline draws.
pub const AXIS_X_COLOR_SRGB: io::Color = io::Color { r: 255, g: 0, b: 0 };

/// Frame axis color of Y as sRGB bytes: Y green (display-types spec §7 F3,
/// semantic color A4). Pure green `(0, 255, 0)`.
pub const AXIS_Y_COLOR_SRGB: io::Color = io::Color { r: 0, g: 255, b: 0 };

/// Frame axis color of Z as sRGB bytes: Z blue (display-types spec §7 F3,
/// semantic color A4). Pure blue `(0, 0, 255)`.
pub const AXIS_Z_COLOR_SRGB: io::Color = io::Color { r: 0, g: 0, b: 255 };

/// Arrow head length as a fraction of the shaft length.
const ARROW_HEAD_FRACTION: f32 = 0.25;

/// Half spread of the arrow head lines from the reverse shaft direction
/// (30°): the two head lines open a 60° head.
const ARROW_HEAD_HALF_ANGLE: f32 = std::f32::consts::FRAC_PI_6;

/// One CPU-side colored line strip: an ordered run of vertices drawn as one
/// `LineStrip` primitive, all in one color. Invariant: uploaded strips have
/// at least two finite vertices (see the module docs on CPU geometry
/// policy); the builders below guarantee it.
pub(crate) struct CpuStrip {
    color: io::Color,
    vertices: Vec<Vec3>,
}

/// Split a path's point sequence into its finite runs (spec G1 policy at the
/// geometry level — see the module docs). Every run of at least two finite
/// points becomes a monochrome strip in file order; isolated finite points
/// between non-finite neighbors cannot form a segment and are dropped.
fn path_strips(data: &io::PathData) -> Vec<CpuStrip> {
    let mut strips: Vec<CpuStrip> = Vec::new();
    let mut run: Vec<Vec3> = Vec::new();
    for &point in &data.points {
        if point.is_finite() {
            run.push(point);
            continue;
        }
        // A non-finite point ends the current run. Runs of one point would
        // draw zero segments, so only ≥2-point runs become strips.
        if run.len() >= 2 {
            strips.push(CpuStrip {
                color: LINE_AMBER_SRGB,
                vertices: std::mem::take(&mut run),
            });
        } else {
            run.clear();
        }
    }
    if run.len() >= 2 {
        strips.push(CpuStrip {
            color: LINE_AMBER_SRGB,
            vertices: run,
        });
    }
    strips
}

/// Build the three world-axis segments of a frame (spec §7 F3): from
/// `origin` along +X (red), +Y (green), and +Z (blue), each `length` long
/// and each its own two-vertex strip. Non-finite origins or non-positive or
/// non-finite lengths produce no geometry — a frame the UI built with
/// garbage parameters simply draws nothing.
fn frame_strips(origin: Vec3, length: f32) -> Vec<CpuStrip> {
    if !origin.is_finite() || !length.is_finite() || length <= 0.0 {
        return Vec::new();
    }
    [
        (Vec3::X, AXIS_X_COLOR_SRGB),
        (Vec3::Y, AXIS_Y_COLOR_SRGB),
        (Vec3::Z, AXIS_Z_COLOR_SRGB),
    ]
    .into_iter()
    .map(|(axis, color)| CpuStrip {
        color,
        vertices: vec![origin, origin + axis * length],
    })
    .collect()
}

/// Build the shaft plus two head lines of a marker arrow (spec §7 F4):
/// `start` → `end`, then two short lines from `end` angled 30° off the
/// reverse shaft direction, spread in the plane spanned by the shaft and a
/// world reference axis (world Y; world X when the shaft runs near-parallel
/// to Y, so the spread plane never degenerates). Non-finite endpoints or a
/// zero-length shaft (no direction to spread around) produce no geometry.
fn arrow_strips(start: Vec3, end: Vec3) -> Vec<CpuStrip> {
    if !start.is_finite() || !end.is_finite() {
        return Vec::new();
    }
    let shaft = end - start;
    let length = shaft.length();
    if length <= 0.0 || !shaft.is_finite() {
        return Vec::new();
    }
    let direction = shaft / length;

    // Unit axis perpendicular to the shaft, spanning the head plane. The
    // world-Y reference degenerates only when the shaft is (near-)parallel
    // to Y; world X is then a sound substitute (it is never parallel to Y).
    let perp = {
        let y_cross = direction.cross(Vec3::Y);
        if y_cross.length_squared() < 1e-12 {
            direction.cross(Vec3::X).normalize()
        } else {
            y_cross.normalize()
        }
    };

    let (sin, cos) = ARROW_HEAD_HALF_ANGLE.sin_cos();
    let head_length = length * ARROW_HEAD_FRACTION;
    let tip = |side: f32| end + head_length * (-direction * cos + perp * side * sin);

    vec![
        CpuStrip {
            color: LINE_AMBER_SRGB,
            vertices: vec![start, end],
        },
        CpuStrip {
            color: LINE_AMBER_SRGB,
            vertices: vec![end, tip(1.0)],
        },
        CpuStrip {
            color: LINE_AMBER_SRGB,
            vertices: vec![end, tip(-1.0)],
        },
    ]
}

/// Pack strips into one positions buffer, one per-vertex sRGB color buffer,
/// and the per-strip (first vertex, vertex count) draw ranges. Color bytes
/// repeat the strip color once per vertex, mirroring the point pipeline's
/// Rgba8Unorm layout (hardware-decoded in the vertex stage).
fn pack_strips(strips: &[CpuStrip]) -> (Vec<u8>, Vec<u8>, Vec<(u32, u32)>) {
    let vertex_count: usize = strips.iter().map(|strip| strip.vertices.len()).sum();
    let mut position_bytes = Vec::with_capacity(vertex_count * POSITION_STRIDE_BYTES as usize);
    let mut color_bytes = Vec::with_capacity(vertex_count * COLOR_STRIDE_BYTES as usize);
    let mut ranges: Vec<(u32, u32)> = Vec::with_capacity(strips.len());
    let mut start = 0u32;
    for strip in strips {
        // Defensive: an empty strip is never produced by the builders (their
        // invariant is ≥2 vertices per strip) and would draw nothing.
        if strip.vertices.is_empty() {
            continue;
        }
        let count = u32::try_from(strip.vertices.len()).expect(
            "a line strip cannot exceed u32::MAX vertices; the io size guards bound the \
             point count far below that",
        );
        start = start
            .checked_add(count)
            .expect("total line vertices exceed u32::MAX; the io size guards prevent this");
        for vertex in &strip.vertices {
            position_bytes.extend_from_slice(&vertex.x.to_le_bytes());
            position_bytes.extend_from_slice(&vertex.y.to_le_bytes());
            position_bytes.extend_from_slice(&vertex.z.to_le_bytes());
        }
        for _ in 0..strip.vertices.len() {
            color_bytes.extend_from_slice(&[strip.color.r, strip.color.g, strip.color.b, 255]);
        }
        ranges.push((start - count, count));
    }
    (position_bytes, color_bytes, ranges)
}

/// GPU handles of one uploaded line geometry: the positions and per-vertex
/// color buffers, the bind group referencing the renderer's scene-wide
/// view-projection uniform buffer, and the per-object appearance channel
/// ([`AppearanceGpu`], group 1). Owned by the caller — a path, frame, or
/// marker-arrow display behind an [`Arc`], or the viewport itself for the
/// persistent helper form of [`LinePipeline::with_capacity`]; dropping it
/// frees the buffers through wgpu's deferred destruction, exactly like
/// [`crate::render::renderer::PointCloudMesh`] (`renderer` module). The
/// appearance resources ride inside this struct, provisioned and dropped
/// together with the geometry (plan §3.1).
pub struct LineMesh {
    positions: wgpu::Buffer,
    colors: wgpu::Buffer,
    /// One (first vertex, vertex count) per `LineStrip` primitive; paint
    /// issues one draw call per entry. Empty when the CPU geometry was
    /// empty (e.g. an all-non-finite path) — paint then draws nothing.
    strips: Vec<(u32, u32)>,
    bind_group: wgpu::BindGroup,
    appearance: AppearanceGpu,
    /// Vertex capacity the `positions`/`colors` buffers were sized for: the
    /// vertex count of the upload, or the `with_capacity` preallocation for
    /// the persistent form. [`LinePipeline::update_mesh`] refuses refreshes
    /// beyond it, so the buffers are never recreated.
    vertex_capacity: u32,
    /// CPU staging buffers of the persistent form ([`LinePipeline::with_capacity`]),
    /// preallocated to the vertex capacity and reused by every
    /// [`LinePipeline::update_mesh`] refresh — a refresh allocates no
    /// staging and no GPU objects. Empty for upload-returned meshes, whose
    /// geometry never refreshes.
    staging_positions: Vec<u8>,
    staging_colors: Vec<u8>,
}

/// Owns the line pipeline and uploads line geometry; one instance per
/// renderer (see [`LinePipeline::new`]).
pub struct LinePipeline {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    appearance_bind_group_layout: wgpu::BindGroupLayout,
    uniform_buffer: wgpu::Buffer,
}

impl LinePipeline {
    /// Create the line pipeline against a [`Renderer`], reading every
    /// shared-depth parameter (depth format, sample count), the target
    /// format, and the scene's shared bind group layouts (scene + per-
    /// object appearance) and uniform buffer from it — the renderer is the
    /// single source for the whole scene (plan §3.3), so this pipeline can
    /// never disagree with the pass the host opens. Rebuild it whenever the
    /// renderer is rebuilt (target format, depth format, or sample count
    /// change).
    pub fn new(renderer: &Renderer) -> Self {
        let device = renderer.device();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("line"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(LINE_SHADER_SOURCE)),
        });

        let bind_group_layout = renderer.scene_bind_group_layout();
        let appearance_bind_group_layout = renderer.appearance_bind_group_layout();
        // Two bind groups per pipeline: group 0 the scene-wide view-proj
        // uniform, group 1 the per-object appearance uniform (spec §6).
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("line"),
            bind_group_layouts: &[bind_group_layout, appearance_bind_group_layout],
            push_constant_ranges: &[],
        });

        let vertex_buffer_layouts = line_vertex_buffer_layouts();

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("line"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &vertex_buffer_layouts,
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineStrip,
                ..Default::default()
            },
            // Shared depth policy for lines (spec §6): strict Less compare,
            // depth writes disabled, no polygon offset — see the module docs.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: renderer.depth_format(),
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: renderer.sample_count(),
                ..Default::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: renderer.target_format(),
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        Self {
            device: renderer.device().clone(),
            queue: renderer.queue().clone(),
            pipeline,
            bind_group_layout: renderer.scene_bind_group_layout().clone(),
            appearance_bind_group_layout: renderer.appearance_bind_group_layout().clone(),
            uniform_buffer: renderer.uniform_buffer().clone(),
        }
    }

    /// Upload a path (spec §7 F2) as its finite-run strips — one open
    /// polyline per run, in file order, in the default line amber. Records
    /// one created event for the path kind in the A6 handle ledger.
    pub fn upload_path(&self, data: &io::PathData) -> Arc<LineMesh> {
        counters::note_uploaded(DisplayKind::Path);
        self.upload_strips(&path_strips(data))
    }

    /// Upload a frame (spec §7 F3) as its three world-axis strips. Records
    /// one created event for the frame kind in the A6 handle ledger.
    pub fn upload_frame(&self, origin: Vec3, length: f32) -> Arc<LineMesh> {
        counters::note_uploaded(DisplayKind::Frame);
        self.upload_strips(&frame_strips(origin, length))
    }

    /// Upload a marker arrow (spec §7 F4) as its shaft plus two head lines.
    /// Records one created event for the marker kind in the A6 handle
    /// ledger.
    pub fn upload_arrow(&self, start: Vec3, end: Vec3) -> Arc<LineMesh> {
        counters::note_uploaded(DisplayKind::Marker);
        self.upload_strips(&arrow_strips(start, end))
    }

    fn upload_strips(&self, strips: &[CpuStrip]) -> Arc<LineMesh> {
        let (position_bytes, color_bytes, ranges) = pack_strips(strips);
        let total_vertices = u32::try_from(position_bytes.len() / POSITION_STRIDE_BYTES as usize)
            .expect(
                "more than u32::MAX line vertices cannot be drawn; the io size guards \
                     bound the point count far below that",
            );

        let positions = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("line.positions"),
            size: position_bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let colors = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("line.colors"),
            size: color_bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&positions, 0, &position_bytes);
        self.queue.write_buffer(&colors, 0, &color_bytes);

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("line"),
            layout: &self.bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.uniform_buffer.as_entire_binding(),
            }],
        });

        // The per-object appearance channel rides in the same handle
        // (plan §3.1); it starts neutral so the baked vertex colors show.
        let appearance = create_appearance_gpu(
            &self.device,
            &self.queue,
            &self.appearance_bind_group_layout,
            "line.appearance",
            &Appearance::DEFAULT,
        );

        Arc::new(LineMesh {
            positions,
            colors,
            strips: ranges,
            bind_group,
            appearance,
            vertex_capacity: total_vertices,
            staging_positions: Vec::new(),
            staging_colors: Vec::new(),
        })
    }

    /// Record the draws of one line mesh into an externally opened render
    /// pass (mirrors [`Renderer::paint`]: records commands only, never
    /// creates or submits a pass or encoder). Sets the pipeline, the mesh's
    /// bind groups (group 0: view-proj; group 1: the appearance uniform),
    /// and issues one `LineStrip` draw per strip range; a mesh whose CPU
    /// geometry was empty draws nothing.
    pub fn paint(&self, pass: &mut wgpu::RenderPass<'static>, mesh: &LineMesh) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &mesh.bind_group, &[]);
        pass.set_bind_group(1, &mesh.appearance.bind_group, &[]);
        pass.set_vertex_buffer(0, mesh.positions.slice(..));
        pass.set_vertex_buffer(1, mesh.colors.slice(..));
        for &(start, count) in &mesh.strips {
            pass.draw(start..start + count, 0..1);
        }
    }

    /// Create a persistent [`LineMesh`] whose vertex buffers are
    /// preallocated for `segments` two-vertex strips (2·segments vertices)
    /// — the viewport-helper form (ground grid, spec §6/plan §3.3). The
    /// buffers, bind groups, appearance uniform, and the CPU staging are
    /// all created once here and refreshed in place by
    /// [`LinePipeline::update_mesh`], which never provisions GPU objects —
    /// a per-frame refresh therefore never touches the A6 ledger (helper
    /// layers are not scene objects, spec §6) and never rebuilds anything.
    ///
    /// `segments` must cover the largest refresh the caller will issue; the
    /// grid module's [`super::grid::segment_capacity_bound`] exists for
    /// exactly this prebuild (the bound holds for every window radius up to
    /// the options' radius). The mesh is owned by the caller (viewport
    /// state) and starts empty — paint draws nothing until the first
    /// [`LinePipeline::update_mesh`].
    pub fn with_capacity(&self, segments: usize) -> LineMesh {
        let vertices = segments
            .checked_mul(2)
            .expect("a line mesh capacity of more than usize::MAX segments is not addressable");
        let vertices = u32::try_from(vertices).expect(
            "a persistent line mesh cannot exceed u32::MAX vertices; the grid capacity \
             bounds are far below that",
        );
        // Staging sized to the same vertex capacity in bytes: update_mesh
        // refreshes into these vectors and never reallocates.
        let vertex_capacity = usize::try_from(vertices)
            .expect("a vertex capacity of u32::MAX always fits usize on every supported target");
        let positions = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("line.persistent.positions"),
            size: u64::from(vertices) * POSITION_STRIDE_BYTES,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let colors = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("line.persistent.colors"),
            size: u64::from(vertices) * COLOR_STRIDE_BYTES,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("line.persistent"),
            layout: &self.bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.uniform_buffer.as_entire_binding(),
            }],
        });
        let appearance = create_appearance_gpu(
            &self.device,
            &self.queue,
            &self.appearance_bind_group_layout,
            "line.persistent.appearance",
            &Appearance::DEFAULT,
        );

        LineMesh {
            positions,
            colors,
            strips: Vec::with_capacity(segments),
            bind_group,
            appearance,
            vertex_capacity: vertices,
            staging_positions: Vec::with_capacity(vertex_capacity * POSITION_STRIDE_BYTES as usize),
            staging_colors: Vec::with_capacity(vertex_capacity * COLOR_STRIDE_BYTES as usize),
        }
    }

    /// Refresh the geometry of a persistent mesh in place — the ground-grid
    /// refresh path (plan §3.3): packs `segments` (each `[Vec3; 2]` becomes
    /// one two-vertex `LineStrip` in one color) into the mesh's preallocated
    /// staging and writes both vertex buffers with a single queue write
    /// each. No buffer, bind group, uniform, or pipeline is ever created
    /// here, and the mesh keeps every handle across calls — only
    /// `queue.write_buffer` touches the GPU (spec §6: 持久 LineMesh 容量预建
    /// + `queue.write_buffer` 就地刷新).
    ///
    /// Panics when `segments` need more vertices than the mesh was prebuilt
    /// for — choose the capacity with [`LinePipeline::with_capacity`] for
    /// the largest refresh (e.g. [`super::grid::segment_capacity_bound`] of
    /// the maximum camera window), not per refresh.
    pub fn update_mesh(&self, mesh: &mut LineMesh, segments: &[[Vec3; 2]], color: io::Color) {
        let vertices = u32::try_from(segments.len() * 2)
            .expect("a persistent mesh cannot refresh more than u32::MAX vertices");
        assert!(
            vertices <= mesh.vertex_capacity,
            "update_mesh needs {vertices} vertices but the mesh was prebuilt for {} — \
             refresh cannot exceed the with_capacity preallocation",
            mesh.vertex_capacity
        );

        pack_segments_into(
            segments,
            color,
            &mut mesh.staging_positions,
            &mut mesh.staging_colors,
            &mut mesh.strips,
        );
        self.queue
            .write_buffer(&mesh.positions, 0, &mesh.staging_positions);
        self.queue
            .write_buffer(&mesh.colors, 0, &mesh.staging_colors);
    }

    /// Refresh the appearance of one line mesh in place (plan §3.1): one
    /// 64-byte queue write into the mesh's preallocated appearance uniform.
    /// Never creates a buffer or bind group and never triggers a renderer
    /// or scene rebuild; the effect is visible on the next frame.
    pub fn set_appearance(&self, mesh: &LineMesh, appearance: &Appearance) {
        write_appearance(&self.queue, &mesh.appearance, appearance);
    }
}

/// Pack `segments` into caller-provided buffers: each `[Vec3; 2]` becomes
/// one two-vertex `LineStrip`, all in one sRGB `color`. The byte format is
/// identical to [`pack_strips`]'s (positions little-endian f32 triples,
/// per-vertex Rgba8Unorm color bytes), so the shader and the pipeline are
/// shared with the upload forms — a unit test pins the two packers against
/// each other. The three buffers are *cleared and refilled*, keeping their
/// capacity: [`LinePipeline::update_mesh`] preallocates them in
/// [`LinePipeline::with_capacity`] and a refresh never reallocates.
fn pack_segments_into(
    segments: &[[Vec3; 2]],
    color: io::Color,
    position_bytes: &mut Vec<u8>,
    color_bytes: &mut Vec<u8>,
    ranges: &mut Vec<(u32, u32)>,
) {
    position_bytes.clear();
    color_bytes.clear();
    ranges.clear();
    let mut start = 0u32;
    for segment in segments {
        let count = 2u32;
        start = start
            .checked_add(count)
            .expect("total line vertices exceed u32::MAX; the capacity guards prevent this");
        for vertex in segment {
            position_bytes.extend_from_slice(&vertex.x.to_le_bytes());
            position_bytes.extend_from_slice(&vertex.y.to_le_bytes());
            position_bytes.extend_from_slice(&vertex.z.to_le_bytes());
            color_bytes.extend_from_slice(&[color.r, color.g, color.b, 255]);
        }
        ranges.push((start - count, count));
    }
}

/// The line pipeline's vertex buffer layouts, in slot order: slot 0
/// positions (tightly packed `f32` triples), slot 1 colors (packed
/// Rgba8Unorm) — the same two shapes the point pipeline consumes, so the
/// layout constants are the shared ones of `renderer.rs`.
fn line_vertex_buffer_layouts() -> [wgpu::VertexBufferLayout<'static>; 2] {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_6;

    fn point(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3::new(x, y, z)
    }

    fn path_data(points: Vec<Vec3>) -> io::PathData {
        io::PathData {
            bounds: io::Aabb::from_points(&points),
            points,
        }
    }

    #[test]
    fn path_strips_split_non_finite_points_into_finite_runs() {
        // Runs: [a, b] and [c, d, e]; the NaN between them splits the
        // polyline (spec G1: the NaN stays in the data, geometry keeps only
        // drawable segments).
        let a = point(0.0, 0.0, 0.0);
        let b = point(1.0, 0.0, 0.0);
        let c = point(0.0, 1.0, 0.0);
        let d = point(1.0, 1.0, 0.0);
        let e = point(2.0, 1.0, 0.0);
        let strips = path_strips(&path_data(vec![a, b, Vec3::splat(f32::NAN), c, d, e]));

        assert_eq!(strips.len(), 2, "two finite runs, one strip each");
        assert_eq!(strips[0].vertices, [a, b]);
        assert_eq!(strips[1].vertices, [c, d, e]);
        for strip in &strips {
            assert_eq!(strip.color, LINE_AMBER_SRGB);
            assert!(strip.vertices.len() >= 2, "every strip holds ≥2 points");
        }
    }

    #[test]
    fn path_strips_drop_single_point_runs_that_cannot_segment() {
        // An isolated finite point between non-finite neighbors cannot form
        // a segment and is dropped; leading/trailing non-finite runs just
        // truncate.
        let a = point(0.0, 0.0, 0.0);
        let b = point(1.0, 0.0, 0.0);
        let c = point(2.0, 0.0, 0.0);
        let strips = path_strips(&path_data(vec![
            Vec3::splat(f32::NAN),
            a,
            Vec3::splat(f32::INFINITY),
            b,
            c,
            Vec3::splat(f32::NAN),
        ]));
        assert_eq!(strips.len(), 1, "only the two-point run survives");
        assert_eq!(strips[0].vertices, [b, c]);
        assert_eq!(strips[0].vertices.len(), 2);

        // A lone finite point between NaNs has nothing to segment: no strip.
        let strips = path_strips(&path_data(vec![
            Vec3::splat(f32::NAN),
            a,
            Vec3::splat(f32::NAN),
        ]));
        assert!(strips.is_empty());
    }

    #[test]
    fn path_strips_of_an_all_finite_or_all_invalid_path() {
        let a = point(0.0, 0.0, 0.0);
        let b = point(1.0, 0.0, 0.0);
        let strips = path_strips(&path_data(vec![a, b]));
        assert_eq!(strips.len(), 1);
        assert_eq!(strips[0].vertices, [a, b]);

        // All non-finite, or empty: no strip at all (a polyline needs ≥2
        // finite points; the parser already rejects such files).
        let invalid = path_data(vec![Vec3::splat(f32::NAN); 4]);
        assert!(path_strips(&invalid).is_empty());
        assert!(path_strips(&path_data(vec![])).is_empty());
    }

    #[test]
    fn frame_strips_draw_three_colored_world_axis_segments() {
        let origin = point(1.0, -2.0, 3.0);
        let strips = frame_strips(origin, 2.0);
        assert_eq!(strips.len(), 3);

        let expected = [
            (Vec3::X, AXIS_X_COLOR_SRGB),
            (Vec3::Y, AXIS_Y_COLOR_SRGB),
            (Vec3::Z, AXIS_Z_COLOR_SRGB),
        ];
        for (strip, (axis, color)) in strips.iter().zip(expected) {
            assert_eq!(
                strip.color, color,
                "axis colors follow the X/Y/Z convention"
            );
            assert_eq!(strip.vertices, [origin, origin + axis * 2.0]);
        }
    }

    #[test]
    fn axis_color_constants_match_002_a4_semantic_colors() {
        // Semantic lock (display-types spec A4, §7 F3): frame axes are
        // X red / Y green / Z blue as pure sRGB bytes — X=(255,0,0),
        // Y=(0,255,0), Z=(0,0,255), no gamma or alpha deviation. The app's
        // ui-blueprint palette (spec §6 A9) asserts its origin-axes token
        // against these `pub` constants, so this test pins them to the spec
        // semantics they stand in for.
        assert_eq!(AXIS_X_COLOR_SRGB, io::Color { r: 255, g: 0, b: 0 });
        assert_eq!(AXIS_Y_COLOR_SRGB, io::Color { r: 0, g: 255, b: 0 });
        assert_eq!(AXIS_Z_COLOR_SRGB, io::Color { r: 0, g: 0, b: 255 });
    }

    #[test]
    fn frame_strips_of_invalid_input_draw_nothing() {
        assert!(frame_strips(point(0.0, 0.0, 0.0), 0.0).is_empty());
        assert!(frame_strips(point(0.0, 0.0, 0.0), -1.0).is_empty());
        assert!(frame_strips(point(0.0, 0.0, 0.0), f32::NAN).is_empty());
        assert!(frame_strips(point(0.0, 0.0, 0.0), f32::INFINITY).is_empty());
        assert!(frame_strips(Vec3::splat(f32::NAN), 1.0).is_empty());
    }

    #[test]
    fn arrow_strips_build_a_shaft_and_two_symmetric_head_lines() {
        let start = point(0.0, 0.0, 0.0);
        let end = point(4.0, 0.0, 0.0);
        let strips = arrow_strips(start, end);
        assert_eq!(strips.len(), 3);

        // Strip 0 is the shaft itself, tip to tip.
        assert_eq!(strips[0].vertices, [start, end]);

        // Head strips: both start at the shaft end and end `0.25 · 4 = 1`
        // unit away, angled 30° off the reverse shaft direction (−X here).
        let head_length = 4.0 * ARROW_HEAD_FRACTION;
        for strip in &strips[1..] {
            assert_eq!(strip.color, LINE_AMBER_SRGB);
            assert_eq!(strip.vertices[0], end);
            let offset = strip.vertices[1] - end;
            assert!(
                (offset.length() - head_length).abs() < 1e-6,
                "head lines are exactly the head length long"
            );
            let direction = offset / offset.length();
            assert!(
                (direction.dot(-Vec3::X) - FRAC_PI_6.cos()).abs() < 1e-6,
                "head lines sit 30° off the reverse shaft"
            );
        }
        // Symmetry: the two head tips mirror across the shaft axis (their
        // midpoint lies on the reverse shaft ray).
        let tip_a = strips[1].vertices[1];
        let tip_b = strips[2].vertices[1];
        let midpoint = (tip_a + tip_b) * 0.5;
        assert!(
            midpoint.abs_diff_eq(end - Vec3::X * (head_length * FRAC_PI_6.cos()), 1e-5),
            "head tips mirror across the shaft line"
        );
    }

    #[test]
    fn arrow_strips_handle_vertical_shafts_and_short_arrows() {
        // Shaft along world Y: the world-Y spread reference degenerates and
        // must fall back to world X — the head still opens cleanly.
        let strips = arrow_strips(point(0.0, 0.0, 0.0), point(0.0, 4.0, 0.0));
        assert_eq!(strips.len(), 3);
        for strip in &strips[1..] {
            assert!(strip.vertices[1].is_finite());
            let offset = (strip.vertices[1] - strip.vertices[0]) / 1.0;
            assert!(
                (offset.y + FRAC_PI_6.cos()).abs() < 1e-5,
                "head tips aim back down the −Y shaft"
            );
        }

        // Zero-length and non-finite arrows draw nothing.
        assert!(arrow_strips(point(1.0, 1.0, 1.0), point(1.0, 1.0, 1.0)).is_empty());
        assert!(arrow_strips(Vec3::splat(f32::NAN), point(1.0, 1.0, 1.0)).is_empty());
        assert!(arrow_strips(point(0.0, 0.0, 0.0), Vec3::splat(f32::INFINITY)).is_empty());
    }

    #[test]
    fn pack_strips_flattens_strips_and_repeats_colors_per_vertex() {
        let a = point(0.0, 0.0, 0.0);
        let b = point(1.0, 0.0, 0.0);
        let c = point(2.0, 0.0, 0.0);
        let strips = [
            CpuStrip {
                color: AXIS_X_COLOR_SRGB,
                vertices: vec![a, b],
            },
            CpuStrip {
                color: LINE_AMBER_SRGB,
                vertices: vec![b, c, a],
            },
        ];
        let (position_bytes, color_bytes, ranges) = pack_strips(&strips);

        assert_eq!(position_bytes.len(), 5 * POSITION_STRIDE_BYTES as usize);
        assert_eq!(color_bytes.len(), 5 * COLOR_STRIDE_BYTES as usize);
        assert_eq!(ranges, [(0, 2), (2, 3)]);

        // Colors repeat per vertex of their strip.
        assert_eq!(&color_bytes[0..4], &[255, 0, 0, 255]);
        assert_eq!(&color_bytes[4..8], &[255, 0, 0, 255]);
        assert_eq!(&color_bytes[8..12], &[255, 196, 100, 255]);

        // Positions keep vertex order across strips.
        assert_eq!(
            &position_bytes[24..28],
            &b.x.to_le_bytes(),
            "strip 2 starts at vertex 2"
        );
    }

    #[test]
    fn line_pipeline_takes_two_vertex_buffers_positions_then_colors() {
        let layouts = line_vertex_buffer_layouts();
        assert_eq!(layouts.len(), 2);
        assert_eq!(layouts[0].array_stride, POSITION_STRIDE_BYTES);
        assert_eq!(layouts[0].attributes, &POSITION_ATTRIBUTES[..]);
        assert_eq!(layouts[1].array_stride, COLOR_STRIDE_BYTES);
        assert_eq!(layouts[1].attributes, &COLOR_ATTRIBUTES[..]);
    }

    #[test]
    fn line_wgsl_compiles_headlessly() {
        let module = naga::front::wgsl::parse_str(LINE_SHADER_SOURCE)
            .unwrap_or_else(|error| panic!("line.wgsl failed to parse:\n{error}"));
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator
            .validate(&module)
            .unwrap_or_else(|error| panic!("line.wgsl failed naga validation:\n{error}"));
    }

    #[test]
    fn shader_and_cpu_color_curve_share_constants() {
        // Both srgb_to_linear implementations must move together (same
        // pinning test as the point cloud shader in renderer.rs).
        for constant in ["0.04045", "12.92", "0.055", "1.055", "2.4"] {
            assert!(
                LINE_SHADER_SOURCE.contains(constant),
                "line.wgsl is missing sRGB constant {constant}"
            );
        }
    }

    #[test]
    fn persistent_packer_matches_the_upload_packer_byte_for_byte() {
        // The persistent form (`pack_segments_into`) and the upload form
        // (`pack_strips`) must produce identical bytes and ranges for the
        // same geometry, because both feed the same vertex buffers, bind
        // groups, and shader — one pipeline, two fill paths. Each segment
        // becomes one canonical two-vertex strip.
        let segments = [
            [point(0.0, 0.0, 0.0), point(1.0, 0.0, 0.0)],
            [point(1.0, 0.0, 0.0), point(1.0, 1.0, 0.0)],
            [point(1.0, 1.0, 0.0), point(0.0, 1.0, 0.0)],
        ];
        let color = io::Color {
            r: 90,
            g: 160,
            b: 220,
        };
        let mut positions = Vec::with_capacity(segments.len() * 2 * POSITION_STRIDE_BYTES as usize);
        let mut colors = Vec::with_capacity(segments.len() * 2 * COLOR_STRIDE_BYTES as usize);
        let mut ranges = Vec::with_capacity(segments.len());
        pack_segments_into(&segments, color, &mut positions, &mut colors, &mut ranges);

        assert_eq!(
            ranges,
            [(0, 2), (2, 2), (4, 2)],
            "one two-vertex strip per segment"
        );
        assert_eq!(
            positions.len(),
            3 * 2 * POSITION_STRIDE_BYTES as usize,
            "two vertices per segment"
        );
        assert_eq!(
            colors.len(),
            3 * 2 * COLOR_STRIDE_BYTES as usize,
            "one color byte quadruple per vertex"
        );

        // The upload packer of the same strips: identical bytes.
        let strips: Vec<CpuStrip> = segments
            .iter()
            .map(|segment| CpuStrip {
                color,
                vertices: segment.to_vec(),
            })
            .collect();
        let (expected_positions, expected_colors, expected_ranges) = pack_strips(&strips);
        assert_eq!(positions, expected_positions);
        assert_eq!(colors, expected_colors);
        assert_eq!(ranges, expected_ranges);

        // Vertex colors repeat the strip color per vertex, like the upload
        // form (Rgba8Unorm sRGB bytes, opaque alpha).
        assert_eq!(&colors[0..4], &[90, 160, 220, 255]);
        assert_eq!(
            &colors[8..12],
            &[90, 160, 220, 255],
            "vertex 2 is strip 2's first vertex"
        );
    }

    #[test]
    fn persistent_refresh_reuses_its_staging_buffers() {
        // The T6 zero-allocation claim, tested headlessly on the exact
        // buffers `update_mesh` refreshes (CI has no GPU device to create a
        // real `LineMesh`): `with_capacity` preallocates the staging
        // vectors; repeated refresh — including shrinking refreshes — must
        // keep each allocation's identity and capacity and only rewrite
        // contents, exactly like the GPU-side writes of `update_mesh`,
        // which create no buffer, bind group, or staging of their own.
        let segments_a = [
            [point(0.0, 0.0, 0.0), point(1.0, 0.0, 0.0)],
            [point(1.0, 0.0, 0.0), point(1.0, 1.0, 0.0)],
            [point(1.0, 1.0, 0.0), point(0.0, 1.0, 0.0)],
        ];
        let segments_b = [[point(5.0, 5.0, 0.0), point(6.0, 5.0, 0.0)]];
        let color_a = io::Color {
            r: 90,
            g: 160,
            b: 220,
        };
        let color_b = AXIS_Y_COLOR_SRGB;

        // Staging sized as `with_capacity(segments_a.len())` would size it.
        let mut positions =
            Vec::with_capacity(segments_a.len() * 2 * POSITION_STRIDE_BYTES as usize);
        let mut colors = Vec::with_capacity(segments_a.len() * 2 * COLOR_STRIDE_BYTES as usize);
        let mut ranges = Vec::with_capacity(segments_a.len());

        pack_segments_into(
            &segments_a,
            color_a,
            &mut positions,
            &mut colors,
            &mut ranges,
        );
        let position_ptr = positions.as_ptr();
        let color_ptr = colors.as_ptr();
        let ranges_ptr = ranges.as_ptr();
        let position_capacity = positions.capacity();
        let color_capacity = colors.capacity();
        let ranges_capacity = ranges.capacity();

        // 32 refresh rounds alternating a full and a shrinking refresh —
        // the camera-motion pattern of the ground grid (plan §3.3).
        for _ in 0..32 {
            pack_segments_into(
                &segments_a,
                color_a,
                &mut positions,
                &mut colors,
                &mut ranges,
            );
            pack_segments_into(
                &segments_b,
                color_b,
                &mut positions,
                &mut colors,
                &mut ranges,
            );
        }

        assert_eq!(
            positions.as_ptr(),
            position_ptr,
            "refresh must never reallocate the position staging"
        );
        assert_eq!(colors.as_ptr(), color_ptr);
        assert_eq!(ranges.as_ptr(), ranges_ptr);
        assert_eq!(positions.capacity(), position_capacity);
        assert_eq!(colors.capacity(), color_capacity);
        assert_eq!(ranges.capacity(), ranges_capacity);

        // Contents are exactly the last (shrunk) refresh: the two green
        // vertices of segments_b.
        let (expected_positions, expected_colors, expected_ranges) = pack_strips(&[CpuStrip {
            color: color_b,
            vertices: segments_b[0].to_vec(),
        }]);
        assert_eq!(positions, expected_positions);
        assert_eq!(colors, expected_colors);
        assert_eq!(ranges, expected_ranges);
    }

    #[test]
    fn empty_and_exact_capacity_refreshes_pack_cleanly() {
        // An empty refresh clears the mesh to nothing — the grid's
        // "no grid" state — without touching the preallocated staging.
        let mut positions = Vec::with_capacity(8 * POSITION_STRIDE_BYTES as usize);
        let mut colors = Vec::with_capacity(8 * COLOR_STRIDE_BYTES as usize);
        let mut ranges = Vec::with_capacity(4);
        pack_segments_into(
            &[],
            AXIS_X_COLOR_SRGB,
            &mut positions,
            &mut colors,
            &mut ranges,
        );
        assert!(positions.is_empty() && colors.is_empty() && ranges.is_empty());
        assert!(positions.capacity() >= 8 * POSITION_STRIDE_BYTES as usize);

        // A refresh of exactly `capacity` segments needs exactly 2·capacity
        // vertices — the boundary `update_mesh`'s vertex guard admits
        // (anything beyond it would trip the assert and means the caller
        // under-provisioned `with_capacity`).
        let capacity = 1000usize;
        let segments: Vec<[Vec3; 2]> = (0..capacity)
            .map(|k| [point(k as f32, 0.0, 0.0), point(k as f32, 1.0, 0.0)])
            .collect();
        pack_segments_into(
            &segments,
            LINE_AMBER_SRGB,
            &mut positions,
            &mut colors,
            &mut ranges,
        );
        assert_eq!(
            positions.len(),
            capacity * 2 * POSITION_STRIDE_BYTES as usize
        );
        assert_eq!(colors.len(), capacity * 2 * COLOR_STRIDE_BYTES as usize);
        assert_eq!(ranges.len(), capacity);
        assert_eq!(ranges.first(), Some(&(0, 2)));
        assert_eq!(ranges.last(), Some(&((2 * (capacity - 1)) as u32, 2)));
        // 2·capacity vertices stay inside the 2·capacity vertex prebuild.
        let vertex_capacity = capacity * 2;
        assert!(segments.len() * 2 <= vertex_capacity);
    }

    #[test]
    fn grid_sweep_fits_a_mesh_prebuilt_for_the_maximum_window() {
        // The T13 integration contract, headless: the viewport provisions
        // ONE persistent line mesh with
        // `with_capacity(segment_capacity_bound(&maximum_options))` and
        // refreshes it every frame with `grid_strips` output as the camera
        // moves. Any window radius up to the prebuilt maximum must fit that
        // capacity, and the refresh must not reallocate its staging (spec
        // §6: 容量预建 + queue.write_buffer 就地刷新).
        use crate::render::grid::{GridOptions, GridView, grid_strips, segment_capacity_bound};

        let grid_color = io::Color {
            r: 210,
            g: 210,
            b: 215,
        };
        let bound = segment_capacity_bound(&GridOptions::new(1.0, 1000.0, 2.0));
        assert!(bound > 0);

        // Staging exactly as `with_capacity(bound)` provisions it: two
        // vertices per prebuilt segment.
        let mut positions = Vec::with_capacity(bound * 2 * POSITION_STRIDE_BYTES as usize);
        let mut colors = Vec::with_capacity(bound * 2 * COLOR_STRIDE_BYTES as usize);
        let mut ranges = Vec::with_capacity(bound);
        let position_ptr = positions.as_ptr();
        let color_ptr = colors.as_ptr();

        let radii = [0.4, 4.4, 10.0, 60.0, 100.0, 250.0, 452.6, 1000.0];
        let centers = [[0.0f32, 0.0], [0.33, 0.77], [12.3, -45.6], [-999.5, 777.7]];
        for [cx, cy] in centers {
            for radius in radii {
                let view =
                    GridView::new(Vec3::new(cx, cy, 0.0), GridOptions::new(1.0, radius, 2.0));
                let strips = grid_strips(&view);
                assert!(!strips.is_empty());

                pack_segments_into(
                    &strips,
                    grid_color,
                    &mut positions,
                    &mut colors,
                    &mut ranges,
                );
                assert_eq!(
                    positions.len(),
                    strips.len() * 2 * POSITION_STRIDE_BYTES as usize
                );
                assert_eq!(colors.len(), strips.len() * 2 * COLOR_STRIDE_BYTES as usize);
                assert_eq!(ranges.len(), strips.len());
                assert_eq!(
                    positions.as_ptr(),
                    position_ptr,
                    "a within-bound refresh must never reallocate the staging"
                );
                assert_eq!(colors.as_ptr(), color_ptr);
                for (index, &(start, count)) in ranges.iter().enumerate() {
                    assert_eq!(
                        (start, count),
                        (u32::try_from(index * 2).unwrap(), 2),
                        "grid segments pack as canonical two-vertex strips"
                    );
                }
                // `update_mesh`'s vertex guard: 2·segments ≤ 2·bound —
                // proven here at the byte level: every grid output fits the
                // staging the prebuilt mesh would carry.
                assert!(positions.len() <= positions.capacity());
            }
        }
    }
}
