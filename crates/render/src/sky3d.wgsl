struct Globals {
    view_proj: mat4x4<f32>,
}
@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var miniature: texture_2d<f32>;
@group(1) @binding(1) var miniature_sampler: sampler;
@group(1) @binding(2) var<uniform> target_size: vec4<f32>;

@vertex
fn vs_main(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return globals.view_proj * vec4<f32>(position, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = clamp(position.xy / target_size.xy * target_size.zw, vec2<f32>(0.5), target_size.zw - vec2<f32>(0.5));
    return textureSampleLevel(miniature, miniature_sampler, pixel / vec2<f32>(textureDimensions(miniature)), 0.0);
}
