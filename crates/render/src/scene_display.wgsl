@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var<uniform> exposure: vec4<f32>;

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((vertex << 1u) & 2u), f32(vertex & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let texel = textureLoad(scene, vec2<i32>(position.xy), 0);
    let exposed = texel.rgb * exposure.x;
    return vec4<f32>(exposed / (vec3<f32>(1.0) + exposed), texel.a);
}
