struct Globals {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
    eye: vec4<f32>,
    surface_view: vec4<u32>,
    fog_color: vec4<f32>,
    fog_range: vec4<f32>,
    forward: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    sky_params: vec4<f32>,
}
@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var icon: texture_2d<f32>;
@group(1) @binding(2) var icon_sampler: sampler;
struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}
@vertex fn vs_main(@location(0) center: vec3<f32>, @location(1) offset: vec2<f32>, @location(2) uv: vec2<f32>) -> VertexOut {
    var out: VertexOut;
    let position = center + globals.right.xyz * offset.x + globals.up.xyz * offset.y;
    out.position = globals.view_proj * vec4<f32>(position, 1.0);
    out.uv = uv;
    return out;
}
@fragment fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return textureSample(icon, icon_sampler, in.uv);
}
