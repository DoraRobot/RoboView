// Mesh pipeline shaders (display-types spec §6), embedded into the renderer
// via `include_str!` and naga-validated headless in the unit tests.
//
// Design notes:
//
// - Triangle lists, three vertices per face: the CPU duplicates face corners
//   (no index buffer) and computes one face normal per corner from
//   `(b − a) × (c − a)`, uploaded as a second vertex attribute (location 1).
//   Faces whose corners are not finite, out of range, or collinear are
//   skipped CPU-side and never reach this shader, so positions here are
//   always finite (unlike the point cloud shader, no escape position is
//   needed — see mesh.rs `expand_faces`).
//
// - Shading policy (spec §6, first-listed option): constant face color, no
//   lighting. The face normal attribute is therefore not consumed today: it
//   is part of the pipeline contract (spec §6: face normals are computed
//   CPU-side, with vertex duplication) and is the input a future headlight
//   model would read once a view matrix exists — the scene's uniform is the
//   combined view-projection only, which cannot rotate normals into view
//   space.
//
// - Shared depth (spec §6): the pipeline writes depth with a strict Less
//   compare and a positive polygon offset (constant 4, slope 1.0 — see the
//   constant table in mesh.rs, calibrated against the M3 protocol) that
//   pushes mesh fragments away from equal-depth geometry, so clouds and
//   lines lying on a mesh surface keep winning the depth test. Rasterization
//   is double-sided (no culling, spec §6), so back faces of a closed mesh
//   are visible from inside or behind.
//
// - The constant face color is linear light: the render target is an sRGB
//   surface, so writing linear light lets the hardware sRGB encode on store
//   reproduce the intended color. (0.7, 0.75, 0.8) linear ≈ sRGB
//   (0.854, 0.881, 0.906) — a light neutral gray.

/// Constant face color of the unlit mesh pipeline, linear light, opaque.
const FACE_COLOR: vec4<f32> = vec4<f32>(0.7, 0.75, 0.8, 1.0);

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> view_proj: mat4x4<f32>;

@vertex
fn vs_main(
    @location(0) pos: vec3<f32>,
    @location(1) _face_normal: vec3<f32>,
) -> VsOut {
    var out: VsOut;
    out.clip_position = view_proj * vec4<f32>(pos, 1.0);
    return out;
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return FACE_COLOR;
}
