// Fixed-function style map shading, done in gamma space like D3D9 texture stages:
// diffuse × lightmap × scale, clamped. Textures are bound as non-sRGB so samples are raw.
#import bevy_pbr::forward_io::VertexOutput

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: vec4<f32>; // x: alpha cutoff, y: lightmap scale, z: additive
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var diffuse_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var diffuse_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var lightmap_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var lightmap_sampler: sampler;

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3(2.4)), c / 12.92, c <= vec3(0.04045));
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let color = textureSample(diffuse_texture, diffuse_sampler, in.uv);
    let light = textureSample(lightmap_texture, lightmap_sampler, in.uv_b).rgb;
    if color.a < params.x {
        discard;
    }
    if params.z > 0.5 {
        // Bevy's AlphaMode::Add is premultiplied blending: alpha 0 makes it src + dst.
        // Glow surfaces are unlit (lightmapping would darken them).
        return vec4(srgb_to_linear(color.rgb * color.a), 0.0);
    }
    return vec4(srgb_to_linear(min(color.rgb * light * params.y, vec3(1.0))), color.a);
}
