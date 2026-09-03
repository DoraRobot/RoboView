//! Mesh pipeline (display-types spec §7 F1, §6): triangle rendering with
//! CPU-computed face normals, a per-object face color uniform, and a
//! shared-depth polygon offset. The unlit face color moved from a WGSL
//! constant (`FACE_COLOR`) to the per-object appearance channel
//! (ui-blueprint spec §6, plan §3.1) — every mesh upload provisions a
//! 64-byte appearance uniform (renderer.rs) whose albedo is the flat face
//! color, so a mesh recoloring or the selection highlight is one in-place
//! queue write, never a re-upload or rebuild.
//!
//! The upload side expands every face into three standalone vertices (no
//! index buffer — the spec's ruling: "face normals are computed CPU-side,
//! with vertex duplication"): for each face the CPU computes the geometric
//! normal `(b − a) × (c − a)`, normalizes it, and pushes it once per corner.
//! Degenerate faces — corners out of range or non-finite (spec G1 data is
//! kept but defended against), or a zero/non-finite cross product (duplicate
//! or collinear corners) — are skipped and counted, so every uploaded vertex
//! is finite and carries a unit normal.
//!
//! Files without faces (`MeshData::indices` is `None`, spec §7 F1: the whole
//! file shows as a scatter) upload through the point cloud geometry shape
//! instead ([`MeshGpu::Scatter`]): the display kind stays a mesh, but its
//! GPU form is drawn by the point pipeline, whose WGSL already escapes
//! non-finite points (spec G1: non-finite vertices are kept in the data and
//! clipped in the renderer).
//!
//! The pipeline joins the scene family under the rules of `render/mod.rs`:
//! it is built from a [`Renderer`] (device, queue, target/depth formats,
//! sample count, and the shared view-projection bind group layout and
//! uniform buffer all come from it), so it can never disagree with the
//! render pass the host opens, and it writes depth with a strict Less
//! compare plus a positive polygon offset.
//!
//! # Depth bias constant table (calibrated against the M3 protocol)
//!
//! | Parameter | Value | Meaning |
//! |---|---|---|
//! | constant | 4 | Depth units (each ≈ 2⁻²⁴ of the depth range for a depth-24 buffer): pushes mesh fragments ~2.4e-7 farther |
//! | slope_scale | 1.0 | Multiplies the face's depth slope (max |dz/dx|, |dz/dy|) before adding, scaled by the same depth unit |
//! | clamp | 0.0 | No clamp on the combined bias (wgpu: 0 = unbounded) |
//!
//! The bias is positive, so coplanar points and lines (zero bias, written by
//! their own pipelines) stay nearer and win the strict Less compare — the
//! spec's "push the mesh away" policy. plan.md records that these values are
//! the M3 calibration entry point: if the protocol's probe scene fails, this
//! table is what the calibration round adjusts, not the depth compare.

use std::borrow::Cow;
use std::sync::Arc;

use glam::Vec3;

use super::counters;
use super::renderer::{
    Appearance, AppearanceGpu, POSITION_ATTRIBUTES, POSITION_STRIDE_BYTES, PointCloudMesh,
    Renderer, create_appearance_gpu, pack_colors, pack_positions, write_appearance,
};
use crate::displays::DisplayKind;
use crate::io;

/// Embedded WGSL source of the mesh pipeline. Compiled headlessly against
/// naga in the unit tests; the same naga major version validates it again
/// inside wgpu when the pipeline is created.
const MESH_SHADER_SOURCE: &str = include_str!("../../assets/shaders/mesh.wgsl");

/// Byte stride of one normal vertex: x, y, z as three `f32`.
const NORMAL_STRIDE_BYTES: u64 = 12;

/// Vertex attribute of the normal buffer (vertex slot 1): `x y z` at shader
/// location 1 (the shader declares it as the face-normal input; the unlit
/// policy leaves it unconsumed, see `assets/shaders/mesh.wgsl`).
const NORMAL_ATTRIBUTES: [wgpu::VertexAttribute; 1] = [wgpu::VertexAttribute {
    format: wgpu::VertexFormat::Float32x3,
    offset: 0,
    shader_location: 1,
}];

/// Depth bias of the mesh pipeline, positive to push the mesh away from
/// equal-depth geometry (see the constant table in the module docs).
const DEPTH_BIAS_CONSTANT: i32 = 4;
const DEPTH_BIAS_SLOPE_SCALE: f32 = 1.0;
const DEPTH_BIAS_CLAMP: f32 = 0.0;

/// Default face color of the unlit mesh pipeline as linear light, opaque —
/// the former WGSL `FACE_COLOR` constant, now the CPU-side albedo of the
/// default appearance every surface-mesh upload provisions (the shader
/// reads the uniform; see the module docs). Linear (0.7, 0.75, 0.8) ≈ sRGB
/// (0.854, 0.881, 0.906), a light neutral gray.
const DEFAULT_MESH_FACE_COLOR: [f32; 4] = [0.7, 0.75, 0.8, 1.0];

/// The mesh pipeline's vertex buffer layouts, in slot order: slot 0
/// positions (tightly packed `f32` triples), slot 1 face normals (same
/// layout). The slot index is the `set_vertex_buffer` slot used in
/// [`MeshPipeline::paint`].
fn mesh_vertex_buffer_layouts() -> [wgpu::VertexBufferLayout<'static>; 2] {
    [
        wgpu::VertexBufferLayout {
            array_stride: POSITION_STRIDE_BYTES,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &POSITION_ATTRIBUTES,
        },
        wgpu::VertexBufferLayout {
            array_stride: NORMAL_STRIDE_BYTES,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &NORMAL_ATTRIBUTES,
        },
    ]
}

/// Result of the CPU-side face expansion of [`expand_faces`].
struct ExpandedFaces {
    /// Three duplicated corner positions per kept face, in face order.
    corners: Vec<Vec3>,
    /// The kept faces' unit normals, repeated once per corner of their face.
    normals: Vec<Vec3>,
    /// Faces skipped because their geometry cannot be drawn (out-of-range or
    /// non-finite corners, collinear/duplicate corners, a trailing partial
    /// index chunk). Not read by the pipeline itself; the unit tests assert
    /// the skip accounting against hand-built inputs.
    #[cfg_attr(not(test), allow(dead_code))]
    degenerate_faces: usize,
}

/// Expand triangle indices into duplicated, normal-carrying vertices (spec
/// §6: CPU face normals, no index buffer).
///
/// For each face `(a, b, c)` the geometric normal is `(b − a) × (c − a)`
/// (right-handed, matching the scene's right-handed coordinate convention),
/// normalized to unit length and repeated on all three corners. A face is
/// skipped — and counted — when any corner reference is out of range or its
/// position is not finite (spec G1: non-finite data is kept but defended
/// against; a NaN corner would make the normal NaN and poison the fragment
/// color), or when the cross product is exactly zero or not finite
/// (duplicate corners, collinear corners). Near-zero but finite cross
/// products are kept: normalization is well defined for them, only the
/// accuracy degrades.
fn expand_faces(positions: &[Vec3], indices: &[u32]) -> ExpandedFaces {
    let mut corners = Vec::with_capacity(indices.len());
    let mut normals = Vec::with_capacity(indices.len());
    let mut degenerate_faces = 0;
    for chunk in indices.chunks(3) {
        let mut face = [Vec3::ZERO; 3];
        let mut valid = true;
        if chunk.len() != 3 {
            degenerate_faces += 1; // trailing partial chunk (hand-built data)
            continue;
        }
        for (slot, &index) in chunk.iter().enumerate() {
            let Some(corner) = positions.get(index as usize) else {
                valid = false;
                break;
            };
            if !corner.is_finite() {
                valid = false;
                break;
            }
            face[slot] = *corner;
        }
        if !valid {
            degenerate_faces += 1;
            continue;
        }
        let normal = (face[1] - face[0]).cross(face[2] - face[0]);
        if !normal.is_finite() || normal.length_squared() == 0.0 {
            degenerate_faces += 1;
            continue;
        }
        let normal = normal.normalize();
        corners.extend_from_slice(&face);
        normals.extend([normal; 3]);
    }
    ExpandedFaces {
        corners,
        normals,
        degenerate_faces,
    }
}

/// GPU handles of one uploaded surface mesh: its two vertex buffers (slot 0
/// duplicated face corners, slot 1 their unit face normals), the bind group
/// referencing the renderer's scene-wide view-projection uniform buffer,
/// and the per-object appearance channel ([`AppearanceGpu`]) whose albedo
/// is the face color. Owned by the caller (a mesh display behind an
/// [`Arc`]); dropping it frees the buffers through wgpu's deferred
/// destruction, exactly like [`PointCloudMesh`]. The appearance resources
/// ride inside this struct, so they are provisioned and dropped together
/// with the geometry (plan §3.1).
pub struct MeshMesh {
    positions: wgpu::Buffer,
    normals: wgpu::Buffer,
    /// Number of vertices: three per kept face (`0` for an all-degenerate
    /// mesh — the draw call is then a harmless no-op).
    count: u32,
    bind_group: wgpu::BindGroup,
    appearance: AppearanceGpu,
}

/// The uploaded GPU form of a mesh display (spec §7 F1): one of two shapes,
/// chosen by whether the file had `f` records.
pub enum MeshGpu {
    /// Files with faces: duplicated vertices plus CPU face normals, drawn by
    /// [`MeshPipeline::paint`].
    Faces(Arc<MeshMesh>),
    /// Files without faces: the whole file drawn as points through the point
    /// pipeline ([`Renderer::paint`] with the inner [`PointCloudMesh`]) —
    /// scatter mode reuses the point cloud geometry shape as uploaded.
    Scatter(Arc<PointCloudMesh>),
}

/// Owns the triangle pipeline and uploads mesh geometry; one instance per
/// renderer (see [`MeshPipeline::new`]).
pub struct MeshPipeline {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    appearance_bind_group_layout: wgpu::BindGroupLayout,
    uniform_buffer: wgpu::Buffer,
}

impl MeshPipeline {
    /// Create the mesh pipeline against a [`Renderer`], reading every
    /// shared-depth parameter (depth format, sample count), the target
    /// format, and the scene's shared bind group layout and uniform buffer
    /// from it — the renderer is the single source for the whole scene
    /// (plan §3.3), so this pipeline can never disagree with the pass the
    /// host opens. Rebuild it whenever the renderer is rebuilt (target
    /// format, depth format, or sample count change).
    pub fn new(renderer: &Renderer) -> Self {
        let device = renderer.device();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(MESH_SHADER_SOURCE)),
        });

        let bind_group_layout = renderer.scene_bind_group_layout();
        let appearance_bind_group_layout = renderer.appearance_bind_group_layout();
        // Two bind groups per pipeline: group 0 the scene-wide view-proj
        // uniform, group 1 the per-object appearance uniform (spec §6).
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh"),
            bind_group_layouts: &[bind_group_layout, appearance_bind_group_layout],
            push_constant_ranges: &[],
        });

        let vertex_buffer_layouts = mesh_vertex_buffer_layouts();

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &vertex_buffer_layouts,
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                // Double-sided: no culling (spec §6), so back faces of a
                // closed mesh stay visible when the camera is behind them.
                ..Default::default()
            },
            // Shared depth (spec §6): write with a strict Less compare and
            // the polygon offset of the module constant table, which pushes
            // the mesh away from equal-depth points and lines.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: renderer.depth_format(),
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState {
                    constant: DEPTH_BIAS_CONSTANT,
                    slope_scale: DEPTH_BIAS_SLOPE_SCALE,
                    clamp: DEPTH_BIAS_CLAMP,
                },
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

    /// Upload one mesh data set and return its GPU form — the single branch
    /// point between the surface and the scatter shape (spec §7 F1). Called
    /// once per data replacement from the host's prepare stage; the data is
    /// static, so there is no per-frame upload. Records one created event in
    /// the A6 handle ledger.
    pub fn upload(&self, data: &io::MeshData) -> MeshGpu {
        counters::note_uploaded(DisplayKind::Mesh);
        match data.indices.as_deref() {
            Some(indices) => MeshGpu::Faces(self.upload_faces(&data.positions, indices)),
            None => MeshGpu::Scatter(self.upload_scatter(&data.positions)),
        }
    }

    /// Upload the triangle form: expand every face into duplicated corners
    /// with CPU-computed unit normals (degenerate faces skipped and counted
    /// inside [`expand_faces`]), pack the two vertex buffers, and bind the
    /// shared view-projection uniform.
    fn upload_faces(&self, positions: &[Vec3], indices: &[u32]) -> Arc<MeshMesh> {
        let expanded = expand_faces(positions, indices);
        let position_bytes = pack_positions(&expanded.corners);
        let normal_bytes = pack_positions(&expanded.normals);
        let count = u32::try_from(expanded.corners.len()).expect(
            "more than u32::MAX mesh vertices cannot be drawn in one call; the io size \
             guards bound the face count far below that",
        );

        let positions = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh.positions"),
            size: position_bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let normals = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh.normals"),
            size: normal_bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&positions, 0, &position_bytes);
        self.queue.write_buffer(&normals, 0, &normal_bytes);

        let bind_group = self.bind_group();

        // The per-object appearance channel rides in the same handle; its
        // default albedo reproduces the former WGSL FACE_COLOR constant, so
        // an upload without appearance calls looks exactly as before.
        let appearance = create_appearance_gpu(
            &self.device,
            &self.queue,
            &self.appearance_bind_group_layout,
            "mesh.appearance",
            &Appearance::new(DEFAULT_MESH_FACE_COLOR, 0),
        );

        Arc::new(MeshMesh {
            positions,
            normals,
            count,
            bind_group,
            appearance,
        })
    }

    /// Upload the scatter form: the file's vertices as points with the
    /// default point color, shaped exactly like a point cloud upload
    /// (positions packed, default colors) so the point pipeline can draw
    /// them. Non-finite vertices are uploaded unchanged: the point cloud
    /// WGSL already escapes them out of the clip volume (spec G1: retained,
    /// clipped in the renderer).
    fn upload_scatter(&self, positions: &[Vec3]) -> Arc<PointCloudMesh> {
        let count = u32::try_from(positions.len()).expect(
            "more than u32::MAX scatter vertices cannot be drawn in one call; the io size \
             guards bound the vertex count far below that",
        );
        let position_bytes = pack_positions(positions);
        let color_bytes = pack_colors(positions.len(), None);

        let positions = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh.scatter.positions"),
            size: position_bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let colors = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh.scatter.colors"),
            size: color_bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&positions, 0, &position_bytes);
        self.queue.write_buffer(&colors, 0, &color_bytes);

        let bind_group = self.bind_group();

        // Scatter points have no per-vertex semantic colors of their own
        // (uploaded with the default point color), so their appearance
        // channel starts neutral like a point cloud's.
        let appearance = create_appearance_gpu(
            &self.device,
            &self.queue,
            &self.appearance_bind_group_layout,
            "mesh.scatter.appearance",
            &Appearance::DEFAULT,
        );

        Arc::new(PointCloudMesh::from_parts(
            positions, colors, count, bind_group, appearance,
        ))
    }

    /// Bind the scene-wide view-projection uniform buffer (binding 0) as the
    /// per-mesh bind group, against the scene's shared bind group layout.
    fn bind_group(&self) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh"),
            layout: &self.bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.uniform_buffer.as_entire_binding(),
            }],
        })
    }

    /// Record the draw of one surface mesh into an externally opened render
    /// pass (mirrors [`crate::render::Renderer::paint`]: records commands
    /// only, never creates or submits a pass or encoder). Sets the pipeline,
    /// the mesh's bind groups (group 0: view-proj; group 1: the appearance
    /// uniform whose albedo is the face color), the two vertex buffers, and
    /// draws all duplicated vertices as one triangle-list instance; a mesh
    /// whose upload kept no face draws nothing.
    pub fn paint(&self, pass: &mut wgpu::RenderPass<'static>, mesh: &MeshMesh) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &mesh.bind_group, &[]);
        pass.set_bind_group(1, &mesh.appearance.bind_group, &[]);
        pass.set_vertex_buffer(0, mesh.positions.slice(..));
        pass.set_vertex_buffer(1, mesh.normals.slice(..));
        pass.draw(0..mesh.count, 0..1);
    }

    /// Refresh the face color and markers of one surface mesh in place
    /// (plan §3.1): one 64-byte queue write into the mesh's preallocated
    /// appearance uniform — recoloring (002 property editing) and the 004
    /// selection highlight never re-upload geometry or rebuild anything.
    /// Scatter-shaped meshes ([`MeshGpu::Scatter`]) are drawn by the point
    /// pipeline and are updated through [`Renderer::set_appearance`]
    /// instead.
    pub fn set_appearance(&self, mesh: &MeshMesh, appearance: &Appearance) {
        write_appearance(&self.queue, &mesh.appearance, appearance);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3::new(x, y, z)
    }

    #[test]
    fn expand_faces_computes_right_handed_unit_normals_per_corner() {
        // Counter-clockwise in a right-handed y-up world: X × Y = +Z.
        let positions = [
            point(0.0, 0.0, 0.0), // a
            point(1.0, 0.0, 0.0), // b
            point(0.0, 1.0, 0.0), // c
        ];
        let expanded = expand_faces(&positions, &[0, 1, 2]);
        assert_eq!(expanded.degenerate_faces, 0);
        assert_eq!(expanded.corners, positions.to_vec());
        assert!(expanded.normals[0].abs_diff_eq(Vec3::Z, 1e-6));
        assert!(expanded.normals[1].abs_diff_eq(Vec3::Z, 1e-6));
        assert!(expanded.normals[2].abs_diff_eq(Vec3::Z, 1e-6));

        // Clockwise winding: Y × X = −Z — the cross product follows the
        // winding (no culling happens GPU-side, but the CPU normal must be
        // the geometric one of the spec formula).
        let expanded = expand_faces(&positions, &[1, 0, 2]);
        assert_eq!(expanded.degenerate_faces, 0);
        assert!(expanded.normals[0].abs_diff_eq(-Vec3::Z, 1e-6));
    }

    #[test]
    fn expand_faces_keeps_vertices_per_kept_face_in_face_order() {
        let positions = [
            point(0.0, 0.0, 0.0),
            point(1.0, 0.0, 0.0),
            point(0.0, 1.0, 0.0),
            point(1.0, 1.0, 0.0),
        ];
        // Two faces sharing an edge: the shared vertices are duplicated, one
        // copy per face (spec §6: vertex duplication, no index buffer).
        let expanded = expand_faces(&positions, &[0, 1, 2, 1, 3, 2]);
        assert_eq!(expanded.degenerate_faces, 0);
        assert_eq!(
            expanded.corners,
            vec![
                positions[0],
                positions[1],
                positions[2], // face 0
                positions[1],
                positions[3],
                positions[2], // face 1
            ]
        );
        assert_eq!(expanded.normals.len(), 6);
        assert!(expanded.normals[3].abs_diff_eq(Vec3::Z, 1e-6));
    }

    #[test]
    fn expand_faces_skips_and_counts_degenerate_faces() {
        let valid = [
            point(0.0, 0.0, 0.0),
            point(1.0, 0.0, 0.0),
            point(0.0, 1.0, 0.0),
        ];
        let positions = [
            valid[0],
            valid[1],
            valid[2],
            point(2.0, 0.0, 0.0),      // 3: collinear with 0 and 1
            point(f32::NAN, 0.0, 0.0), // 4: non-finite (spec G1)
        ];
        // Faces: valid; collinear (0,1,3); duplicate corner (0,1,1 → wait no, (0,0,1)); NaN corner (0,4,2); OOB (0,1,99).
        let indices = [0, 1, 2, 0, 1, 3, 0, 0, 1, 0, 4, 2, 0, 1, 99, 0, 1];
        let expanded = expand_faces(&positions, &indices);
        assert_eq!(
            expanded.degenerate_faces, 5,
            "collinear, duplicate, NaN, OOB, partial chunk"
        );
        assert_eq!(expanded.corners.len(), 3, "only the valid face survives");
        assert_eq!(expanded.normals.len(), 3);
        assert!(expanded.normals[0].abs_diff_eq(Vec3::Z, 1e-6));
    }

    #[test]
    fn expand_faces_of_an_index_less_mesh_is_empty() {
        let expanded = expand_faces(&[], &[]);
        assert_eq!(expanded.degenerate_faces, 0);
        assert!(expanded.corners.is_empty());
        assert!(expanded.normals.is_empty());
    }

    #[test]
    fn expand_faces_normalizes_tilted_faces_to_unit_length() {
        // A face in the xz plane ordered +X then +Z: the right-hand rule
        // (b − a) × (c − a) points the normal at −Y, and normalization
        // brings its length from 6 to 1.
        let positions = [
            point(0.0, 0.0, 0.0),
            point(2.0, 0.0, 0.0),
            point(0.0, 0.0, 3.0),
        ];
        let expanded = expand_faces(&positions, &[0, 1, 2]);
        assert_eq!(expanded.degenerate_faces, 0);
        assert!(expanded.normals[0].abs_diff_eq(Vec3::NEG_Y, 1e-6));
        assert!((expanded.normals[0].length() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn mesh_pipeline_takes_two_vertex_buffers_positions_then_normals() {
        let layouts = mesh_vertex_buffer_layouts();
        assert_eq!(layouts.len(), 2);

        let positions = &layouts[0];
        assert_eq!(positions.array_stride, POSITION_STRIDE_BYTES);
        assert_eq!(positions.step_mode, wgpu::VertexStepMode::Vertex);
        assert_eq!(positions.attributes, &POSITION_ATTRIBUTES[..]);

        let normals = &layouts[1];
        assert_eq!(normals.array_stride, NORMAL_STRIDE_BYTES);
        assert_eq!(normals.step_mode, wgpu::VertexStepMode::Vertex);
        assert_eq!(normals.attributes, &NORMAL_ATTRIBUTES[..]);
        assert_eq!(normals.attributes[0].shader_location, 1);
    }

    #[test]
    fn mesh_pipeline_depth_bias_matches_the_constant_table() {
        // The pipeline's bias comes from the module constants — the table in
        // the module docs is the M3 calibration entry point, so the values
        // must be exactly the ones the tests and the manual protocol read.
        assert_eq!(DEPTH_BIAS_CONSTANT, 4);
        assert_eq!(DEPTH_BIAS_SLOPE_SCALE, 1.0);
        assert_eq!(DEPTH_BIAS_CLAMP, 0.0);
    }

    #[test]
    fn mesh_wgsl_compiles_headlessly() {
        let module = naga::front::wgsl::parse_str(MESH_SHADER_SOURCE)
            .unwrap_or_else(|error| panic!("mesh.wgsl failed to parse:\n{error}"));
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator
            .validate(&module)
            .unwrap_or_else(|error| panic!("mesh.wgsl failed naga validation:\n{error}"));
    }

    #[test]
    fn default_mesh_face_color_pins_the_moved_wgsl_constant() {
        // The former WGSL `FACE_COLOR` constant moved to the CPU side of
        // the appearance channel (ui-blueprint plan §3.1): linear light
        // (0.7, 0.75, 0.8, 1.0) ≈ sRGB (0.854, 0.881, 0.906), a light
        // neutral gray. It is the albedo every surface-mesh upload
        // provisions, so an upload without appearance calls renders exactly
        // as before the move.
        assert_eq!(DEFAULT_MESH_FACE_COLOR, [0.7, 0.75, 0.8, 1.0]);

        // The shader reads the uniform now, and the old WGSL constant is
        // gone as code (the module docs may still mention `FACE_COLOR` in
        // prose) — a second color path must never come back.
        assert!(
            MESH_SHADER_SOURCE.contains("appearance.albedo"),
            "mesh.wgsl must take its color from the appearance uniform"
        );
        assert!(
            !MESH_SHADER_SOURCE.contains("const FACE_COLOR"),
            "the color moved to the uniform; a WGSL constant would be a second color path"
        );
    }
}
