// The editor's colour preview (a port of the reference's `color.frag`):
// grade (contrast, exposure), saturation around Rec.709 luma, temperature,
// quantize to 1/255, a 5x5 binomial unsharp, and a vignette inside the crop
// relative to the crop size. Input is gamma-encoded (an Rgba8Unorm view);
// the output is re-linearised only for an sRGB target.

struct Uniforms {
    // exposure, contrast, saturation, temperature
    grade: vec4<f32>,
    // sharpness, vignette, crop active, sRGB target
    fx: vec4<f32>,
    // crop x, y, w, h (normalised)
    crop: vec4<f32>,
    // vignette colour (gamma), unused
    background: vec4<f32>,
    // source width, height, unused, unused
    size: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var frame: texture_2d<f32>;
@group(0) @binding(2) var frame_sampler: sampler;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    let p = corners[index];
    var out: VertexOut;
    out.position = vec4<f32>(p, 0.0, 1.0);
    out.uv = vec2<f32>((p.x + 1.0) * 0.5, (1.0 - p.y) * 0.5);
    return out;
}

fn graded(uv: vec2<f32>) -> vec3<f32> {
    let c = textureSampleLevel(frame, frame_sampler, uv, 0.0).rgb;
    var rgb = (c - vec3<f32>(0.5)) * u.grade.y + vec3<f32>(0.5 + u.grade.x);
    let luma = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    rgb = mix(vec3<f32>(luma), rgb, u.grade.z);
    rgb = rgb * vec3<f32>(1.0 + u.grade.w, 1.0, 1.0 - u.grade.w);
    rgb = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    return floor(rgb * 255.0 + vec3<f32>(0.5)) / 255.0;
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let size = u.size.xy;
    let texel = vec2<f32>(1.0) / size;
    var rgb = graded(in.uv);
    let sharpness = u.fx.x;
    if (sharpness > 0.0) {
        let weights = array<f32, 5>(1.0, 4.0, 6.0, 4.0, 1.0);
        var blurred = vec3<f32>(0.0);
        for (var j = 0; j < 5; j = j + 1) {
            for (var i = 0; i < 5; i = i + 1) {
                let offset = vec2<f32>(f32(i - 2), f32(j - 2)) * texel;
                blurred = blurred + graded(in.uv + offset) * (weights[i] * weights[j]);
            }
        }
        blurred = blurred / 256.0;
        rgb = clamp(rgb * (1.0 + sharpness) - blurred * sharpness, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    let vignette = u.fx.y;
    if (vignette > 0.0) {
        let p = in.uv * size;
        let crop = u.crop * vec4<f32>(size, size);
        let inside = p.x >= crop.x && p.x <= crop.x + crop.z && p.y >= crop.y && p.y <= crop.y + crop.w;
        if (inside) {
            let centre = crop.xy + crop.zw * 0.5;
            let radius = max(crop.z, crop.w) * 0.72;
            let d = length(p - centre) / radius;
            let f = clamp((d - 0.35) / 0.65, 0.0, 1.0) * vignette;
            rgb = mix(rgb, u.background.rgb, f);
        }
    }
    if (u.fx.w > 0.5) {
        rgb = to_linear(rgb);
    }
    return vec4<f32>(rgb, 1.0);
}
