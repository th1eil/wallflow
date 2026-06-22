struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) in_vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;
    let x = f32(i32(in_vertex_index == 1u) * 4 - 1);
    let y = f32(i32(in_vertex_index == 2u) * 4 - 1);
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.tex_coords = vec2<f32>((x + 1.0) * 0.5, 1.0 - (y + 1.0) * 0.5);
    return out;
}

@group(0) @binding(0) var t_diffuse: texture_2d<f32>;
@group(0) @binding(1) var s_diffuse: sampler;

// Unified uniform struct — shared by both fs_filter and fs_blur.
// Layout (std140): 9 f32 fields + 1 f32 padding = 40 bytes total.
struct ShaderUniform {
    // filter params (used by fs_filter)
    brightness: f32,   // offset 0
    contrast: f32,     // offset 4
    saturation: f32,   // offset 8
    temperature: f32,  // offset 12
    // blur params (used by fs_blur)
    direction: vec2<f32>, // offset 16 (align 8)
    radius: f32,          // offset 24
    width: f32,           // offset 28
    height: f32,          // offset 32
}

@group(0) @binding(2) var<uniform> params: ShaderUniform;

@fragment
fn fs_filter(in: VertexOutput) -> @location(0) vec4<f32> {
    var color = textureSample(t_diffuse, s_diffuse, in.tex_coords);

    // 1. Brightness
    color.r = color.r + params.brightness;
    color.g = color.g + params.brightness;
    color.b = color.b + params.brightness;

    // 2. Contrast
    let c = params.contrast + 1.0;
    color.r = (color.r - 0.5) * c + 0.5;
    color.g = (color.g - 0.5) * c + 0.5;
    color.b = (color.b - 0.5) * c + 0.5;

    // 3. Saturation
    let luma = 0.299 * color.r + 0.587 * color.g + 0.114 * color.b;
    let s = params.saturation + 1.0;
    color.r = mix(luma, color.r, s);
    color.g = mix(luma, color.g, s);
    color.b = mix(luma, color.b, s);

    // 4. Color Temperature (warm/cool)
    color.r = color.r + params.temperature * 0.1;
    color.g = color.g + params.temperature * 0.05;
    color.b = color.b - params.temperature * 0.1;

    return clamp(color, vec4<f32>(0.0), vec4<f32>(1.0));
}

@fragment
fn fs_blur(in: VertexOutput) -> @location(0) vec4<f32> {
    let radius = i32(params.radius);
    if radius <= 0 {
        return textureSample(t_diffuse, s_diffuse, in.tex_coords);
    }

    let texel_size = vec2<f32>(1.0 / params.width, 1.0 / params.height);
    let step = texel_size * params.direction;

    var color = vec4<f32>(0.0);
    var total_weight = 0.0;

    for (var i = -radius; i <= radius; i = i + 1) {
        let weight = exp(-f32(i * i) / (2.0 * params.radius * params.radius));
        let coords = in.tex_coords + f32(i) * step;
        color = color + textureSample(t_diffuse, s_diffuse, coords) * weight;
        total_weight = total_weight + weight;
    }

    return color / total_weight;
}
