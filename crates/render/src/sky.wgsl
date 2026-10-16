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
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var sky_rt: texture_2d<f32>;
@group(1) @binding(1) var sky_bk: texture_2d<f32>;
@group(1) @binding(2) var sky_lf: texture_2d<f32>;
@group(1) @binding(3) var sky_ft: texture_2d<f32>;
@group(1) @binding(4) var sky_up: texture_2d<f32>;
@group(1) @binding(5) var sky_dn: texture_2d<f32>;
@group(1) @binding(6) var sky_sampler: sampler;

struct SkyTransforms {
    faces: array<vec4<f32>, 6>,
}
@group(1) @binding(7) var<uniform> transforms: SkyTransforms;

fn sky_uv(uv: vec2<f32>, face: u32) -> vec2<f32> {
    let transform = transforms.faces[face];
    return uv * transform.xy + transform.zw;
}

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) direction: vec3<f32>,
}

@vertex
fn vs_main(@location(0) position: vec3<f32>) -> VertexOut {
    var out: VertexOut;
    out.clip = globals.view_proj * vec4<f32>(position, 1.0);
    out.direction = position - globals.eye.xyz;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let d = in.direction;
    let a = abs(d);
    var texel: vec4<f32>;
    if a.x >= a.y && a.x >= a.z {
        if d.x >= 0.0 {
            texel = textureSampleLevel(sky_rt, sky_sampler, sky_uv(vec2<f32>(-d.y, -d.z) / a.x * 0.5 + 0.5, 0u), 0.0);
        } else {
            texel = textureSampleLevel(sky_lf, sky_sampler, sky_uv(vec2<f32>(d.y, -d.z) / a.x * 0.5 + 0.5, 2u), 0.0);
        }
    } else if a.y >= a.z {
        if d.y >= 0.0 {
            texel = textureSampleLevel(sky_bk, sky_sampler, sky_uv(vec2<f32>(d.x, -d.z) / a.y * 0.5 + 0.5, 1u), 0.0);
        } else {
            texel = textureSampleLevel(sky_ft, sky_sampler, sky_uv(vec2<f32>(-d.x, -d.z) / a.y * 0.5 + 0.5, 3u), 0.0);
        }
    } else if d.z >= 0.0 {
        texel = textureSampleLevel(sky_up, sky_sampler, sky_uv(vec2<f32>(-d.y, d.x) / a.z * 0.5 + 0.5, 4u), 0.0);
    } else {
        texel = textureSampleLevel(sky_dn, sky_sampler, sky_uv(vec2<f32>(-d.y, -d.x) / a.z * 0.5 + 0.5, 5u), 0.0);
    }
    return vec4<f32>(texel.rgb, 1.0);
}
