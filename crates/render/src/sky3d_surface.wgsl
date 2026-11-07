struct MaterialParams {
    flags: vec4<f32>,
    helper: vec4<f32>,
}
@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(2) var texture_sampler: sampler;
@group(1) @binding(3) var<uniform> material: MaterialParams;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) world_position: vec3<f32>,
}

@vertex
fn vs_main(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32>) -> VertexOut {
    var out: VertexOut;
    out.clip = globals.view_proj * vec4<f32>(position, 1.0);
    out.normal = normal;
    out.uv = uv;
    out.world_position = position;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let texel = textureSample(base_texture, texture_sampler, in.uv);
    if material.flags.x > 0.5 && texel.a < material.helper.x {
        discard;
    }
    let diffuse = 0.55 + 0.45 * clamp(dot(normalize(in.normal), globals.light_dir.xyz) * 0.5 + 0.5, 0.0, 1.0);
    let shade = select(diffuse, 1.0, material.helper.y > 0.5);
    return vec4<f32>(apply_fog(in.world_position, texel.rgb * shade), 1.0);
}
