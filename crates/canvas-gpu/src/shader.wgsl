// One pipeline draws everything: flat and gradient fills (mode 0), text
// coverage out of the glyph atlas (mode 1) and pictures (mode 2). Colours
// arrive premultiplied; the target is a non-sRGB format, so blending happens
// on the same numbers the CPU rasteriser blends.

struct Frame {
    size: vec2<f32>,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;
@group(2) @binding(0) var picture: texture_2d<f32>;
@group(2) @binding(1) var picture_sampler: sampler;

struct VertexIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) mode: f32,
}

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) mode: f32,
}

@vertex
fn vs_main(v: VertexIn) -> VertexOut {
    var out: VertexOut;
    let x = v.pos.x / frame.size.x * 2.0 - 1.0;
    let y = 1.0 - v.pos.y / frame.size.y * 2.0;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = v.uv;
    out.color = v.color;
    out.mode = v.mode;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Sample unconditionally: derivatives need uniform control flow.
    let cover = textureSample(atlas, atlas_sampler, in.uv).r;
    let pic = textureSample(picture, picture_sampler, in.uv);
    if (in.mode < 0.5) {
        return in.color;
    }
    if (in.mode < 1.5) {
        return in.color * cover;
    }
    return pic * in.color.a;
}
