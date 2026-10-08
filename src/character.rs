//! Player character definitions (`model/character.xml` -> `model/<sex>/<sex>01.xml`) and
//! outfit assembly. Layouts and evidence: `docs/formats.md`.

use crate::{
    elu,
    model::{self, Model, Textures},
    mrs::Vfs,
};
use bevy::{mesh::skinning::SkinnedMeshInverseBindposes, prelude::*};
use std::io::{self, ErrorKind};

/// `AddAnimation` entry. `file` is a VFS path next to the character XML.
#[derive(Clone, Debug)]
pub struct Animation {
    pub name: String,
    pub file: String,
    pub sound: String,
    /// Weapon motion type (see `model/weapon.xml`): 1 melee, 2 one-handed pistol, ...
    pub motion_type: u32,
    /// `loop`, `lastframe`, `onceidle`, `onceLowerbody`, ... (verbatim; retail has a few typos).
    pub loop_type: String,
    pub gm: bool,
}

#[derive(Clone, Debug)]
pub struct Character {
    /// Registry name from `model/character.xml`, e.g. `heroman1`.
    pub name: String,
    /// VFS directory of the XML and its models, ending in `/`.
    pub dir: String,
    /// VFS path of the base model (skeleton, default body, weapon attach nodes).
    pub base: String,
    /// VFS paths of every `AddParts` set (each holds a full skeleton copy plus its slot meshes).
    pub parts: Vec<String>,
    pub animations: Vec<Animation>,
}

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, msg.into())
}

/// `(name, xml path)` of every `AddXml` in `model/character.xml`.
pub fn registry(vfs: &Vfs) -> io::Result<Vec<(String, String)>> {
    let text = read_text(vfs, "model/character.xml")?;
    let doc = roxmltree::Document::parse(&text).map_err(|e| bad(format!("character.xml: {e}")))?;
    Ok(doc
        .descendants()
        .filter(|n| n.has_tag_name("AddXml"))
        .filter_map(|n| {
            Some((
                n.attribute("name")?.to_string(),
                n.attribute("filename")?.to_string(),
            ))
        })
        .collect())
}

fn read_text(vfs: &Vfs, path: &str) -> io::Result<String> {
    let bytes = vfs.read(path)?;
    Ok(String::from_utf8_lossy(&bytes)
        .trim_start_matches('\u{feff}')
        .to_string())
}

/// Loads a character XML by registry name (`heroman1`, `herowoman1`).
pub fn load(vfs: &Vfs, name: &str) -> io::Result<Character> {
    let xml = registry(vfs)?
        .into_iter()
        .find(|(n, _)| n == name)
        .ok_or_else(|| bad(format!("character {name:?} not in model/character.xml")))?
        .1;
    let xml = crate::mrs::normalize(&xml);
    let dir = xml
        .rsplit_once('/')
        .map_or(String::new(), |(d, _)| format!("{d}/"));
    let text = read_text(vfs, &xml)?;
    let doc = roxmltree::Document::parse(&text).map_err(|e| bad(format!("{xml}: {e}")))?;
    let attr = |n: roxmltree::Node, a: &str| {
        n.attribute(a)
            .map(str::to_string)
            .ok_or_else(|| bad(format!("{xml}: <{}> without {a}", n.tag_name().name())))
    };
    let (mut base, mut parts, mut animations) = (None, Vec::new(), Vec::new());
    for n in doc.root_element().children().filter(|n| n.is_element()) {
        match n.tag_name().name() {
            "AddBaseModel" => {
                base = Some(format!(
                    "{dir}{}",
                    crate::mrs::normalize(&attr(n, "filename")?)
                ))
            }
            "AddParts" => parts.push(format!(
                "{dir}{}",
                crate::mrs::normalize(&attr(n, "filename")?)
            )),
            "AddAnimation" => animations.push(Animation {
                name: attr(n, "name")?,
                file: format!("{dir}{}", crate::mrs::normalize(&attr(n, "filename")?)),
                sound: n.attribute("sound").unwrap_or_default().to_string(),
                motion_type: attr(n, "motion_type")?
                    .parse()
                    .map_err(|_| bad(format!("{xml}: bad motion_type")))?,
                loop_type: attr(n, "motion_loop_type")?,
                gm: n.attribute("gm") == Some("1"),
            }),
            _ => {}
        }
    }
    for p in parts
        .iter()
        .chain(base.iter())
        .chain(animations.iter().map(|a| &a.file))
    {
        if !vfs.exists(p) {
            return Err(bad(format!("{xml}: {p} not in archives")));
        }
    }
    Ok(Character {
        name: name.to_string(),
        dir,
        base: base.ok_or_else(|| bad(format!("{xml}: no AddBaseModel")))?,
        parts,
        animations,
    })
}

/// Body slots, named by the node prefix `eq_<slot>_NNN` of skinned part meshes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slot {
    Head,
    Face,
    Chest,
    Hands,
    Legs,
    Feet,
}

impl Slot {
    pub const ALL: [Slot; 6] = [
        Slot::Head,
        Slot::Face,
        Slot::Chest,
        Slot::Hands,
        Slot::Legs,
        Slot::Feet,
    ];

    /// Slot of an `eq_<slot>_NNN` skinned node name.
    pub fn of_node(name: &str) -> Option<Slot> {
        match name.strip_prefix("eq_")?.split('_').next()? {
            "head" => Some(Slot::Head),
            "face" => Some(Slot::Face),
            "chest" => Some(Slot::Chest),
            "hands" => Some(Slot::Hands),
            "legs" => Some(Slot::Legs),
            "feet" => Some(Slot::Feet),
            _ => None,
        }
    }
}

/// Weapon attach nodes of the base model (`eq_wd_*`, `eq_wl_*`, `eq_wr_*`) are not part of
/// the body; they carry the held-weapon frames and stay hidden.
pub fn is_weapon_node(name: &str) -> bool {
    name.starts_with("eq_wd_") || name.starts_with("eq_wl_") || name.starts_with("eq_wr_")
}

/// Which part set (index into [`Character::parts`]) dresses each slot; `None` keeps the base
/// model's default piece. One mesh per slot is spawned, so nothing overlaps.
#[derive(Clone, Debug)]
pub struct Outfit {
    pub slots: [(Slot, Option<usize>); 6],
}

impl Outfit {
    pub fn base() -> Self {
        Self {
            slots: Slot::ALL.map(|s| (s, None)),
        }
    }

    /// Every slot that part set `part` provides is taken from it, the rest from the base.
    pub fn from_part(vfs: &Vfs, ch: &Character, part: usize) -> io::Result<Self> {
        let elu = elu::load(&vfs.read(&ch.parts[part])?)?;
        let mut o = Self::base();
        for n in &elu.nodes {
            if let (Some(slot), false) = (Slot::of_node(&n.name), n.skin.is_empty()) {
                o.slots.iter_mut().find(|(s, _)| *s == slot).unwrap().1 = Some(part);
            }
        }
        Ok(o)
    }

    fn part_for(&self, slot: Slot) -> Option<usize> {
        self.slots
            .iter()
            .find(|(s, _)| *s == slot)
            .and_then(|(_, p)| *p)
    }
}

/// Spawns the character in bind pose: skeleton and weapon attach nodes from the base model,
/// the base model's default body pieces for slots without a part, and the chosen part set's
/// meshes for the others. The `eq_w*` attach nodes exist as entities but get no meshes.
pub fn spawn(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    inverse_bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
    images: &mut Assets<Image>,
    standard: &mut Assets<StandardMaterial>,
    textures: &mut Textures,
    vfs: &Vfs,
    ch: &Character,
    outfit: &Outfit,
    placement: Transform,
) -> io::Result<Model> {
    let base = elu::load(&vfs.read(&ch.base)?)?;
    let mut material = |m: &elu::Material| textures.standard(images, standard, &ch.dir, m);
    let keep_base = |n: &elu::Node| match Slot::of_node(&n.name) {
        Some(slot) => outfit.part_for(slot).is_none(),
        None => !is_weapon_node(&n.name),
    };
    let skeleton = model::spawn_elu(
        commands,
        meshes,
        inverse_bindposes,
        &base,
        &mut material,
        placement,
        keep_base,
    );
    let mut used = Vec::new();
    for (_, part) in outfit.slots {
        let Some(part) = part else { continue };
        if used.contains(&part) {
            continue;
        }
        used.push(part);
        let elu = elu::load(&vfs.read(&ch.parts[part])?)?;
        model::spawn_part(
            commands,
            meshes,
            inverse_bindposes,
            &elu,
            &mut material,
            &skeleton,
            |n| Slot::of_node(&n.name).is_some_and(|s| outfit.part_for(s) == Some(part)),
        );
    }
    Ok(skeleton)
}
