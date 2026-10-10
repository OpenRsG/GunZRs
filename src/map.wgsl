// Fixed-function style map shading, done in gamma space like D3D9 texture stages:
// diffuse × lightmap × scale, clamped. Textures are bound as non-sRGB so samples are raw.
#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::{
    clustered_forward as clustering,
    mesh_view_bindings as view_bindings,
    mesh_view_types,
}
#ifdef DISTANCE_FOG
#import bevy_pbr::{mesh_view_bindings::{fog, view}, pbr_functions::apply_fog}
#endif

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: vec4<f32>; // x: alpha cutoff, y: lightmap scale, z: additive
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var diffuse_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var diffuse_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var lightmap_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var lightmap_sampler: sampler;

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3(2.4)), c / 12.92, c <= vec3(0.04045));
}

// Point lights of the scene (muzzle flashes, explosions: `light.rs`): Lambert on the polygon
// normal, bevy's range falloff and units, added on top of the lightmap. Fill lights of actors
// opt out (`affects_lightmapped_mesh_diffuse` false). Nothing runs when there are none.
fn dynamic_light(in: VertexOutput) -> vec3<f32> {
    let v = view_bindings::view;
    let view_z = dot(vec4<f32>(
        v.view_from_world[0].z, v.view_from_world[1].z, v.view_from_world[2].z, v.view_from_world[3].z
    ), in.world_position);
    let cluster_index = clustering::view_fragment_cluster_index(in.position.xy, view_z, false);
    let ranges = clustering::unpack_clusterable_object_index_ranges(cluster_index);
    let n = normalize(in.world_normal);
    var sum = vec3(0.0);
    for (var i = ranges.first_point_light_index_offset; i < ranges.first_spot_light_index_offset; i = i + 1u) {
        let light = &view_bindings::clustered_lights.data[clustering::get_clusterable_object_id(i)];
        if ((*light).flags & mesh_view_types::POINT_LIGHT_FLAGS_AFFECTS_LIGHTMAPPED_MESH_DIFFUSE_BIT) == 0u {
            continue;
        }
        let to_light = (*light).position_radius.xyz - in.world_position.xyz;
        let d2 = dot(to_light, to_light);
        let n_dot_l = max(dot(n, to_light * inverseSqrt(max(d2, 0.0001))), 0.0);
        // bevy's `getDistanceAttenuation`: smooth window at the range, inverse square inside
        let window = clamp(1.0 - pow(d2 * (*light).color_inverse_square_range.w, 2.0), 0.0, 1.0);
        sum += (*light).color_inverse_square_range.rgb * (window * window / max(d2, 0.0001)) * n_dot_l;
    }
    // bevy's diffuse term is albedo / pi, then the view's exposure
    return sum * v.exposure * 0.3183;
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
    var lit = vec4(srgb_to_linear(min(color.rgb * light * params.y, vec3(1.0))), color.a);
    lit = vec4(lit.rgb + srgb_to_linear(color.rgb) * dynamic_light(in), lit.a);
    // GRAPHICS' distance fog (the camera carries a `DistanceFog` only then)
#ifdef DISTANCE_FOG
    lit = apply_fog(fog, lit, in.world_position.xyz, view.world_position.xyz, in.position.xy);
#endif
    return lit;
}
