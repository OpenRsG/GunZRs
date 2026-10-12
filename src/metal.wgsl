// Polished, brushed orange metal for the main buttons (`ui::metal`), clipped to the Notch shape.
// The face is a shallow convex plate with a bevelled rim; its normal reflects a procedural
// studio (sky, dark ground, softboxes, a black flag) smeared across the brushing, with a clear
// coat on top and a moving light strip for the sweep. Output is linear, straight alpha.
#import bevy_ui::ui_vertex_output::UiVertexOutput

struct Metal {
    // metal tint (linear)
    color: vec4<f32>,
    // node width, height and notch (logical px), brightness
    shape: vec4<f32>,
    // sweep position 0..1 (negative: none)
    anim: vec4<f32>,
}

@group(1) @binding(0)
var<uniform> m: Metal;

const BEVEL: f32 = 7.0;

fn hash(x: i32, y: i32) -> f32 {
    var h = u32(x) * 374761393u + u32(y) * 668265263u;
    h = (h ^ (h >> 13u)) * 1274126177u;
    return f32(h ^ (h >> 16u)) / 4294967295.0;
}

fn vnoise(p: vec2<f32>) -> f32 {
    let i = vec2<i32>(floor(p));
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(hash(i.x, i.y), hash(i.x + 1, i.y), u.x),
        mix(hash(i.x, i.y + 1), hash(i.x + 1, i.y + 1), u.x),
        u.y,
    );
}

// 1 inside the rectangle `c` +- `hs` (softened by `w`)
fn rect(p: vec2<f32>, c: vec2<f32>, hs: vec2<f32>, w: f32) -> f32 {
    let q = abs(p - c) - hs;
    return (1.0 - smoothstep(-w, w, q.x)) * (1.0 - smoothstep(-w, w, q.y));
}

// The room in direction `r` (y up, z toward the viewer), blurred by `w`.
fn room(r: vec3<f32>, w: f32, sweep: f32) -> vec3<f32> {
    let y = r.y;
    let above = smoothstep(-w - 0.015, w + 0.015, y);
    let sky = mix(vec3(1.05, 1.0, 0.95), vec3(0.45, 0.5, 0.6), smoothstep(0.0, 1.0, y));
    let ground = mix(vec3(0.3, 0.25, 0.2), vec3(0.04, 0.04, 0.05), smoothstep(0.0, -0.8, y));
    var c = mix(ground, sky, above);
    // a black flag standing in the sky
    c *= 1.0 - 0.85 * above * rect(r.xy, vec2(0.08, 0.3), vec2(0.05, 0.5), w + 0.02);
    // lights, in front of the plate only
    let front = smoothstep(-0.25, 0.25, r.z);
    var lights = vec3(2.2, 2.1, 1.9) * rect(r.xy, vec2(-0.3, 0.3), vec2(0.15, 0.15), w + 0.02);
    lights += vec3(2.2, 2.4, 2.8) * rect(r.xy, vec2(0.34, 0.1), vec2(0.03, 0.7), w + 0.01);
    lights += vec3(1.8) * rect(r.xy, vec2(0.0, 0.64), vec2(0.7, 0.03), w + 0.01);
    if sweep >= 0.0 {
        let x = r.x + 0.15 * y - mix(-0.6, 0.6, sweep);
        lights += vec3(5.0, 4.6, 4.0) * (1.0 - smoothstep(0.015, 0.04 + w, abs(x)));
    }
    return c + lights * front;
}

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let size = m.shape.xy;
    let notch = m.shape.z;
    let p = in.uv * size;
    // distance (px) to the nearest edge and the direction away from it (y up)
    var d = p.x;
    var inward = vec2(1.0, 0.0);
    if size.x - p.x < d { d = size.x - p.x; inward = vec2(-1.0, 0.0); }
    if p.y < d { d = p.y; inward = vec2(0.0, -1.0); }
    if size.y - p.y < d { d = size.y - p.y; inward = vec2(0.0, 1.0); }
    let dn = (size.x + size.y - notch - p.x - p.y) * 0.70710678;
    if dn < d { d = dn; inward = vec2(-0.70710678, 0.70710678); }
    let cover = clamp(d / max(fwidth(d), 1e-4) + 0.5, 0.0, 1.0);

    // normal: bevel tilts out at the rim, the face bows a little, the brushing ripples it
    let t = clamp(d / BEVEL, 0.0, 1.0);
    let tilt = 1.1 * pow(1.0 - t, 1.5);
    let c = vec2(p.x - size.x * 0.5, size.y * 0.5 - p.y) / (size * 0.5);
    let grain = vnoise(vec2(p.x * 0.025, p.y * 1.2)) + 0.5 * vnoise(vec2(p.x * 0.09, p.y * 2.7));
    let n = normalize(vec3(
        -inward.x * tilt + c.x * 0.22,
        -inward.y * tilt + c.y * 0.3 + (grain - 0.75) * 0.1,
        1.0,
    ));
    let r = vec3(2.0 * n.z * n.x, 2.0 * n.z * n.y, 2.0 * n.z * n.z - 1.0);

    // metal: the room smeared across the grain (brushed lines run along x)
    var metal = vec3(0.0);
    var wsum = 0.0;
    for (var k = -3; k <= 3; k++) {
        let wk = 1.0 - abs(f32(k)) * 0.22;
        metal += wk * room(normalize(r + vec3(0.0, f32(k) * 0.05, 0.0)), 0.05, m.anim.x);
        wsum += wk;
    }
    metal = metal / wsum * m.color.rgb;
    // clear coat: sharp, white, stronger at grazing angles
    let fres = 0.04 + 0.96 * pow(1.0 - n.z, 5.0);
    var col = metal * (1.0 - fres) + room(r, 0.012, m.anim.x) * fres;
    // the machined line where bevel meets face
    let seam = (d - BEVEL) / 0.8;
    col *= 1.0 - 0.3 * exp(-seam * seam);
    // clip per channel, but what overflows washes out toward white like an overexposed print
    let over = max(col * m.shape.w - 1.0, vec3(0.0));
    col = min(col * m.shape.w, vec3(1.0)) + (over.r + over.g + over.b) * 0.12;
    return vec4(col, cover);
}
