struct Globals {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
    eye: vec4<f32>,
    surface_view: vec4<u32>,
}

// x: alpha test, y: two-layer blend, z: translucent, w: untextured.
struct MaterialParams {
    flags: vec4<f32>,
    helper: vec4<f32>,
}

@group(0) @binding(0) var<uniform> globals: Globals;

@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var blend_texture: texture_2d<f32>;
@group(1) @binding(2) var texture_sampler: sampler;
@group(1) @binding(3) var<uniform> material: MaterialParams;
@group(1) @binding(4) var normal_texture: texture_2d<f32>;
@group(1) @binding(5) var surface_texture: texture_2d<f32>;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) lightmap: vec3<f32>,
    @location(5) smoothing: u32,
}

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) world_position: vec3<f32>,
    @location(4) lightmap: vec3<f32>,
    @location(5) @interpolate(flat) smoothing: u32,
}

@vertex
fn vs_main(v: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.clip = globals.view_proj * vec4<f32>(v.position, 1.0);
    out.uv = v.uv;
    out.normal = v.normal;
    out.color = v.color;
    out.world_position = v.position;
    out.lightmap = v.lightmap;
    out.smoothing = v.smoothing;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    var texel = textureSample(base_texture, texture_sampler, in.uv);
    let second = textureSample(blend_texture, texture_sampler, in.uv);
    let mapped_normal = textureSample(normal_texture, texture_sampler, in.uv).xyz * 2.0 - 1.0;
    let surface = textureSample(surface_texture, texture_sampler, in.uv).xyz;
    let dx = dpdx(in.world_position);
    let dy = dpdy(in.world_position);
    let uv_dx = dpdx(in.uv);
    let uv_dy = dpdy(in.uv);
    let grid_width = max(fwidth(in.lightmap.xy), vec2<f32>(0.00001));
    if globals.surface_view.x == 3u && in.lightmap.z != 0.0 {
        let shade = 0.55 + 0.45 * clamp(dot(normalize(in.normal), globals.light_dir.xyz) * 0.5 + 0.5, 0.0, 1.0);
        return vec4<f32>(vec3<f32>(0.65) * in.color.rgb * shade, 1.0);
    }
    if (globals.surface_view.x == 1u || globals.surface_view.x == 2u) && in.lightmap.z != 0.0 {
        if globals.surface_view.x == 2u {
            let member = (in.smoothing & (1u << globals.surface_view.y)) != 0u;
            let base = select(vec3<f32>(0.065, 0.08, 0.11), vec3<f32>(0.1, 0.7, 0.8), member);
            let shade = 0.65 + 0.35 * abs(dot(normalize(in.normal), globals.light_dir.xyz));
            return vec4<f32>(base * shade, 1.0);
        }
        if in.lightmap.z < 0.0 {
            return vec4<f32>(0.8, 0.02, 0.8, 1.0);
        }
        var base = vec3<f32>(0.12, 0.45, 0.85);
        if in.lightmap.z > 8.0 { base = vec3<f32>(0.08, 0.6, 0.3); }
        if in.lightmap.z > 16.0 { base = vec3<f32>(0.9, 0.48, 0.05); }
        if in.lightmap.z > 32.0 { base = vec3<f32>(0.8, 0.12, 0.07); }
        let distance = abs(fract(in.lightmap.xy - 0.5) - 0.5) / grid_width;
        let line = 1.0 - clamp(min(distance.x, distance.y), 0.0, 1.0);
        let fade = 1.0 - smoothstep(0.5, 1.5, max(grid_width.x, grid_width.y));
        return vec4<f32>(base * (1.0 - 0.65 * line * fade), 1.0);
    }
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
    if material.helper.x > 0.5 {
        var helper_normal = n;
        let determinant = uv_dx.x * uv_dy.y - uv_dx.y * uv_dy.x;
        if abs(determinant) > 0.00000001 {
            let tangent = normalize((dx * uv_dy.y - dy * uv_dx.y) / determinant);
            let bitangent = normalize((dy * uv_dx.x - dx * uv_dy.x) / determinant);
            helper_normal = normalize(tangent * mapped_normal.x + bitangent * mapped_normal.y + n * mapped_normal.z);
        }
        let base = texel.rgb * in.color.rgb;
        let light = globals.light_dir.xyz;
        let view = normalize(globals.eye.xyz - in.world_position);
        let halfway = normalize(light + view);
        let roughness = clamp(surface.y, 0.12, 1.0);
        let metallic = surface.z;
        let diffuse = (0.5 + 0.5 * max(dot(helper_normal, light), 0.0)) * (0.7 + 0.3 * surface.x);
        let gloss = pow(max(dot(helper_normal, halfway), 0.0), mix(128.0, 8.0, roughness));
        let reflection = mix(vec3<f32>(0.08), base, metallic) * gloss;
        let shaded = base * diffuse + reflection;
        let globe = mix(base, in.color.rgb, 0.7) * (0.7 + 0.3 * max(dot(helper_normal, light), 0.0));
        let highlight = 0.15 * pow(max(dot(helper_normal, halfway), 0.0), 64.0);
        return vec4<f32>(mix(shaded, globe + vec3<f32>(highlight), in.color.a), 1.0);
    }
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
