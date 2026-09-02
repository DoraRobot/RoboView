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
    COLOR_ATTRIBUTES, COLOR_STRIDE_BYTES, POSITION_ATTRIBUTES, POSITION_STRIDE_BYTES, Renderer,
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

/// Frame axis colors as sRGB bytes: X red, Y green, Z blue (spec §7 F3).
const AXIS_X_COLOR_SRGB: io::Color = io::Color { r: 255, g: 0, b: 0 };
const AXIS_Y_COLOR_SRGB: io::Color = io::Color { r: 0, g: 255, b: 0 };
const AXIS_Z_COLOR_SRGB: io::Color = io::Color { r: 0, g: 0, b: 255 };

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
/// color buffers plus the bind group referencing the renderer's scene-wide
/// view-projection uniform buffer. Owned by the caller (a path, frame, or
/// marker-arrow display behind an [`Arc`]); dropping it frees the buffers
/// through wgpu's deferred destruction, exactly like
/// [`crate::render::renderer::PointCloudMesh`] (`renderer` module).
pub struct LineMesh {
    positions: wgpu::Buffer,
    colors: wgpu::Buffer,
    /// One (first vertex, vertex count) per `LineStrip` primitive; paint
    /// issues one draw call per entry. Empty when the CPU geometry was
    /// empty (e.g. an all-non-finite path) — paint then draws nothing.
    strips: Vec<(u32, u32)>,
    bind_group: wgpu::BindGroup,
}

/// Owns the line pipeline and uploads line geometry; one instance per
/// renderer (see [`LinePipeline::new`]).
pub struct LinePipeline {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buffer: wgpu::Buffer,
}

impl LinePipeline {
    /// Create the line pipeline against a [`Renderer`], reading every
    /// shared-depth parameter (depth format, sample count), the target
    /// format, and the scene's shared bind group layout and uniform buffer
    /// from it — the renderer is the single source for the whole scene
    /// (plan §3.3), so this pipeline can never disagree with the pass the
    /// host opens. Rebuild it whenever the renderer is rebuilt (target
    /// format, depth format, or sample count change).
    pub fn new(renderer: &Renderer) -> Self {
        let device = renderer.device();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("line"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(LINE_SHADER_SOURCE)),
        });

        let bind_group_layout = renderer.scene_bind_group_layout();
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("line"),
            bind_group_layouts: &[bind_group_layout],
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

        Arc::new(LineMesh {
            positions,
            colors,
            strips: ranges,
            bind_group,
        })
    }

    /// Record the draws of one line mesh into an externally opened render
    /// pass (mirrors [`Renderer::paint`]: records commands only, never
    /// creates or submits a pass or encoder). Issues one `LineStrip` draw
    /// per strip range; a mesh whose CPU geometry was empty draws nothing.
    pub fn paint(&self, pass: &mut wgpu::RenderPass<'static>, mesh: &LineMesh) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &mesh.bind_group, &[]);
        pass.set_vertex_buffer(0, mesh.positions.slice(..));
        pass.set_vertex_buffer(1, mesh.colors.slice(..));
        for &(start, count) in &mesh.strips {
            pass.draw(start..start + count, 0..1);
        }
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
}
