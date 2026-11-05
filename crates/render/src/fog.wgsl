fn apply_fog(position: vec3<f32>, color: vec3<f32>) -> vec3<f32> {
    if globals.fog_color.a <= 0.0 { return color; }
    let offset = position - globals.eye.xyz;
    let depth = select(dot(offset, globals.forward.xyz), length(offset), globals.fog_range.z > 0.5);
    let factor = clamp(depth * globals.fog_range.y - globals.fog_range.x, 0.0, globals.fog_color.a);
    return mix(color, globals.fog_color.rgb, factor * factor);
}
