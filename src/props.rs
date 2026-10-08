//! Map `OBJECTLIST` props: every `.elu` listed in the map XML is spawned (sky domes, fires, light
//! shafts, water, fans, ...) with its `.elu.ani`, and the cloth props listed in the map's
//! `flag.xml` (flags, curtains) are waved by [`flutter`]. Layouts and evidence: `docs/formats.md`.

use crate::{
    ani,
    anim::Loop,
    effect::{Clock, FxAssets, Loader},
    elu::{self, Elu},
    level::Level,
    model::node_batches,
    mrs::Vfs,
    view,
};
use bevy::{camera::visibility::NoFrustumCulling, prelude::*};
use std::{
    collections::HashMap,
    f32::consts::{PI, TAU},
    sync::Arc,
};

/// `<FLAG NAME DIRECTION POWER><WINDTYPE TYPE DELAY/></FLAG>` of `flag.xml`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Wind {
    /// Degrees, 0 = map +X, 90 = map +Y (inferred: it makes the Town flags, which are
    /// authored as streamers along map Y, blow along their length).
    direction: f32,
    power: f32,
    /// Milliseconds; used as the wave period (inferred).
    delay: f32,
}

/// Cloth entries of `<map dir>/flag.xml` by lower-case `NAME` (the ELU file name without the map
/// prefix). Entries without `DIRECTION`/`POWER`/`WINDTYPE` (test_a) carry no wind: not waved.
fn read_flags(vfs: &Vfs, dir: &str) -> HashMap<String, Wind> {
    let Ok(bytes) = vfs.read(&format!("{dir}flag.xml")) else {
        return HashMap::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let parsed = roxmltree::Document::parse(&text);
    let doc = match parsed {
        Ok(d) => d,
        Err(e) => {
            warn!("{dir}flag.xml: {e}");
            return HashMap::new();
        }
    };
    let num = |n: roxmltree::Node, k: &str| n.attribute(k)?.parse::<f32>().ok();
    doc.descendants()
        .filter(|n| n.has_tag_name("FLAG"))
        .filter_map(|f| {
            let delay = num(f.children().find(|c| c.has_tag_name("WINDTYPE"))?, "DELAY")?;
            Some((
                f.attribute("NAME")?.to_ascii_lowercase(),
                Wind {
                    direction: num(f, "DIRECTION")?,
                    power: num(f, "POWER")?,
                    delay,
                },
            ))
        })
        .collect()
}

/// A waved prop node: the cloth hangs from its top edge and its vertices move along the cloth's
/// horizontal normal by a travelling wave that grows with the distance from that edge
/// (inferred model; retail simulates cloth in code, its `.elu.ani` is a constant identity track).
/// Everything is computed in ELU world space (Y up), then mapped back to node space.
#[derive(Component)]
pub struct Cloth {
    elu: Arc<Elu>,
    node: usize,
    /// Bind-pose positions in ELU world space.
    rest: Vec<Vec3>,
    to_local: Mat4,
    /// 0 at the top edge, 1 at the hem.
    hang: Vec<f32>,
    /// Metres along the wind direction: the wave travels downwind.
    travel: Vec<f32>,
    /// Unit horizontal world axis the vertices move along (the cloth's thinnest one).
    normal: Vec3,
    /// Displacement at the hem, centimetres.
    amp: f32,
    period: f32,
    phase: f32,
    culling_off: bool,
}

impl Cloth {
    /// `None` for a node without vertices or without height.
    fn new(elu: &Arc<Elu>, node: usize, wind: Wind) -> Option<Self> {
        let n = &elu.nodes[node];
        let world = Mat4::from_cols_array(&n.world);
        let rest: Vec<Vec3> = n
            .positions
            .iter()
            .map(|&p| world.transform_point3(p.into()))
            .collect();
        // ELU (x, y, z) = (-map x, map z, map y): a map heading `a` is ELU (-cos a, 0, sin a).
        let a = wind.direction.to_radians();
        let dir = Vec3::new(-a.cos(), 0.0, a.sin());
        let extent = |axis: usize| {
            let v = rest.iter().map(|p| p[axis]);
            let (lo, hi) = v.fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
            (lo, hi - lo)
        };
        let ((_, ext_x), (y_lo, ext_y), (_, ext_z)) = (extent(0), extent(1), extent(2));
        if rest.is_empty() || ext_y < 1.0 {
            return None;
        }
        let top = y_lo + ext_y;
        Some(Self {
            elu: elu.clone(),
            node,
            to_local: world.inverse(),
            hang: rest.iter().map(|p| (top - p.y) / ext_y).collect(),
            travel: rest.iter().map(|p| 0.01 * p.dot(dir)).collect(),
            rest,
            normal: if ext_x <= ext_z { Vec3::X } else { Vec3::Z },
            amp: 0.015 * wind.power * ext_y,
            period: (wind.delay / 1000.0).max(0.5),
            // Neighbouring cloths must not wave in lockstep.
            phase: (world.w_axis.x * 0.013 + world.w_axis.z * 0.007).rem_euclid(TAU),
            culling_off: false,
        })
    }
}

/// Props whose file name contains one of these are the sky (domes/boxes with a sliding cloud
/// layer): `obj_sky_*`, `obj_ef_sky`, `obj_ef_daylight`, `sky_ani_cloud`.
fn is_sky(file: &str) -> bool {
    file.contains("_sky") || file.contains("_daylight")
}

/// Startup system: spawns every `OBJECTLIST` prop. Props have no transform in the XML (their
/// vertices are in map space), so each ELU is placed by the fixed Y-up -> map mapping below.
/// The sky is spawned like any other prop, in place: every retail sky object is authored at
/// the world origin and horizontally encloses its map (14 of 14 maps with a sky object, see
/// `docs/formats.md`), so no camera tracking is needed (retail may still do it: not in the data).
pub fn spawn_props(mut commands: Commands, level: Res<Level>, mut fx: FxAssets) {
    let (vfs, map) = (&level.vfs, &level.map);
    let mut loader = Loader::new(vfs, "maps/");
    let flags = read_flags(vfs, &map.dir);
    // Props are authored Y-up with map (x, y, z) = (-x, z, y) (checked against the Mansion
    // fire lights); the model loader mirrors z, so a half turn about Y finishes the mapping.
    let placement = Transform::from_rotation(Quat::from_rotation_y(PI));
    let (mut sky, mut cloth, mut animated) = (0, 0, 0);
    for name in &map.objects {
        let Some(path) = view::texture_path(vfs, &HashMap::new(), &map.dir, name) else {
            warn!("object {name} not found");
            continue;
        };
        let elu = match vfs.read(&path).and_then(|b| elu::load(&b)) {
            Ok(e) => Arc::new(e),
            Err(e) => {
                warn!("{path}: {e}");
                continue;
            }
        };
        let ani = vfs
            .read(&format!("{path}.ani"))
            .ok()
            .and_then(|b| ani::load(&b).map_err(|e| warn!("{path}.ani: {e}")).ok());
        animated += ani.is_some() as usize;
        let dir = path.rsplit_once('/').map_or("", |(d, _)| d).to_string() + "/";
        let file = path.rsplit('/').next().unwrap();
        sky += is_sky(file) as usize;
        let model = loader.spawn(
            &mut fx,
            &mut commands,
            &dir,
            &elu,
            ani.map(|a| (Arc::new(a), Loop::Wrap)),
            placement,
        );
        commands.entity(model.root).insert(crate::game::MapEntity);
        let wind = flags.iter().find(|(n, _)| file.ends_with(n.as_str()));
        if let Some((_, &wind)) = wind {
            for (i, n) in elu.nodes.iter().enumerate() {
                if let (Some(c), Some(&e)) = (Cloth::new(&elu, i, wind), model.nodes.get(&n.name)) {
                    commands.entity(e).insert(c);
                    cloth += 1;
                }
            }
        }
    }
    println!(
        "{} props: {sky} sky, {cloth} cloth, {animated} animated",
        map.objects.len()
    );
}

/// Rebuilds the meshes of every [`Cloth`] for the current time.
pub fn flutter(
    mut commands: Commands,
    clock: Res<Clock>,
    time: Res<Time>,
    mut cloths: Query<(&mut Cloth, &Children)>,
    handles: Query<&Mesh3d>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let t = clock.now(&time);
    for (mut c, children) in &mut cloths {
        let mesh_children = || children.iter().filter(|&c| handles.contains(c));
        if !c.culling_off {
            // The bounding boxes are those of the rest pose.
            for child in mesh_children() {
                commands.entity(child).insert(NoFrustumCulling);
            }
            c.culling_off = true;
        }
        let phase = TAU * t / c.period + c.phase;
        let mut node = c.elu.nodes[c.node].clone();
        let vertices = node.positions.iter_mut().zip(&c.rest);
        for (i, (out, p)) in vertices.enumerate() {
            let (s, u) = (c.hang[i], c.travel[i]);
            let wave = (phase - 3.0 * s - u).sin() + 0.5 * (1.7 * phase - 5.0 * s - 1.7 * u).sin();
            *out = c
                .to_local
                .transform_point3(*p + c.normal * (c.amp * s * wave / 1.5))
                .to_array();
        }
        for (child, batch) in mesh_children().zip(node_batches(&c.elu, &node, None)) {
            if let Some(mut m) = meshes.get_mut(&handles.get(child).unwrap().0) {
                *m = batch.mesh;
            }
        }
    }
}
