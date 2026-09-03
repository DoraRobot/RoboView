// Point cloud pipeline shaders, embedded into the renderer binary via
// `include_str!`. Keep the color curve in sync with `Renderer::srgb_to_linear`
// in renderer.rs (same constants, guarded by a unit test).
//
// Design notes:
//
// - One 1-pixel point per vertex, drawn as a `PointList`. The scene shares
//   one depth attachment (display-types spec §6): the pipeline writes depth
//   with a strict Less compare and zero bias, so point-vs-point and
//   point-vs-geometry visibility follows the depth test instead of upload
//   order. Points are the reference surface that later mesh pipelines are
//   depth-biased against.
//
// - Colors are uploaded as packed sRGB bytes (Rgba8Unorm, one u32 per point).
//   The hardware unorm-decodes the attribute into [0, 1] sRGB values in the
//   vertex stage, so this shader only sees floats. The linear conversion
//   happens here because the render target is an sRGB surface: writing
//   linear light lets the hardware sRGB encode on store reproduce the stored
//   file colors without a color cast. Alpha passes through unconverted (it
//   is always 255 in the uploaded data and is not a color channel).
//
// - Non-finite positions (spec G1: NaN/Inf points are retained in the data
//   and excluded from the bounds only) are not fed to the rasterizer:
//   writing NaN/Inf to `@builtin(position)` has undefined rasterization
//   behavior, and WGSL has no vertex-stage discard. Instead the point is
//   pushed outside the clip volume: for w = 1 the clip test is
//   -w <= x, y, z <= w, so (2, 2, 2, 1) fails every plane. Points whose
//   center lies outside the clip volume are culled before rasterization, and
//   this position is also far outside any viewport, so no fragment can
//   survive. All outputs stay finite.
//
// - Per-object appearance channel (004 ui-blueprint plan §3.1): group(1)
//   binding(0) carries one fixed 64-byte uniform per mesh handle, mixed over
//   the vertex color in the fragment stage — bit 0 of `flags` replaces the
//   per-vertex colors with the albedo (the semantic-color path the app drives
//   for a point cloud's theme color), bit 1 boosts the surviving color as a
//   selection highlight. The albedo arrives in linear light, exactly like the
//   vertex colors after `srgb_to_linear`, so both mixing branches sit in the
//   same space (the sRGB target re-encodes on store).

/// Standard sRGB EOTF (IEC 61966-2-1): linear below the 0.04045 knee,
/// gamma 2.4 above it. Mirrors `Renderer::srgb_to_linear` exactly.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

/// True when `v` is a finite f32. WGSL provides no finiteness builtin, so
/// non-finite values are detected with plain comparisons: NaN compares
/// unequal to itself, and infinite magnitudes exceed the largest
/// representable f32.
fn is_finite_scalar(v: f32) -> bool {
    return v == v && abs(v) <= 3.4028235e38;
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

@group(0) @binding(0)
var<uniform> view_proj: mat4x4<f32>;

@group(1) @binding(0)
var<uniform> appearance: ObjectAppearance;

@vertex
fn vs_main(
    @location(0) pos: vec3<f32>,
    @location(1) rgba: vec4<f32>,
) -> VsOut {
    var out: VsOut;
    if !is_finite_scalar(pos.x) || !is_finite_scalar(pos.y) || !is_finite_scalar(pos.z) {
        out.clip_position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    } else {
        out.clip_position = view_proj * vec4<f32>(pos, 1.0);
    }
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
