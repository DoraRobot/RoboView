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

/// Standard sRGB EOTF (IEC 61966-2-1): linear below the 0.04045 knee,
/// gamma 2.4 above it. Mirrors `Renderer::srgb_to_linear` exactly.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> view_proj: mat4x4<f32>;

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
    return color;
}
