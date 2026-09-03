// Line pipeline shaders (display-types spec §6), embedded into the renderer
// via `include_str!` and naga-validated headless in the unit tests. One
// shader serves the whole line family: open polylines (paths), coordinate
// axes (frames), and marker arrows.
//
// Design notes:
//
// - Geometry is uploaded as one or more `LineStrip` runs (line.rs
//   `upload_path`/`upload_frame`/`upload_arrow`). Non-finite points are
//   split out CPU-side into finite runs of at least two vertices (paths) or
//   rejected at build time (frames/arrows), so positions here are always
//   finite — a strip vertex can never escape to a clip-corner line.
//
// - Colors are uploaded as packed sRGB bytes (Rgba8Unorm, one u32 per
//   vertex) exactly like the point cloud pipeline; the conversion to linear
//   light happens here for the same reason (sRGB render target, hardware
//   encode on store). Keep the curve constants in sync with
//   `Renderer::srgb_to_linear` in renderer.rs (pinned by a unit test).
//
// - Shared depth policy for line primitives (spec §6, plan §3.3): the
//   pipeline depth-tests with a strict Less compare but never writes depth
//   and carries no polygon offset. Lines are therefore visible through the
//   reference surfaces (points, biased mesh) exactly where they are nearer,
//   while overlapping line work (a path crossing itself, an axis crossing a
//   path) resolves by order-independent depth compare instead of fighting at
//   equal depth — and lines never punch holes into the depth buffer that
//   would hide geometry drawn later. Polygons offsets do not apply to line
//   primitives in any case (spec: lines use strict Less, without bias).
//
// - Per-object appearance channel (004 ui-blueprint plan §3.1): group(1)
//   binding(0) carries one fixed 64-byte uniform per mesh handle, mixed over
//   the vertex color in the fragment stage — bit 0 of `flags` replaces the
//   per-vertex colors with the albedo (the selection/semantic-color path the
//   app drives for frames and helpers), bit 1 boosts the surviving color as
//   a selection highlight. The albedo arrives in linear light, exactly like
//   the vertex colors after `srgb_to_linear`, so the two mixing branches sit
//   in the same space (the sRGB target re-encodes on store).

/// Standard sRGB EOTF (IEC 61966-2-1): linear below the 0.04045 knee,
/// gamma 2.4 above it. Mirrors `Renderer::srgb_to_linear` exactly.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

/// When set in `appearance.flags`, the albedo replaces the per-vertex color
/// entirely. Mirrors `Appearance::srgb_override` (renderer.rs): the override
/// color is the object's semantic color, baked CPU-side into the uniform.
const APPEARANCE_FLAG_OVERRIDE: u32 = 1u;

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
    /// Linear-light replacement color (bit `APPEARANCE_FLAG_OVERRIDE`);
    /// unused while the flag is clear, when the per-vertex color wins.
    albedo: vec4<f32>,
    /// Appearance flags: `APPEARANCE_FLAG_OVERRIDE` | `APPEARANCE_FLAG_SELECTED`.
    flags: u32,
    reserved_a: vec4<f32>,
    reserved_b: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> view_proj: mat4x4<f32>;

@group(1) @binding(0)
var<uniform> appearance: ObjectAppearance;

/// Mix the appearance channel over one vertex color (already linear): the
/// override replaces it wholesale, the selection bit highlights whatever
/// survives. Both rules compose — a selected overridden mesh highlights its
/// override albedo.
fn mix_appearance(color: vec4<f32>) -> vec4<f32> {
    var out = color;
    if (appearance.flags & APPEARANCE_FLAG_OVERRIDE) != 0u {
        out = appearance.albedo;
    }
    if (appearance.flags & APPEARANCE_FLAG_SELECTED) != 0u {
        out = vec4<f32>(min(out.rgb * vec3<f32>(HIGHLIGHT_GAIN), vec3<f32>(1.0)), out.a);
    }
    return out;
}

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vs_main(
    @location(0) pos: vec3<f32>,
    @location(1) rgba: vec4<f32>,
) -> VsOut {
    var out: VsOut;
    out.clip_position = view_proj * vec4<f32>(pos, 1.0);
    out.color = vec4<f32>(
        srgb_to_linear(rgba.r),
        srgb_to_linear(rgba.g),
        srgb_to_linear(rgba.b),
        rgba.a,
    );
    return out;
}

@fragment
fn fs_main(@location(0) color: vec4<f32>) -> @location(0) vec4<f32> {
    return mix_appearance(color);
}
