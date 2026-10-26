@group(0) @binding(0) var frame: texture_2d<f32>;
@group(0) @binding(1) var lut: texture_3d<f32>;
@group(0) @binding(2) var lut_sampler: sampler;
@group(0) @binding(3) var<uniform> settings: vec4<f32>;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    return vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
}

fn srgb_to_linear(rgb: vec3<f32>) -> vec3<f32> {
    return select(pow((rgb + 0.055) / 1.055, vec3<f32>(2.4)), rgb / 12.92, rgb <= vec3<f32>(0.04045));
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let source = textureLoad(frame, vec2<i32>(position.xy), 0);
    let coordinate = source.rgb * (31.0 / 32.0) + (0.5 / 32.0);
    let corrected = textureSampleLevel(lut, lut_sampler, coordinate, 0.0).rgb;
    let display = mix(source.rgb, corrected, settings.x);
    return vec4<f32>(srgb_to_linear(display), source.a);
}
