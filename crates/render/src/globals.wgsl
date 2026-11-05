struct AuthoredLight {
    origin_radius: vec4<f32>,
    intensity_cap: vec4<f32>,
    coefficients_spot: vec4<f32>,
    direction_inner: vec4<f32>,
    outer_exponent_fade: vec4<f32>,
}

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
    scene_display: vec4<f32>,
    lighting_lights: array<AuthoredLight, 32>,
}

@group(0) @binding(0) var<uniform> globals: Globals;

@group(0) @binding(1) var<storage, read> detail_hdr_radiance: array<vec4<f32>>;
