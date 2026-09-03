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
//   lighting — the color now arrives per object through the appearance
//   channel (004 ui-blueprint plan §3.1) instead of a WGSL constant, so the
//   app can retint a mesh in place with one 64-byte uniform write. The face
//   normal attribute is therefore still not consumed today: it is part of the
//   pipeline contract (spec §6: face normals are computed CPU-side, with
//   vertex duplication) and is the input a future headlight model would read
//   once a view matrix exists — the scene's uniform is the combined
//   view-projection only, which cannot rotate normals into view space.
//
// - Shared depth (spec §6): the pipeline writes depth with a strict Less
//   compare and a positive polygon offset (constant 4, slope 1.0 — see the
//   constant table in mesh.rs, calibrated against the M3 protocol) that
//   pushes mesh fragments away from equal-depth geometry, so clouds and
//   lines lying on a mesh surface keep winning the depth test. Rasterization
//   is double-sided (no culling, spec §6), so back faces of a closed mesh
//   are visible from inside or behind.
//
// - The appearance albedo is linear light: the render target is an sRGB
//   surface, so writing linear light lets the hardware sRGB encode on store
//   reproduce the intended color. The CPU default is (0.7, 0.75, 0.8) linear
//   ≈ sRGB (0.854, 0.881, 0.906) — a light neutral gray (the former WGSL
//   `FACE_COLOR`, moved to the uniform's CPU default in mesh.rs).
//   Meshes have no per-vertex color, so the albedo is always the face color;
//   the `APPEARANCE_FLAG_OVERRIDE` bit (present for the point/line mixers)
//   has no separate meaning here, and only the selection highlight applies.

/// When set in `appearance.flags`, the surviving linear color is boosted by
/// `HIGHLIGHT_GAIN` (selection highlight, spec §6). Mirrors
/// `Appearance::with_selected` and the `APPEARANCE_FLAG_SELECTED` constant
/// of renderer.rs.
const APPEARANCE_FLAG_SELECTED: u32 = 2u;

/// Selection-highlight gain applied to `color.rgb` (linear light), clamped
/// to white; alpha is untouched so highlights never change coverage.
const HIGHLIGHT_GAIN: f32 = 1.25;

/// Fixed 64-byte per-object appearance uniform (plan §3.1; byte layout
/// mirrored by `pack_appearance` in renderer.rs): `albedo` at 0, `flags` at
/// 16, the two reserved vec4 slots padding the struct to 64 bytes.
struct ObjectAppearance {
    /// Linear-light face color (the unlit mesh's only color input).
    albedo: vec4<f32>,
    /// Appearance flags: `APPEARANCE_FLAG_SELECTED` (and, for CPU-side
    /// uniformity with the point/line mixers, `APPEARANCE_FLAG_OVERRIDE`,
    /// which is meaningless here because there is no vertex color to
    /// replace).
    flags: u32,
    reserved_a: vec4<f32>,
    reserved_b: vec4<f32>,
}

/// Mix the appearance channel over the constant face color: the albedo *is*
/// the face color, and the selection bit highlights it.
fn mix_appearance() -> vec4<f32> {
    var out = appearance.albedo;
    if (appearance.flags & APPEARANCE_FLAG_SELECTED) != 0u {
        out = vec4<f32>(min(out.rgb * vec3<f32>(HIGHLIGHT_GAIN), vec3<f32>(1.0)), out.a);
    }
    return out;
}

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> view_proj: mat4x4<f32>;

@group(1) @binding(0)
var<uniform> appearance: ObjectAppearance;

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
    return mix_appearance();
}
