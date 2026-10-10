// Screen effects of `gfx.rs`, one fullscreen pass before bloom and tonemapping (scene-linear
// colours): contrast, camera-turn motion blur, radial blur and speed lines, hit flash, low-health
// desaturation and heartbeat, film grain. `gfx.rs` removes the pass while every effect is idle.
#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

struct Fx {
    time: f32,
    grain: f32,
    hurt: f32,
    low: f32,
    speed: f32,
    aspect: f32,
    pulse: f32,
    contrast: f32,
    blur: vec2<f32>,
    pad2: vec2<f32>,
}

@group(0) @binding(0) var screen: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> fx: Fx;

const TAPS: i32 = 8;

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453);
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3(0.2126, 0.7152, 0.0722));
}

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let to_c = uv - vec2(0.5);
    // 0 at the centre, 1 in the corners
    let r = length(to_c * vec2(fx.aspect, 1.0)) / length(vec2(fx.aspect * 0.5, 0.5));
    let edge = smoothstep(0.3, 1.0, r);

    var col = textureSampleLevel(screen, samp, uv, 0.0).rgb;
    // motion blur along the camera's turn, radial blur towards the centre at speed
    let smear = fx.blur - to_c * (fx.speed * 0.14 * edge);
    if length(smear) > 0.0004 {
        var acc = vec3(0.0);
        for (var i = 0; i < TAPS; i++) {
            let f = f32(i) / f32(TAPS - 1) - 0.5;
            acc += textureSampleLevel(screen, samp, uv + smear * f, 0.0).rgb;
        }
        col = acc / f32(TAPS);
    }
    if fx.contrast != 1.0 {
        // around the display encoding's mid grey, not the linear light's
        let g = pow(max(col, vec3(0.0)), vec3(1.0 / 2.2));
        col = pow(max((g - 0.5) * fx.contrast + 0.5, vec3(0.0)), vec3(2.2));
    }
    if fx.speed > 0.01 {
        let streak = hash(vec2(floor(atan2(to_c.y, to_c.x) * 48.0), floor(fx.time * 18.0)));
        col += vec3(step(0.86, streak) * smoothstep(0.4, 1.0, r) * fx.speed * 0.22);
    }
    // hit flash: red wash, strongest at the edges
    col += vec3(0.55, 0.03, 0.02) * fx.hurt * (0.12 + 0.88 * edge);
    // low health: drained colour, darkened edges that throb with the heartbeat
    col = mix(col, vec3(luma(col)), fx.low * 0.8);
    col *= 1.0 - fx.low * 0.4 * edge;
    col *= 1.0 - fx.pulse * 0.35 * edge;
    if fx.grain > 0.0 {
        let dims = vec2<f32>(textureDimensions(screen));
        let n = hash(floor(uv * dims) + fract(fx.time) * 61.0) - 0.5;
        col += col * n * fx.grain * 0.45 + vec3(n * fx.grain * 0.02);
    }
    return vec4(max(col, vec3(0.0)), 1.0);
}
