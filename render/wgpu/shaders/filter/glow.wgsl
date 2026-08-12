/// How many stops a gradient glow may carry. SWF allows fifteen.
const STOP_CAPACITY: u32 = 16u;

struct Filter {
    color: vec4<f32>,
    strength: f32,
    inner: u32,
    knockout: u32,
    composite_source: u32,
    /// Zero for a plain glow, which is one colour and needs no ramp.
    stop_count: u32,
    padding: vec3<u32>,
    stop_colors: array<vec4<f32>, 16>,
    /// Sixteen ratios packed four to a vector, because a uniform array of scalars would be padded
    /// out to sixteen bytes an element.
    stop_ratios: array<vec4<f32>, 4>,
}

@group(0) @binding(0) var texture: texture_2d<f32>;
@group(0) @binding(1) var texture_sampler: sampler;
@group(0) @binding(2) var<uniform> filter_args: Filter;
@group(0) @binding(3) var blurred: texture_2d<f32>;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) source_uv: vec2<f32>,
    @location(1) blur_uv: vec2<f32>,
};

struct VertexInput {
    /// The position of the vertex in texture space (topleft 0,0, bottomright 1,1)
    @location(0) position: vec2<f32>,

    /// The coordinate of the source texture to sample in texture space (topleft 0,0, bottomright 1,1)
    @location(1) source_uv: vec2<f32>,

    /// The coordinate of the blur texture to sample in texture space (topleft 0,0, bottomright 1,1)
    @location(2) blur_uv: vec2<f32>,
};

@vertex
fn main_vertex(in: VertexInput) -> VertexOutput {
    // Convert texture space (topleft 0,0 to bottomright 1,1) to render space (topleft -1,1 to bottomright 1,-1)
    let pos = vec4<f32>((in.position.x * 2.0 - 1.0), (1.0 - in.position.y * 2.0), 0.0, 1.0);
    return VertexOutput(pos, in.source_uv, in.blur_uv);
}

fn ratio_at(i: u32) -> f32 {
    return filter_args.stop_ratios[i / 4u][i % 4u];
}

/// The colour and opacity a gradient glow has at `t`, where `t` runs from nothing at the outside of
/// the glow to fully covered at the object.
///
/// A gradient glow is a ramp: it changes colour with distance, which is the whole reason it is a
/// different filter to a glow. Collapsing it to a single stop keeps the shape and loses the
/// colour -- a weapon whose glow runs red to orange comes out flat orange, and one that runs
/// through a second hue comes out wrong outright.
fn ramp(t: f32) -> vec4<f32> {
    if (filter_args.stop_count == 0u) {
        // A plain glow. One colour, opacity straight from how much the blur covered.
        return vec4<f32>(filter_args.color.rgb, filter_args.color.a * t);
    }

    var previous_ratio = ratio_at(0u);
    var previous_color = filter_args.stop_colors[0];
    if (t <= previous_ratio) {
        return previous_color;
    }

    for (var i = 1u; i < filter_args.stop_count && i < STOP_CAPACITY; i = i + 1u) {
        let ratio = ratio_at(i);
        let color = filter_args.stop_colors[i];
        if (t <= ratio) {
            // Two stops may share a ratio to make a hard edge; never divide by that.
            let span = max(ratio - previous_ratio, 1e-5);
            return mix(previous_color, color, (t - previous_ratio) / span);
        }
        previous_ratio = ratio;
        previous_color = color;
    }

    return previous_color;
}

@fragment
fn main_fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let inner = filter_args.inner > 0u;
    let knockout = filter_args.knockout > 0u;
    let composite_source = filter_args.composite_source > 0u;
    var blur = textureSample(blurred, texture_sampler, in.blur_uv).a;
    var dest = textureSample(texture, texture_sampler, in.source_uv);

    if (in.blur_uv.x < 0.0 || in.blur_uv.x > 1.0 || in.blur_uv.y < 0.0 || in.blur_uv.y > 1.0) {
        blur = 0.0;
    }

    // [NA] It'd be nice to use hardware blending but the operation is too complex :( Only knockouts would work.

    // How far along the glow this pixel is. A plain glow reads its one colour out of this too, at
    // exactly the opacity it had before.
    var coverage = blur;
    if (inner) {
        coverage = 1.0 - blur;
    }
    let stop = ramp(saturate(coverage * filter_args.strength));
    let alpha = stop.a;

    // Start with 1 alpha because we'll be multiplying the whole thing
    var color = vec4<f32>(stop.r, stop.g, stop.b, 1.0);
    if (inner) {
        if (knockout) {
            color = color * alpha * dest.a;
        } else if (composite_source) {
            color = color * alpha * dest.a + dest * (1.0 - alpha);
        } else {
            // [NA] Yes it's intentional that the !composite_source is different for inner/outer. Just Flash things.
            color = color * alpha * dest.a;
        }
    } else {
        if (knockout) {
            color = color * alpha * (1.0 - dest.a);
        } else if (composite_source) {
            color = color * alpha * (1.0 - dest.a) + dest;
        } else {
            color = color * alpha;
        }
    }

    return color;
}
