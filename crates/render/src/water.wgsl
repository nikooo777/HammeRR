struct WaterParams {
    screen: vec4<f32>,
    fog_color: vec4<f32>,
    reflection_tint: vec4<f32>,
}

@group(1) @binding(0) var refraction_capture: texture_2d<f32>;
@group(1) @binding(1) var capture_sampler: sampler;
@group(1) @binding(2) var normal_map: texture_2d<f32>;
@group(1) @binding(3) var normal_sampler: sampler;
@group(1) @binding(4) var<uniform> water: WaterParams;
@group(1) @binding(5) var reflection_capture: texture_2d<f32>;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(8) tangent: vec4<f32>,
}

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) eye: vec3<f32>,
    @location(2) world_position: vec3<f32>,
}

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    let up = vec3<f32>(0.0, 0.0, 1.0);
    let tangent = in.tangent.xyz;
    let bitangent = cross(up, tangent) * sign(in.tangent.w);
    let eye = globals.eye.xyz - in.position;
    out.clip = globals.view_proj * vec4<f32>(in.position, 1.0);
    out.uv = in.uv;
    out.eye = vec3<f32>(dot(eye, tangent), dot(eye, bitangent), eye.z);
    out.world_position = in.position;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let uv = in.clip.xy * water.screen.xy;
    let unwarped = textureSample(refraction_capture, capture_sampler, uv);
    let packed = textureSample(normal_map, normal_sampler, in.uv);
    let normal = packed.xyz * 2.0 - 1.0;
    let offset = normal.xy * packed.a;
    let scale = water.screen.zw * unwarped.a;
    let refracted = textureSample(refraction_capture, capture_sampler, uv + offset * scale.x);
    var reflected = vec3<f32>(0.0);
    if any(water.reflection_tint.rgb > vec3<f32>(0.0)) {
        let mirror_uv = vec2<f32>(uv.x, 1.0 - uv.y);
        reflected = textureSampleLevel(
            reflection_capture, capture_sampler, mirror_uv + offset * scale.y, 0.0,
        ).rgb * water.reflection_tint.rgb;
    }
    let depth = clamp(refracted.a - 0.05, 0.0, 1.0);
    let edge = clamp((refracted.a - 0.05) * 20.0, 0.0, 1.0);
    let grazing = 1.0 - clamp(dot(normalize(in.eye), normal), 0.0, 1.0);
    let squared = grazing * grazing;
    let fresnel = edge * squared * squared * grazing;
    let color = mix(mix(refracted.rgb, water.fog_color.rgb, depth), reflected, fresnel);
    return vec4<f32>(apply_fog(in.world_position, color), 1.0);
}
