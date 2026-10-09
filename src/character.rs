//! Player character definitions (`model/character.xml` -> `model/<sex>/<sex>01.xml`) and
//! outfit assembly. Layouts and evidence: `docs/formats.md`.

use crate::{
    elu,
    item::Items,
    model::{self, Model, Textures},
    mrs::Vfs,
};
use bevy::{mesh::skinning::SkinnedMeshInverseBindposes, prelude::*};
use std::{
    fmt,
    io::{self, ErrorKind},
    str::FromStr,
};

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

    /// Shown name (the head slot holds hair and hats).
    pub fn label(self) -> &'static str {
        ["Hair/hat", "Face", "Chest", "Hands", "Legs", "Feet"][self as usize]
    }
}

/// Weapon attach nodes of the base model (`eq_wd_*`, `eq_wl_*`, `eq_wr_*`) are not part of
/// the body; they carry the held-weapon frames and stay hidden.
pub fn is_weapon_node(name: &str) -> bool {
    name.starts_with("eq_wd_") || name.starts_with("eq_wl_") || name.starts_with("eq_wr_")
}

/// Dyes a slot can wear ([`Look::tints`]): they multiply the texture colour, so they darken or
/// colour a piece but never lighten it. Index 0 leaves the textures as they are. **Not
/// retail**: GunZ has no dyes.
pub const TINTS: [(&str, [f32; 3]); 20] = [
    ("None", [1.0, 1.0, 1.0]),
    ("Ash", [0.75, 0.75, 0.75]),
    ("Slate", [0.55, 0.6, 0.68]),
    ("Charcoal", [0.3, 0.3, 0.32]),
    ("Crimson", [1.0, 0.25, 0.3]),
    ("Red", [1.0, 0.42, 0.35]),
    ("Orange", [1.0, 0.6, 0.3]),
    ("Gold", [1.0, 0.82, 0.4]),
    ("Lime", [0.7, 1.0, 0.4]),
    ("Green", [0.4, 0.85, 0.45]),
    ("Olive", [0.65, 0.7, 0.4]),
    ("Teal", [0.35, 0.85, 0.8]),
    ("Sky", [0.5, 0.8, 1.0]),
    ("Blue", [0.4, 0.55, 1.0]),
    ("Navy", [0.3, 0.35, 0.7]),
    ("Violet", [0.65, 0.45, 1.0]),
    ("Magenta", [1.0, 0.45, 0.9]),
    ("Pink", [1.0, 0.7, 0.8]),
    ("Brown", [0.65, 0.45, 0.3]),
    ("Tan", [0.9, 0.78, 0.6]),
];

/// What a character wears, per slot in [`Slot::ALL`] order: the part set dressing it (index
/// into [`Character::parts`]; `None` keeps the base model's piece) and a [`TINTS`] index.
/// Written as text (`--look`, the profile): the six parts 1-based (0 = base), `;`, the six
/// tints, e.g. `3,0,12,0,5,0;0,0,4,0,0,0`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Look {
    pub parts: [Option<u16>; 6],
    pub tints: [u8; 6],
}

impl Look {
    /// Every slot that part set `part` provides is taken from it, the rest from the base.
    pub fn set(vfs: &Vfs, ch: &Character, part: usize) -> io::Result<Self> {
        let elu = elu::load(&vfs.read(&ch.parts[part])?)?;
        let mut look = Self::default();
        for n in elu.nodes.iter().filter(|n| !n.skin.is_empty()) {
            if let Some(slot) = Slot::of_node(&n.name) {
                look.parts[slot as usize] = Some(part as u16);
            }
        }
        Ok(look)
    }

    /// Drops parts that `ch` does not have and unknown tints (a look saved for the other sex
    /// or another install).
    pub fn fit(mut self, ch: &Character) -> Self {
        for p in &mut self.parts {
            *p = p.filter(|&p| (p as usize) < ch.parts.len());
        }
        for t in &mut self.tints {
            *t = if (*t as usize) < TINTS.len() { *t } else { 0 };
        }
        self
    }
}

impl fmt::Display for Look {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let parts: Vec<_> = self
            .parts
            .iter()
            .map(|p| p.map_or(0, |p| p as u32 + 1).to_string())
            .collect();
        let tints: Vec<_> = self.tints.iter().map(u8::to_string).collect();
        write!(f, "{};{}", parts.join(","), tints.join(","))
    }
}

impl FromStr for Look {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let bad = || format!("bad look {s:?} (six parts ; six tints)");
        let (parts, tints) = s.split_once(';').ok_or_else(bad)?;
        let nums = |list: &str| -> Result<Vec<u32>, String> {
            let v: Vec<u32> = list
                .split(',')
                .map(|n| n.trim().parse().map_err(|_| bad()))
                .collect::<Result<_, _>>()?;
            (v.len() == 6).then_some(v).ok_or_else(bad)
        };
        let (parts, tints) = (nums(parts)?, nums(tints)?);
        Ok(Self {
            parts: std::array::from_fn(|i| parts[i].checked_sub(1).map(|p| p as u16)),
            tints: std::array::from_fn(|i| tints[i].min(255) as u8),
        })
    }
}

/// One piece a slot can wear: a part set and the name of the item that is that mesh.
#[derive(Clone, Debug)]
pub struct Piece {
    pub part: u16,
    pub name: String,
}

/// The pieces each slot ([`Slot::ALL`] order) can wear: the `equip` items of
/// `system/zitem.xml` for the character's sex whose `mesh_name` `eq_<slot>_NNN` is the slot
/// node of part set `<sex>-set-NNN.elu`. **Observed** over both sexes: every slot node of a
/// part set carries the set's number, and items name 540 of the 543 slot meshes.
#[derive(Clone, Debug, Default)]
pub struct Wardrobe {
    pub slots: [Vec<Piece>; 6],
}

impl Wardrobe {
    pub fn new(ch: &Character, items: &Items, woman: bool) -> Self {
        let sex = if woman { 'f' } else { 'm' };
        let number = |path: &str| {
            path.rsplit('-')
                .next()?
                .strip_suffix(".elu")
                .map(str::to_owned)
        };
        let by_number: std::collections::HashMap<String, usize> = ch
            .parts
            .iter()
            .enumerate()
            .filter_map(|(i, p)| Some((number(p)?, i)))
            .collect();
        let mut w = Self::default();
        for item in items.items.values() {
            let Some(mesh) = &item.mesh_name else {
                continue;
            };
            if item.kind != "equip" || !(item.sex == sex || item.sex == 'a') {
                continue;
            }
            let mesh = mesh.to_ascii_lowercase();
            let (Some(slot), Some(num)) = (Slot::of_node(&mesh), mesh.rsplit('_').next()) else {
                continue;
            };
            let Some(&part) = by_number.get(num) else {
                continue;
            };
            let list = &mut w.slots[slot as usize];
            if !list.iter().any(|p| p.part as usize == part) {
                list.push(Piece {
                    part: part as u16,
                    name: item.name.clone().unwrap_or_else(|| mesh.clone()),
                });
            }
        }
        w
    }

    /// A random outfit from `seed`: most slots get a piece, a few keep the base one, and about
    /// one piece in four is dyed.
    pub fn random(&self, seed: u64) -> Look {
        // splitmix64
        let mut s = seed;
        let mut next = move || {
            s = s.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = s;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        };
        let mut look = Look::default();
        for (i, pieces) in self.slots.iter().enumerate() {
            if !pieces.is_empty() && next() % 5 != 0 {
                look.parts[i] = Some(pieces[(next() % pieces.len() as u64) as usize].part);
                if next() % 4 == 0 {
                    look.tints[i] = 1 + (next() % (TINTS.len() as u64 - 1)) as u8;
                }
            }
        }
        look
    }
}

/// Spawns the character in bind pose: skeleton and weapon attach nodes from the base model,
/// then per slot the chosen part set's mesh (the base model's own piece without one), dyed
/// with the slot's tint. The `eq_w*` attach nodes exist as entities but get no meshes. Every
/// file is read before anything spawns, so an error leaves nothing behind.
pub fn spawn(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    inverse_bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
    images: &mut Assets<Image>,
    standard: &mut Assets<StandardMaterial>,
    textures: &mut Textures,
    vfs: &Vfs,
    ch: &Character,
    look: &Look,
    placement: Transform,
) -> io::Result<Model> {
    let base = elu::load(&vfs.read(&ch.base)?)?;
    let mut loaded: Vec<(u16, elu::Elu)> = Vec::new();
    for p in look.parts.iter().flatten() {
        if (*p as usize) < ch.parts.len() && !loaded.iter().any(|(q, _)| q == p) {
            loaded.push((*p, elu::load(&vfs.read(&ch.parts[*p as usize])?)?));
        }
    }
    let skeleton = model::spawn_elu(
        commands,
        meshes,
        inverse_bindposes,
        &base,
        &mut |m| textures.standard(images, standard, &ch.dir, m),
        placement,
        |n| Slot::of_node(&n.name).is_none() && !is_weapon_node(&n.name),
    );
    for (i, slot) in Slot::ALL.into_iter().enumerate() {
        // a part set without this slot (an older profile's whole-set outfit) keeps the base piece
        let has = |e: &elu::Elu| {
            e.nodes
                .iter()
                .any(|n| !n.skin.is_empty() && Slot::of_node(&n.name) == Some(slot))
        };
        let elu = look.parts[i]
            .and_then(|p| loaded.iter().find(|(q, _)| *q == p))
            .map(|(_, e)| e)
            .filter(|e| has(e))
            .unwrap_or(&base);
        let tint = TINTS.get(look.tints[i] as usize).map_or([1.0; 3], |t| t.1);
        let mut material = |m: &elu::Material| {
            let h = textures.standard(images, standard, &ch.dir, m);
            if tint == [1.0; 3] {
                return h;
            }
            let mut dyed = standard.get(&h).cloned().unwrap_or_default();
            let c = dyed.base_color.to_srgba();
            dyed.base_color = Color::srgba(
                c.red * tint[0],
                c.green * tint[1],
                c.blue * tint[2],
                c.alpha,
            );
            standard.add(dyed)
        };
        model::spawn_part(
            commands,
            meshes,
            inverse_bindposes,
            elu,
            &mut material,
            &skeleton,
            |n| Slot::of_node(&n.name) == Some(slot),
        );
    }
    Ok(skeleton)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The look is saved as text in the profile and passed as `--look`.
    #[test]
    fn look_text_round_trip() {
        let look = Look {
            parts: [Some(2), None, Some(11), None, Some(4), Some(0)],
            tints: [0, 3, 0, 0, 19, 1],
        };
        assert_eq!(look.to_string(), "3,0,12,0,5,1;0,3,0,0,19,1");
        assert_eq!(look.to_string().parse::<Look>(), Ok(look));
        assert!("1,2,3;0,0,0,0,0,0".parse::<Look>().is_err());
        assert!("1,2,3,4,5,6".parse::<Look>().is_err());
    }
}
