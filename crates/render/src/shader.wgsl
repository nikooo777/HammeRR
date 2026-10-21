struct AuthoredLight {
    origin_radius: vec4<f32>,
    intensity_cap: vec4<f32>,
    coefficients_spot: vec4<f32>,
    direction_inner: vec4<f32>,
    outer_exponent_fade: vec4<f32>,
}

const SUN_KIND: f32 = 2.0;

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
    lighting_settings: vec4<f32>,
    lighting_lights: array<AuthoredLight, 32>,
}

// x: alpha test, y: two-layer blend, z: translucent, w: untextured.
struct MaterialParams {
    flags: vec4<f32>,
    helper: vec4<f32>,
    normal_transform: vec4<f32>,
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
    @location(6) opacity: f32,
    @location(7) texture_alpha: u32,
    @location(8) tangent: vec4<f32>,
    @location(9) lighting_receiver: u32,
}

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) world_position: vec3<f32>,
    @location(4) lightmap: vec3<f32>,
    @location(5) @interpolate(flat) smoothing: u32,
    @location(6) opacity: f32,
    @location(7) @interpolate(flat) texture_alpha: u32,
    @location(8) tangent: vec4<f32>,
    @location(9) @interpolate(flat) lighting_receiver: u32,
    @location(10) authored_irradiance: vec3<f32>,
}

fn authored_irradiance(position: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    var total = vec3<f32>(0.0);
    for (var index = 0u; index < min(u32(globals.lighting_settings.x), 32u); index += 1u) {
        let light = globals.lighting_lights[index];
        if light.coefficients_spot.w == SUN_KIND {
            total += light.intensity_cap.xyz * clamp(dot(normal, normalize(light.direction_inner.xyz)), 0.0, 1.0) + light.origin_radius.xyz;
            continue;
        }
        let offset = light.origin_radius.xyz - position;
        let scale = max(max(abs(offset.x), abs(offset.y)), abs(offset.z));
        if scale == 0.0 { continue; }
        let distance = scale * length(offset / scale);
        let fade_end = light.outer_exponent_fade.w;
        if distance == 0.0 || (light.origin_radius.w > 0.0 && distance > light.origin_radius.w) || (fade_end > 0.0 && distance >= fade_end) { continue; }
        let direction = offset / distance;
        var evaluated = max(distance, 1.0);
        if light.intensity_cap.w > 0.0 { evaluated = min(evaluated, light.intensity_cap.w); }
        let coefficients = light.coefficients_spot.xyz;
        let divisor = coefficients.x + evaluated * coefficients.y + evaluated * evaluated * coefficients.z;
        if !(divisor > 0.0) { continue; }
        var fade = 1.0;
        if fade_end > 0.0 {
            let t = clamp(1.0 - (max(distance, 1.0) - light.outer_exponent_fade.z) / (fade_end - light.outer_exponent_fade.z), 0.0, 1.0);
            fade = t * t * t * (t * (6.0 * t - 15.0) + 10.0);
        }
        var cone = 1.0;
        if light.coefficients_spot.w > 0.0 {
            let angle = clamp(dot(light.direction_inner.xyz, -direction), -1.0, 1.0);
            let inner = light.direction_inner.w;
            let outer = light.outer_exponent_fade.x;
            cone = 0.0;
            if angle > outer {
                cone = angle;
                if angle < inner {
                    let fringe = clamp((angle - outer) / (inner - outer), 0.0, 1.0);
                    let exponent = light.outer_exponent_fade.y;
                    if exponent == 0.0 || exponent == 1.0 { cone *= fringe; }
                    else { cone *= pow(fringe, exponent); }
                }
            }
        }
        total += light.intensity_cap.xyz * (clamp(dot(normal, direction), 0.0, 1.0) * cone * fade / divisor);
    }
    return total;
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
    out.opacity = v.opacity;
    out.texture_alpha = v.texture_alpha;
    out.tangent = v.tangent;
    out.lighting_receiver = v.lighting_receiver;
    out.authored_irradiance = vec3<f32>(0.0);
    if globals.lighting_settings.z > 0.5 && globals.lighting_settings.y < 0.5 && v.lighting_receiver != 0u {
        out.authored_irradiance = authored_irradiance(v.position, normalize(v.normal));
    }
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
    let alpha = select(1.0, texel.a, material.flags.z > 0.5 || in.texture_alpha != 0u) * in.opacity;
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
    var shading_normal = n;
    if globals.surface_view.z != 0u && material.helper.z > 0.5 && abs(in.tangent.w) > 0.5 {
        let uv = in.uv * material.normal_transform.xy + material.normal_transform.zw;
        let normal = textureSampleGrad(normal_texture, texture_sampler, uv, uv_dx * material.normal_transform.xy, uv_dy * material.normal_transform.xy).xyz * 2.0 - 1.0;
        let tangent = normalize(in.tangent.xyz);
        let bitangent = cross(n, tangent) * in.tangent.w;
        shading_normal = normalize(tangent * normal.x + bitangent * normal.y + n * normal.z);
    }
    var shade = 0.55 + 0.45 * clamp(dot(shading_normal, globals.light_dir.xyz) * 0.5 + 0.5, 0.0, 1.0);
    if globals.surface_view.w != 0u && material.helper.w > 0.5 {
        shade = 0.3 + 0.7 * max(dot(shading_normal, globals.light_dir.xyz), 0.0);
    }
    var color = texel.rgb * in.color.rgb * shade;
    if globals.lighting_settings.z > 0.5 && in.lighting_receiver != 0u {
        var irradiance = in.authored_irradiance;
        if globals.lighting_settings.y > 0.5 { irradiance = authored_irradiance(in.world_position, shading_normal); }
        let radiance = texel.rgb * in.color.rgb * irradiance * globals.lighting_settings.w;
        color = radiance / (vec3<f32>(1.0) + radiance);
    }
    if globals.fog_color.a > 0.0 && material.helper.y < 0.5 {
        let depth = dot(in.world_position - globals.eye.xyz, globals.forward.xyz);
        let factor = clamp((depth - globals.fog_range.x) * globals.fog_range.y, 0.0, globals.fog_color.a);
        color = mix(color, globals.fog_color.rgb, factor * factor);
    }
    return vec4<f32>(color, alpha);
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
