@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    // Full-screen triangle. No vertex buffer required.
    var p = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(3.0, 1.0),
    );
    return vec4<f32>(p[vi], 0.0, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    // Placeholder colour until the guest framebuffer is wired. A soft
    // gradient signals "no signal" without being visually harsh.
    let t = 0.5 + 0.5 * sin(vec3<f32>(0.0, 1.0, 2.0));
    return vec4<f32>(t * 0.25, 1.0);
}
