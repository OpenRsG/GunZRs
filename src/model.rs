//! Bevy spawning of ELU models: named node entities, static or skinned meshes, textures.
//!
//! Conversion from ELU space (left-handed, Y up, cm) to Bevy (right-handed, Y up): `z` is
//! negated for points, normals and matrices (`M' = S M S`, `S = diag(1,1,-1)`), triangle
//! winding is reversed, and the model root carries [`SCALE`] (cm -> m). All node entities are
//! therefore in centimetres below the root, so a child ELU attached below a node entity must
//! be spawned with [`Model::attach`] (it resets the child's own scale).

use crate::{
    elu::{self, Elu, Node},
    mrs::Vfs,
    view::{self, SCALE},
};
use bevy::{
    asset::RenderAssetUsages,
    camera::visibility::NoFrustumCulling,
    image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor},
    mesh::{
        PrimitiveTopology, VertexAttributeValues,
        skinning::{SkinnedMesh, SkinnedMeshInverseBindposes},
    },
    prelude::*,
};
use std::collections::{BTreeMap, HashMap};

/// A spawned ELU: root entity plus every node entity by node name.
pub struct Model {
    pub root: Entity,
    pub nodes: HashMap<String, Entity>,
    /// Mirrored bind (world) matrix of every node, in ELU centimetres, relative to `root`.
    pub bind: HashMap<String, Mat4>,
}

impl Model {
    /// Re-parents the model below `parent` (a node entity of another model) with identity
    /// transform: the ELU's own frame coincides with `parent`'s frame, scale is inherited.
    pub fn attach(&self, commands: &mut Commands, parent: Entity) {
        commands
            .entity(self.root)
            .insert((Transform::IDENTITY, ChildOf(parent)));
    }
}

const MIRROR: Mat4 = Mat4::from_diagonal(Vec4::new(1.0, 1.0, -1.0, 1.0));

/// ELU row-vector matrix (`p' = p * M`) as the equivalent mirrored Bevy column matrix.
pub fn bevy_matrix(m: &[f32; 16]) -> Mat4 {
    MIRROR * Mat4::from_cols_array(m) * MIRROR
}

/// Texture lookup and decoding with a per-name image cache.
pub struct Textures<'a> {
    vfs: &'a Vfs,
    by_name: HashMap<String, String>,
    cache: HashMap<String, Option<Handle<Image>>>,
    repeat: ImageSampler,
}

impl<'a> Textures<'a> {
    /// `prefer`: VFS prefix whose files win when a texture name exists in several archives.
    pub fn new(vfs: &'a Vfs, prefer: &str) -> Self {
        Self {
            vfs,
            by_name: view::file_index(vfs, prefer),
            cache: HashMap::new(),
            repeat: ImageSampler::Descriptor(ImageSamplerDescriptor {
                address_mode_u: ImageAddressMode::Repeat,
                address_mode_v: ImageAddressMode::Repeat,
                ..ImageSamplerDescriptor::linear()
            }),
        }
    }

    /// sRGB image for texture `name` resolved against VFS directory `dir` (ending in `/`).
    pub fn load(
        &mut self,
        images: &mut Assets<Image>,
        dir: &str,
        name: &str,
    ) -> Option<Handle<Image>> {
        let path = view::texture_path(self.vfs, &self.by_name, dir, name)?;
        self.cache
            .entry(path.clone())
            .or_insert_with(|| {
                let ext = path.rsplit('.').next().unwrap();
                let bytes = self.vfs.read(&path).ok()?;
                view::decode(&bytes, ext, true, self.repeat.clone()).map(|i| images.add(i))
            })
            .clone()
    }

    /// Unlit `StandardMaterial` for an ELU material: texture, two-sided, additive, alpha test.
    pub fn standard(
        &mut self,
        images: &mut Assets<Image>,
        materials: &mut Assets<StandardMaterial>,
        dir: &str,
        m: &elu::Material,
    ) -> Handle<StandardMaterial> {
        let texture = if m.texture.is_empty() {
            None
        } else {
            let t = self.load(images, dir, &m.texture);
            if t.is_none() {
                warn!("{dir}: texture {:?} not found or undecodable", m.texture);
            }
            t
        };
        materials.add(StandardMaterial {
            base_color_texture: texture,
            unlit: true,
            cull_mode: if m.two_sided {
                None
            } else {
                Some(bevy::render::render_resource::Face::Back)
            },
            alpha_mode: if m.additive {
                AlphaMode::Add
            } else if m.alpha_ref > 0 {
                AlphaMode::Mask(m.alpha_ref as f32 / 255.0)
            } else {
                AlphaMode::Opaque
            },
            ..default()
        })
    }
}

/// Nodes whose name starts with `Bip01` are skeleton helpers; their stored mesh is a small
/// octahedron that the game never draws.
pub fn is_bone(name: &str) -> bool {
    name.starts_with("Bip01")
}

/// One draw batch of a node: all faces sharing an ELU material (index into `Elu::materials`).
pub struct Batch {
    pub material: Option<usize>,
    pub mesh: Mesh,
}

#[derive(Default)]
struct Verts {
    pos: Vec<[f32; 3]>,
    normal: Vec<[f32; 3]>,
    uv: Vec<[f32; 2]>,
    joints: Vec<[u16; 4]>,
    weights: Vec<[f32; 4]>,
}

/// Triangle-list meshes for `node`, one per material, already converted to Bevy space (still
/// in centimetres). With `bones` (joint order) the vertices are skinned: positions are taken
/// in model space (`pos * world`) and carry joint indices/weights; the influences must only
/// reference names in `bones`.
pub fn node_batches(elu: &Elu, node: &Node, bones: Option<&HashMap<&str, u16>>) -> Vec<Batch> {
    let world = Mat4::from_cols_array(&node.world);
    let flip = |v: Vec3| [v.x, v.y, -v.z];
    let mut groups: BTreeMap<Option<usize>, Verts> = BTreeMap::new();
    for f in &node.faces {
        let b = groups
            .entry(elu.material_index(node.material, f.sub_material))
            .or_default();
        // Mirroring reverses orientation: emit corners 0,2,1.
        for c in [0, 2, 1] {
            let i = f.pos[c] as usize;
            let p = Vec3::from(node.positions[i]);
            let n = Vec3::from(f.normals[c]);
            if let Some(bones) = bones {
                b.pos.push(flip(world.transform_point3(p)));
                b.normal
                    .push(flip(world.transform_vector3(n).normalize_or_zero()));
                let (mut j, mut w) = ([0u16; 4], [0.0f32; 4]);
                for (k, inf) in node.skin[i].iter().enumerate() {
                    j[k] = bones[inf.bone.as_str()];
                    w[k] = inf.weight;
                }
                b.joints.push(j);
                b.weights.push(w);
            } else {
                b.pos.push(flip(p));
                b.normal.push(flip(n));
            }
            b.uv.push(f.uv[c]);
        }
    }
    groups
        .into_iter()
        .map(|(material, v)| {
            let mut mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::RENDER_WORLD,
            )
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, v.pos)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, v.normal)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, v.uv);
            if bones.is_some() {
                mesh.insert_attribute(
                    Mesh::ATTRIBUTE_JOINT_INDEX,
                    VertexAttributeValues::Uint16x4(v.joints),
                );
                mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, v.weights);
            }
            Batch { material, mesh }
        })
        .collect()
}

/// Lazily creates and caches one Bevy material per used ELU material index.
struct Materials<'a> {
    make: &'a mut dyn FnMut(&elu::Material) -> Handle<StandardMaterial>,
    made: HashMap<usize, Handle<StandardMaterial>>,
}

impl<'a> Materials<'a> {
    fn new(make: &'a mut dyn FnMut(&elu::Material) -> Handle<StandardMaterial>) -> Self {
        Self {
            make,
            made: HashMap::new(),
        }
    }

    fn get(&mut self, elu: &Elu, i: usize) -> Handle<StandardMaterial> {
        let make = &mut self.make;
        self.made
            .entry(i)
            .or_insert_with(|| make(&elu.materials[i]))
            .clone()
    }
}

/// Spawns `elu` as a hierarchy: a root (scaled by [`SCALE`], placed by `placement`) with one
/// `Name`d entity per node whose `Transform` is local to its parent node, rigid meshes as
/// children of their node and skinned meshes (nodes with skin data) as children of the root,
/// bound to the node entities. `material` creates the Bevy material of an ELU material the
/// first time a batch uses it (never for unused materials);
/// `keep_mesh(node)` can suppress a node's mesh (the node entity is always spawned).
/// Nodes named `Bip01*` never get meshes ([`is_bone`]).
pub fn spawn_elu(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    inverse_bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
    elu: &Elu,
    material: &mut dyn FnMut(&elu::Material) -> Handle<StandardMaterial>,
    placement: Transform,
    keep_mesh: impl Fn(&Node) -> bool,
) -> Model {
    let root = commands
        .spawn((
            Name::new("elu"),
            placement.with_scale(placement.scale * SCALE),
            Visibility::default(),
        ))
        .id();
    let bind: HashMap<String, Mat4> = elu
        .nodes
        .iter()
        .map(|n| (n.name.clone(), bevy_matrix(&n.world)))
        .collect();
    let mut nodes = HashMap::new();
    for n in &elu.nodes {
        let local = match bind.get(&n.parent) {
            Some(p) => p.inverse() * bind[&n.name],
            None => bind[&n.name],
        };
        let e = commands
            .spawn((
                Name::new(n.name.clone()),
                Transform::from_matrix(local),
                Visibility::default(),
            ))
            .id();
        nodes.insert(n.name.clone(), e);
    }
    for n in &elu.nodes {
        let parent = nodes.get(&n.parent).copied().unwrap_or(root);
        commands.entity(parent).add_child(nodes[&n.name]);
    }
    let model = Model { root, nodes, bind };
    let mut mats = Materials::new(material);
    for n in elu
        .nodes
        .iter()
        .filter(|n| !is_bone(&n.name) && keep_mesh(n))
    {
        spawn_node_meshes(
            commands,
            meshes,
            inverse_bindposes,
            elu,
            n,
            &mut mats,
            &model,
        );
    }
    model
}

/// Spawns the skinned meshes of `part` (nodes with skin data accepted by `keep`) below
/// `skeleton.root`, bound to the skeleton's node entities by bone name. Parts only reference
/// bones, never carry their own pose. Returns the mesh-node entities spawned; nodes that
/// reference a bone missing from the skeleton are skipped with a warning.
pub fn spawn_part(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    inverse_bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
    part: &Elu,
    material: &mut dyn FnMut(&elu::Material) -> Handle<StandardMaterial>,
    skeleton: &Model,
    keep: impl Fn(&Node) -> bool,
) -> Vec<Entity> {
    let mut mats = Materials::new(material);
    part.nodes
        .iter()
        .filter(|n| !n.skin.is_empty() && keep(n))
        .filter_map(|n| {
            spawn_node_meshes(
                commands,
                meshes,
                inverse_bindposes,
                part,
                n,
                &mut mats,
                skeleton,
            )
        })
        .collect()
}

/// Returns the entity holding the node's meshes (a container below the skeleton root for
/// skinned nodes, the node's own entity for rigid ones).
fn spawn_node_meshes(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    inverse_bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
    elu: &Elu,
    node: &Node,
    mats: &mut Materials,
    model: &Model,
) -> Option<Entity> {
    let (container, skin) = if node.skin.is_empty() {
        (model.nodes.get(&node.name).copied()?, None)
    } else {
        let mut names: Vec<&str> = Vec::new();
        for inf in node.skin.iter().flatten() {
            if !names.contains(&inf.bone.as_str()) {
                names.push(&inf.bone);
            }
        }
        let missing = names.iter().find(|b| !model.nodes.contains_key(**b));
        if let Some(b) = missing {
            warn!(
                "{}: bone {b:?} missing in skeleton, mesh skipped",
                node.name
            );
            return None;
        }
        let joints = names.iter().map(|b| model.nodes[*b]).collect();
        let inverse = names
            .iter()
            .map(|b| model.bind[*b].inverse())
            .collect::<Vec<_>>();
        let skin = SkinnedMesh {
            inverse_bindposes: inverse_bindposes.add(SkinnedMeshInverseBindposes::from(inverse)),
            joints,
        };
        let index: HashMap<&str, u16> = names
            .iter()
            .enumerate()
            .map(|(i, n)| (*n, i as u16))
            .collect();
        let container = commands
            .spawn((
                Name::new(node.name.clone()),
                Transform::IDENTITY,
                Visibility::default(),
                ChildOf(model.root),
            ))
            .id();
        (container, Some((skin, index)))
    };
    for batch in node_batches(elu, node, skin.as_ref().map(|(_, i)| i)) {
        let material = batch.material.map(|i| mats.get(elu, i)).unwrap_or_default();
        let mut mesh = commands.spawn((
            Mesh3d(meshes.add(batch.mesh)),
            MeshMaterial3d(material),
            Transform::IDENTITY,
            ChildOf(container),
        ));
        if let Some((s, _)) = &skin {
            mesh.insert((s.clone(), NoFrustumCulling));
        }
    }
    Some(container)
}
