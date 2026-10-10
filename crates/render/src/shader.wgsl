struct Globals {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
}

// x: alpha test, y: two-layer blend, z: translucent, w: untextured.
struct MaterialParams {
    flags: vec4<f32>,
}

@group(0) @binding(0) var<uniform> globals: Globals;

@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var blend_texture: texture_2d<f32>;
@group(1) @binding(2) var texture_sampler: sampler;
@group(1) @binding(3) var<uniform> material: MaterialParams;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
}

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
}

@vertex
fn vs_main(v: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.clip = globals.view_proj * vec4<f32>(v.position, 1.0);
    out.uv = v.uv;
    out.normal = v.normal;
    out.color = v.color;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    var texel = textureSample(base_texture, texture_sampler, in.uv);
    let second = textureSample(blend_texture, texture_sampler, in.uv);
    if material.flags.y > 0.5 {
        texel = mix(texel, second, in.color.a);
    }
    if material.flags.w > 0.5 {
        texel = vec4<f32>(1.0);
    }
    if material.flags.x > 0.5 && texel.a < 0.5 {
        discard;
    }

    // Fixed directional light like Hammer's 3D view: tops brightest,
    // undersides darkest, walls in between depending on facing.
    let n = normalize(in.normal);
    let shade = 0.55 + 0.45 * clamp(dot(n, globals.light_dir.xyz) * 0.5 + 0.5, 0.0, 1.0);
    let alpha = select(1.0, texel.a, material.flags.z > 0.5);
    return vec4<f32>(texel.rgb * in.color.rgb * shade, alpha);
}

struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vs_line(@location(0) position: vec3<f32>, @location(1) color: vec4<f32>) -> LineOut {
    var out: LineOut;
    out.clip = globals.view_proj * vec4<f32>(position, 1.0);
    out.color = color;
    return out;
}

@fragment
fn fs_line(in: LineOut) -> @location(0) vec4<f32> {
    return in.color;
}
