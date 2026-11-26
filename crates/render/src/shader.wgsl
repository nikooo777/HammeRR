const SUN_KIND: f32 = 2.0;
const FULL_BILLBOARD_TAG: f32 = -2.5;
const VERTICAL_BILLBOARD_TAG: f32 = -3.5;
const BAKED_DETAIL_RECEIVER: u32 = 2u;
const BAKED_RADIANCE_RECEIVER: u32 = 3u;
const UNLIT_PROJECTED_RECEIVER: u32 = 6u;
const TEXTURE_ALPHA: u32 = 1u;
const MODEL_NORMAL_FRAME: u32 = 2u;
const MATERIAL_EMISSION: u32 = 4u;
const MODEL_SURFACE: u32 = 8u;
const BASE_TEXTURE_TRANSFORM: u32 = 128u;
const MODEL_PHONG_FRAME: u32 = 16u;
const DETAIL_LAYER: u32 = 32u;
const ENVIRONMENT_LAYER: u32 = 64u;
const ANIMATED_BASE: u32 = 256u;

// x: alpha test, y: two-layer blend, z: translucent, w: untextured.
struct MaterialParams {
    flags: vec4<f32>,
    helper: vec4<f32>,
    normal_transform: vec4<f32>,
    emission: vec4<f32>,
    base_uv_transform: array<vec4<f32>, 2>,
    phong_fresnel_boost: vec4<f32>,
    phong_exponent: f32,
    detail_tint_factor: vec4<f32>,
    detail_scale: f32,
    ssbump: f32,
    environment: vec4<f32>,
}

@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var blend_texture: texture_2d<f32>;
@group(1) @binding(2) var texture_sampler: sampler;
@group(1) @binding(3) var<uniform> material: MaterialParams;
@group(1) @binding(4) var normal_texture: texture_2d<f32>;
@group(1) @binding(5) var second_normal_texture: texture_2d<f32>;
@group(1) @binding(7) var environment_cube: texture_cube<f32>;
@group(1) @binding(8) var environment_sampler: sampler;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) lightmap: vec3<f32>,
    @location(5) smoothing: u32,
    @location(6) opacity: f32,
    @location(7) texture_flags: u32,
    @location(8) tangent: vec4<f32>,
    @location(9) lighting_receiver: u32,
    @location(10) baked_irradiance: vec3<f32>,
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
    @location(7) @interpolate(flat) texture_flags: u32,
    @location(8) tangent: vec4<f32>,
    @location(9) @interpolate(flat) lighting_receiver: u32,
    @location(10) authored_irradiance: vec3<f32>,
    @location(11) baked_irradiance: vec3<f32>,
    @location(12) model_bitangent: vec3<f32>,
}

struct AuthoredLighting {
    diffuse: vec3<f32>,
    specular: vec3<f32>,
}

fn authored_lighting(
    position: vec3<f32>, normal: vec3<f32>, reflection: vec3<f32>, exponent: f32,
) -> AuthoredLighting {
    var total = AuthoredLighting(vec3<f32>(0.0), vec3<f32>(0.0));
    for (var index = 0u; index < min(u32(globals.lighting_settings.x), 32u); index += 1u) {
        let light = globals.lighting_lights[index];
        var direction: vec3<f32>;
        var weight: f32;
        var ambient = vec3<f32>(0.0);
        if light.coefficients_spot.w == SUN_KIND {
            direction = normalize(light.direction_inner.xyz);
            weight = clamp(dot(normal, direction), 0.0, 1.0);
            ambient = light.origin_radius.xyz;
        } else {
            let offset = light.origin_radius.xyz - position;
            let scale = max(max(abs(offset.x), abs(offset.y)), abs(offset.z));
            if scale == 0.0 { continue; }
            let distance = scale * length(offset / scale);
            let fade_end = light.outer_exponent_fade.w;
            if distance == 0.0 || (light.origin_radius.w > 0.0 && distance > light.origin_radius.w) || (fade_end > 0.0 && distance >= fade_end) { continue; }
            direction = offset / distance;
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
                        let cone_exponent = light.outer_exponent_fade.y;
                        if cone_exponent == 0.0 || cone_exponent == 1.0 { cone *= fringe; }
                        else { cone *= pow(fringe, cone_exponent); }
                    }
                }
            }
            weight = clamp(dot(normal, direction), 0.0, 1.0) * cone * fade / divisor;
        }
        let direct = light.intensity_cap.xyz * weight;
        total.diffuse += direct + ambient;
        if exponent > 0.0 {
            let lobe = pow(clamp(dot(reflection, direction), 0.0, 1.0), exponent);
            total.specular += direct * lobe;
        }
    }
    return total;
}

fn view_direction(position: vec3<f32>) -> vec3<f32> {
    if globals.eye.w > 0.5 {
        let offset = globals.eye.xyz - position;
        let scale = max(max(abs(offset.x), abs(offset.y)), abs(offset.z));
        if scale > 0.0 { return normalize(offset / scale); }
    }
    return -globals.forward.xyz;
}

fn environment_reflection(in: VertexOut) -> vec3<f32> {
    let params = material.environment;
    if globals.surface_view.x != 0u || all(params.rgb == vec3<f32>(0.0)) {
        return vec3<f32>(0.0);
    }
    let enabled = (in.texture_flags & ENVIRONMENT_LAYER) != 0u;
    let normal = normalize(select(vec3<f32>(0.0, 0.0, 1.0), in.normal, enabled));
    let eye = view_direction(in.world_position);
    let sampled = textureSample(environment_cube, environment_sampler, reflect(-eye, normal)).rgb;
    let edge = clamp(1.0 - dot(normal, eye), 0.0, 1.0);
    let squared = edge * edge;
    let fresnel = params.w + (1.0 - params.w) * squared * squared * edge;
    return select(vec3<f32>(0.0), sampled * params.rgb * fresnel, enabled);
}

fn authored_radiance(position: vec3<f32>, normal: vec3<f32>, albedo: vec3<f32>, phong_mask: f32) -> vec3<f32> {
    let ranges = material.phong_fresnel_boost;
    if phong_mask == 0.0 || ranges.w == 0.0 {
        return albedo * authored_lighting(position, normal, vec3<f32>(0.0), 0.0).diffuse;
    }
    let eye = view_direction(position);
    let ndotv = dot(normal, eye);
    let reflection = 2.0 * normal * ndotv - eye;
    let lighting = authored_lighting(position, normal, reflection, material.phong_exponent);
    let edge = clamp(1.0 - ndotv, 0.0, 1.0);
    let q = edge * edge - 0.5;
    let fresnel = ranges.y + q * select(ranges.x, ranges.z, q >= 0.0);
    return albedo * lighting.diffuse + lighting.specular * (phong_mask * ranges.w * fresnel);
}

fn bump_basis(normal: vec3<f32>, tangent: vec3<f32>, bitangent: vec3<f32>) -> mat3x3<f32> {
    let up = normal * sqrt(1.0 / 3.0);
    let back = tangent * -sqrt(1.0 / 6.0);
    let cross_axis = bitangent * sqrt(1.0 / 2.0);
    return mat3x3<f32>(tangent * sqrt(2.0 / 3.0) + up, back + cross_axis + up, back - cross_axis + up);
}

fn detail_billboard_basis(origin: vec3<f32>, vertical: bool) -> mat2x3<f32> {
    var direction = globals.eye.xyz - origin;
    if vertical { direction.z = 0.0; }
    let maximum = max(max(abs(direction.x),abs(direction.y)),abs(direction.z));
    if maximum > 0.0 { direction /= maximum; }
    let horizontal = length(direction.xy);
    if horizontal == 0.0 {
        return mat2x3<f32>(vec3<f32>(0.0,-1.0,0.0),vec3<f32>(select(1.0,-1.0,direction.z > 0.0),0.0,0.0));
    }
    let distance = length(direction);
    return mat2x3<f32>(vec3<f32>(direction.y/horizontal,-direction.x/horizontal,0.0),
        vec3<f32>(-direction.x*direction.z/(horizontal*distance),-direction.y*direction.z/(horizontal*distance),horizontal/distance));
}

@vertex
fn vs_main(v: VertexIn) -> VertexOut {
    var out: VertexOut;
    var position = v.position;
    var normal = v.normal;
    // Fractional negative tags carry detail offsets; ordinary lightmaps use integer scales.
    if v.lightmap.z == FULL_BILLBOARD_TAG || v.lightmap.z == VERTICAL_BILLBOARD_TAG {
        let basis = detail_billboard_basis(v.position,v.lightmap.z == VERTICAL_BILLBOARD_TAG);
        position += basis[0]*v.lightmap.x + basis[1]*v.lightmap.y;
        normal = cross(basis[0],basis[1]);
    }
    out.clip = globals.view_proj * vec4<f32>(position, 1.0);
    out.uv = v.uv;
    out.normal = normal;
    out.color = v.color;
    out.world_position = position;
    out.lightmap = v.lightmap;
    out.smoothing = v.smoothing;
    out.opacity = v.opacity;
    out.texture_flags = v.texture_flags;
    out.tangent = v.tangent;
    out.lighting_receiver = v.lighting_receiver;
    out.baked_irradiance = v.baked_irradiance;
    out.model_bitangent = vec3<f32>(0.0);
    if (v.texture_flags & MODEL_PHONG_FRAME) != 0u {
        out.model_bitangent = normalize(cross(v.normal, v.tangent.xyz) * v.tangent.w);
    }
    out.authored_irradiance = vec3<f32>(0.0);
    if globals.lighting_settings.z > 0.5 && globals.lighting_settings.y < 0.5 && v.lighting_receiver == 1u {
        out.authored_irradiance = authored_lighting(v.position, normalize(v.normal), vec3<f32>(0.0), 0.0).diffuse;
    }
    return out;
}

fn surface_diagnostic(in: VertexOut, grid_width: vec2<f32>) -> vec4<f32> {
    if globals.surface_view.x == 3u && in.lightmap.z != 0.0 {
        let shade = select(0.55 + 0.45 * clamp(dot(normalize(in.normal), globals.light_dir.xyz) * 0.5 + 0.5, 0.0, 1.0), 1.0, in.lighting_receiver == BAKED_DETAIL_RECEIVER || (in.lighting_receiver & 15u) == BAKED_RADIANCE_RECEIVER || (in.lighting_receiver & 15u) == 4u);
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
    return vec4<f32>(0.0);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    var base_uv = in.uv;
    if (in.texture_flags & BASE_TEXTURE_TRANSFORM) != 0u {
        base_uv = affine_uv(base_uv, material.base_uv_transform);
    }
    var texel = textureSample(base_texture, texture_sampler, base_uv);
    let reflected = environment_reflection(in);
    let uv_dx = dpdx(in.uv);
    let uv_dy = dpdy(in.uv);
    let grid_width = max(fwidth(in.lightmap.xy), vec2<f32>(0.00001));
    let surface_overlay = surface_diagnostic(in, grid_width);
    if surface_overlay.a > 0.0 { return surface_overlay; }
    if (in.texture_flags & ANIMATED_BASE) != 0u {
        texel = textureSampleGrad(blend_texture, texture_sampler, in.uv, uv_dx, uv_dy);
    }
    let blend_weight = in.color.a;
    if material.flags.y > 0.5 {
        let second = textureSampleGrad(blend_texture, texture_sampler, in.uv, uv_dx, uv_dy);
        texel = mix(texel, second, blend_weight);
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
    let alpha = select(1.0, texel.a, material.flags.z > 0.5 || (in.texture_flags & TEXTURE_ALPHA) != 0u) * in.opacity;
    let model_frame = (in.texture_flags & MODEL_NORMAL_FRAME) != 0u;
    var shading_normal = n;
    var phong_mask = 0.0;
    var ssbump = false;
    if globals.surface_view.z != 0u && material.helper.z > 0.5 && (model_frame || abs(in.tangent.w) > 0.5) {
        let scale = material.normal_transform.xy;
        let uv = in.uv * scale + material.normal_transform.zw;
        let normal_dx = uv_dx * scale;
        let normal_dy = uv_dy * scale;
        let sampled = textureSampleGrad(normal_texture, texture_sampler, uv, normal_dx, normal_dy);
        var normal = sampled.xyz * 2.0 - 1.0;
        if material.helper.z > 1.5 {
            let second_normal = textureSampleGrad(
                second_normal_texture, texture_sampler, uv, normal_dx, normal_dy,
            ).xyz * 2.0 - 1.0;
            normal = mix(normal, second_normal, blend_weight);
        }
        var frame_normal = in.normal;
        var tangent = in.tangent.xyz;
        var handedness = in.tangent.w;
        if !model_frame {
            frame_normal = n;
            tangent = normalize(tangent - n * dot(n, tangent));
            handedness = sign(handedness);
        }
        var bitangent: vec3<f32>;
        if model_frame && (in.texture_flags & MODEL_PHONG_FRAME) != 0u && material.phong_exponent > 0.0 {
            bitangent = in.model_bitangent;
            phong_mask = sampled.a;
        } else {
            bitangent = cross(frame_normal, tangent) * handedness;
        }
        let mapped = tangent * normal.x + bitangent * normal.y + frame_normal * normal.z;
        if material.ssbump > 0.5 && !model_frame {
            ssbump = true;
            shading_normal = bump_basis(frame_normal, tangent, bitangent) * sampled.rgb;
        } else if model_frame {
            let squared_length = dot(mapped, mapped);
            var reciprocal_length = 0x1.fffffep+127f;
            if squared_length != 0.0 {
                reciprocal_length = inverseSqrt(squared_length);
            }
            shading_normal = mapped * reciprocal_length;
        } else {
            shading_normal = normalize(mapped);
        }
    }
    var shade = 0.55 + 0.45 * clamp(dot(shading_normal, globals.light_dir.xyz) * 0.5 + 0.5, 0.0, 1.0);
    if globals.surface_view.w != 0u && material.helper.w > 0.5 {
        shade = 0.3 + 0.7 * max(dot(shading_normal, globals.light_dir.xyz), 0.0);
    }
    if ssbump {
        shade = clamp(dot(shading_normal, globals.light_dir.xyz), 0.0, 1.0);
    }
    if in.lighting_receiver == BAKED_DETAIL_RECEIVER || in.lighting_receiver == UNLIT_PROJECTED_RECEIVER { shade = 1.0; }
    var albedo = texel.rgb * in.color.rgb;
    if (in.texture_flags & DETAIL_LAYER) != 0u {
        let scale = material.detail_scale;
        let detail_rgb = textureSampleGrad(
            blend_texture, texture_sampler, in.uv * scale, uv_dx * scale, uv_dy * scale,
        ).rgb;
        let tinted_detail = detail_rgb * material.detail_tint_factor.rgb;
        albedo *= vec3<f32>(1.0) + (2.0 * tinted_detail - vec3<f32>(1.0)) * material.detail_tint_factor.a;
    }
    let masked_emission = material.emission.w > 1.5;
    let model_surface = (in.texture_flags & MODEL_SURFACE) != 0u;
    let emission = globals.surface_view.x == 0u && (in.texture_flags & MATERIAL_EMISSION) != 0u
        && material.emission.w > 0.5 && (!masked_emission || (material.emission.w > 2.5) == model_surface);
    let emission_weight = select(1.0, texel.a, masked_emission);
    let emitted = albedo * material.emission.rgb;
    var color = albedo * shade + reflected;
    if globals.lighting_settings.z > 0.5 && in.lighting_receiver == 1u {
        var radiance = albedo * in.authored_irradiance;
        if globals.lighting_settings.y > 0.5 {
            radiance = authored_radiance(in.world_position, shading_normal, albedo, phong_mask);
        }
        if emission { radiance = mix(radiance, emitted, emission_weight); }
        radiance *= globals.lighting_settings.w;
        radiance += reflected * (255.0 * globals.lighting_settings.w);
        color = radiance;
        if globals.scene_display.x < 0.5 { color = radiance / (vec3<f32>(1.0) + radiance); }
    } else if emission {
        color = mix(color, emitted, emission_weight);
    }
    if (in.lighting_receiver & 15u) == BAKED_RADIANCE_RECEIVER {
        if globals.scene_display.x > 0.5 {
            color = texel.rgb * in.color.rgb * detail_hdr_radiance[in.lighting_receiver >> 4u].rgb;
        } else {
            let radiance = texel.rgb * in.color.rgb * in.baked_irradiance;
            color = radiance / (vec3<f32>(1.0) + radiance);
        }
    }
    if material.helper.y < 0.5 {
        color = apply_fog(in.world_position, color);
    }
    return vec4<f32>(color, alpha);
}

struct BakedFace {
    dimensions: vec4<u32>,
    map_offsets: vec4<u32>,
}

struct BakedMaterial {
    options: vec4<u32>,
    color: vec4<f32>,
    diffuse_transform: array<array<vec4<f32>, 2>, 2>,
    normal_transform: array<array<vec4<f32>, 2>, 2>,
}

const BAKED_ALPHA_TEST: u32 = 4u;
const BAKED_TRANSLUCENT: u32 = 8u;
const BAKED_NEAREST_BASE: u32 = 16u;
const BAKED_CLAMP_S: u32 = 32u;
const BAKED_CLAMP_T: u32 = 64u;

@group(1) @binding(6) var<uniform> baked_material: BakedMaterial;
@group(2) @binding(0) var<storage, read> baked_samples: array<vec4<f32>>;
@group(2) @binding(1) var<storage, read> baked_faces: array<BakedFace>;
@group(2) @binding(2) var<uniform> baked_settings: vec4<f32>;

fn affine_uv(uv: vec2<f32>, rows: array<vec4<f32>, 2>) -> vec2<f32> {
    let point = vec3<f32>(uv, 1.0);
    return vec2<f32>(dot(rows[0].xyz, point), dot(rows[1].xyz, point));
}

fn repeat_pixel(pixel: vec2<i32>, dimensions: vec2<i32>) -> vec2<i32> {
    return ((pixel % dimensions) + dimensions) % dimensions;
}

fn baked_texture(image: texture_2d<f32>, uv: vec2<f32>) -> vec4<f32> {
    let dimensions = vec2<i32>(textureDimensions(image));
    let position = fract(uv) * vec2<f32>(dimensions) - 0.5;
    let first = vec2<i32>(floor(position));
    let fraction = fract(position);
    let a = textureLoad(image, repeat_pixel(first, dimensions), 0);
    let b = textureLoad(image, repeat_pixel(first + vec2<i32>(1, 0), dimensions), 0);
    let c = textureLoad(image, repeat_pixel(first + vec2<i32>(0, 1), dimensions), 0);
    let d = textureLoad(image, repeat_pixel(first + vec2<i32>(1, 1), dimensions), 0);
    return mix(mix(a, b, fraction.x), mix(c, d, fraction.x), fraction.y);
}

fn baked_base(in: VertexOut) -> vec4<f32> {
    let uv = affine_uv(in.uv, baked_material.diffuse_transform[0]);
    if (baked_material.options.w & BAKED_NEAREST_BASE) != 0u {
        let dimensions = vec2<i32>(textureDimensions(base_texture));
        let clamped = vec2<bool>((baked_material.options.w & BAKED_CLAMP_S) != 0u,
            (baked_material.options.w & BAKED_CLAMP_T) != 0u);
        let coordinate = select(fract(uv), clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)), clamped);
        let pixel = min(vec2<i32>(floor(coordinate * vec2<f32>(dimensions))), dimensions - vec2<i32>(1));
        return textureLoad(base_texture, pixel, 0);
    }
    return baked_texture(base_texture, uv);
}

fn baked_coverage(alpha: f32) -> f32 {
    if (baked_material.options.w & BAKED_ALPHA_TEST) != 0u && alpha < baked_material.color.a {
        discard;
    }
    return select(1.0, alpha, (baked_material.options.w & BAKED_TRANSLUCENT) != 0u);
}

fn baked_map(face: BakedFace, map: u32, uv: vec2<f32>) -> vec3<f32> {
    let last = face.dimensions.xy - vec2<u32>(1u);
    let position = clamp(uv, vec2<f32>(0.0), vec2<f32>(last));
    let first = vec2<u32>(floor(position));
    let second = min(first + vec2<u32>(1u), last);
    let fraction = fract(position);
    let offset = face.map_offsets[map];
    let a = baked_samples[offset + first.y * face.dimensions.x + first.x].xyz;
    let b = baked_samples[offset + first.y * face.dimensions.x + second.x].xyz;
    let c = baked_samples[offset + second.y * face.dimensions.x + first.x].xyz;
    let d = baked_samples[offset + second.y * face.dimensions.x + second.x].xyz;
    return mix(mix(a, b, fraction.x), mix(c, d, fraction.x), fraction.y);
}

fn baked_weights(in: VertexOut) -> vec3<f32> {
    var sampled = baked_texture(normal_texture, affine_uv(in.uv, baked_material.normal_transform[0])).xyz;
    if baked_material.options.x == 1u && baked_material.options.z != 0u {
        let second = baked_texture(second_normal_texture, affine_uv(in.uv, baked_material.normal_transform[1])).xyz;
        sampled = mix(sampled, second, clamp(in.opacity, 0.0, 1.0));
    }
    if baked_material.options.y == 2u { return sampled; }
    let maximum = max(max(abs(sampled.x), abs(sampled.y)), abs(sampled.z));
    let tangent_normal = normalize(sampled / maximum);
    let n = normalize(in.normal);
    let tangent = normalize(in.tangent.xyz - n * dot(in.tangent.xyz, n));
    let bitangent = cross(n, tangent) * in.tangent.w;
    let mapped = tangent * tangent_normal.x + bitangent * tangent_normal.y + n * tangent_normal.z;
    let basis = bump_basis(n, tangent, bitangent);
    let projected = max(transpose(basis) * mapped, vec3<f32>(0.0));
    let squared = projected * projected;
    return squared / dot(squared, vec3<f32>(1.0));
}

fn baked_tonemap(diffuse: vec3<f32>, tint: vec3<f32>, light: vec3<f32>, scale: vec3<f32>, weight_scale: f32) -> vec3<f32> {
    if globals.scene_display.x > 0.5 { return diffuse * tint * light * scale * weight_scale; }
    let logarithm = log2(select(vec3<f32>(1.0), diffuse, diffuse > vec3<f32>(0.0)))
        + log2(select(vec3<f32>(1.0), tint, tint > vec3<f32>(0.0)))
        + log2(select(vec3<f32>(1.0), light, light > vec3<f32>(0.0)))
        + log2(select(vec3<f32>(1.0), scale, scale > vec3<f32>(0.0)))
        + vec3<f32>(log2(select(1.0, weight_scale, weight_scale > 0.0)) + log2(baked_settings.x));
    let positive = vec3<bool>(diffuse.x > 0.0 && tint.x > 0.0 && light.x > 0.0 && scale.x > 0.0 && weight_scale > 0.0,
        diffuse.y > 0.0 && tint.y > 0.0 && light.y > 0.0 && scale.y > 0.0 && weight_scale > 0.0,
        diffuse.z > 0.0 && tint.z > 0.0 && light.z > 0.0 && scale.z > 0.0 && weight_scale > 0.0);
    let small = exp2(-abs(logarithm));
    let denominator = vec3<f32>(1.0) + small;
    let mapped = select(small / denominator, vec3<f32>(1.0) / denominator, logarithm >= vec3<f32>(0.0));
    return select(vec3<f32>(0.0), mapped, positive);
}

fn baked_diffuse(in: VertexOut, first: vec4<f32>) -> vec3<f32> {
    var diffuse = first.rgb;
    if baked_material.options.x == 1u && (baked_material.options.w & 2u) != 0u {
        let second = baked_texture(blend_texture, affine_uv(in.uv, baked_material.diffuse_transform[1]));
        diffuse = mix(first.rgb, second.rgb, clamp(in.opacity, 0.0, 1.0));
    }
    if baked_material.options.x == 2u {
        let detail = baked_texture(blend_texture, affine_uv(in.uv, baked_material.diffuse_transform[1]));
        diffuse = mix(first.rgb, detail.rgb, detail.a);
        if (baked_material.options.w & 1u) != 0u {
            diffuse = clamp(clamp(first.rgb * 2.0, vec3<f32>(0.0), vec3<f32>(1.0)) * detail.a + vec3<f32>(1.0 - detail.a), vec3<f32>(0.0), vec3<f32>(1.0)) * detail.rgb;
        }
    }
    return diffuse;
}

@fragment
fn fs_baked(in: VertexOut) -> @location(0) vec4<f32> {
    let surface_overlay = surface_diagnostic(in, max(fwidth(in.lightmap.xy), vec2<f32>(0.00001)));
    if surface_overlay.a > 0.0 { return surface_overlay; }
    let first = baked_base(in);
    let alpha = baked_coverage(first.a);
    let diffuse = baked_diffuse(in, first);
    let face = baked_faces[in.lighting_receiver >> 4u];
    var light = baked_map(face, 0u, in.lightmap.xy);
    var scale = vec3<f32>(1.0);
    var weight_scale = 1.0;
    if face.dimensions.z == 4u {
        let a = baked_map(face, 1u, in.lightmap.xy);
        let b = baked_map(face, 2u, in.lightmap.xy);
        let c = baked_map(face, 3u, in.lightmap.xy);
        scale = max(a, max(b, c));
        let weights = baked_weights(in);
        weight_scale = max(weights.x, max(weights.y, weights.z));
        let divisor = select(vec3<f32>(1.0), scale, scale > vec3<f32>(0.0));
        let normalized_weights = weights / select(1.0, weight_scale, weight_scale > 0.0);
        light = (a / divisor) * normalized_weights.x + (b / divisor) * normalized_weights.y + (c / divisor) * normalized_weights.z;
    }
    let color = baked_tonemap(diffuse, in.color.rgb * baked_material.color.rgb, light, scale, weight_scale);
    return vec4<f32>(apply_fog(in.world_position, color), alpha);
}

@fragment
fn fs_baked_unlit(in: VertexOut) -> @location(0) vec4<f32> {
    let surface_overlay = surface_diagnostic(in, max(fwidth(in.lightmap.xy), vec2<f32>(0.00001)));
    if surface_overlay.a > 0.0 { return surface_overlay; }
    let first = baked_base(in);
    let alpha = baked_coverage(first.a);
    let diffuse = baked_diffuse(in, first);
    let color = baked_tonemap(diffuse, in.color.rgb * baked_material.color.rgb, vec3<f32>(1.0), vec3<f32>(1.0), 1.0);
    return vec4<f32>(apply_fog(in.world_position, color), alpha);
}

@fragment
fn fs_baked_vertex(in: VertexOut) -> @location(0) vec4<f32> {
    let first = baked_base(in);
    let alpha = baked_coverage(first.a);
    if globals.surface_view.x != 0u {
        return vec4<f32>(vec3<f32>(0.65) * in.color.rgb, 1.0);
    }
    let diffuse = baked_diffuse(in, first);
    let color = baked_tonemap(diffuse, in.color.rgb * baked_material.color.rgb, in.baked_irradiance, vec3<f32>(1.0), 1.0);
    return vec4<f32>(apply_fog(in.world_position, color), alpha);
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
