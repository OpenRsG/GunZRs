//! Items (`system/zitem.xml`, names from `system/strings.xml`) and weapon models
//! (`model/weapon.xml`). Layouts: `docs/formats.md`.

use crate::mrs::Vfs;
use std::{
    collections::{BTreeMap, HashMap},
    io::{self, ErrorKind},
    str::FromStr,
};

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, msg.into())
}

/// The `weapon` attribute of zitem.xml; decides animation set (`motion_type`) and hand dummies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeaponKind {
    Katana,
    Dagger,
    DoubleKatana,
    Pistol,
    PistolX2,
    Revolver,
    RevolverX2,
    Smg,
    SmgX2,
    Shotgun,
    MachineGun,
    Rifle,
    Rocket,
    Frag,
    Flashbang,
    Smoke,
    Medikit,
    Potion,
    RepairKit,
}

impl WeaponKind {
    pub fn parse(s: &str) -> Option<Self> {
        use WeaponKind::*;
        Some(match s {
            "katana" => Katana,
            "dagger" => Dagger,
            "doublekatana" => DoubleKatana,
            "pistol" => Pistol,
            "pistolx2" => PistolX2,
            "revolver" => Revolver,
            "revolverx2" => RevolverX2,
            "smg" => Smg,
            "smgx2" => SmgX2,
            "shotgun" => Shotgun,
            "machinegun" => MachineGun,
            "rifle" => Rifle,
            "rocket" => Rocket,
            "frag" => Frag,
            "flashbang" => Flashbang,
            "smoke" => Smoke,
            "medikit" => Medikit,
            "potion" => Potion,
            "repairkit" => RepairKit,
            _ => return None,
        })
    }

    /// Animation `motion_type` (the key into `<AddAnimation motion_type=..>` of character XMLs;
    /// legend in the comment of `model/weapon.xml`). Every retail mesh agrees with this.
    pub fn motion_type(self) -> u32 {
        use WeaponKind::*;
        match self {
            Katana => 1,
            Pistol | Revolver => 2,
            PistolX2 | RevolverX2 => 3,
            Shotgun | MachineGun => 4,
            Rifle => 5,
            Frag | Flashbang | Smoke => 6,
            Dagger => 7,
            Medikit | Potion | RepairKit => 8,
            Rocket => 9,
            Smg => 10,
            SmgX2 => 11,
            DoubleKatana => 13,
        }
    }

    /// Dummy nodes of the character base model (`eq_w[dlr]_*`) the weapon mesh is attached to:
    /// one entry for one-handed/two-handed grips, right then left for dual weapons.
    pub fn dummies(self) -> &'static [&'static str] {
        use WeaponKind::*;
        match self {
            Katana => &["eq_wd_katana"],
            Dagger => &["eq_wr_dagger"],
            DoubleKatana => &["eq_wr_blade", "eq_wl_blade"],
            Pistol | Revolver => &["eq_wr_pistol"],
            PistolX2 | RevolverX2 => &["eq_wr_pistol", "eq_wl_pistol"],
            Smg => &["eq_wr_smg"],
            SmgX2 => &["eq_wr_smg", "eq_wl_smg"],
            Shotgun | MachineGun => &["eq_wd_shotgun"],
            Rifle => &["eq_wd_rifle"],
            Rocket => &["eq_wd_rl"],
            Frag | Flashbang | Smoke => &["eq_wd_grenade"],
            Medikit | Potion | RepairKit => &["eq_wd_medikit"],
        }
    }

    /// Holding the trigger keeps firing. *Inferred* (the data has no flag): the rapid-fire guns.
    /// Pistols, revolvers, shotguns and launchers need one click per shot.
    pub fn automatic(self) -> bool {
        use WeaponKind::*;
        matches!(self, Smg | SmgX2 | MachineGun | Rifle)
    }

    /// Two guns, fired one hand at a time (`pistolx2`, `revolverx2`, `smgx2`).
    pub fn dual(self) -> bool {
        use WeaponKind::*;
        matches!(self, PistolX2 | RevolverX2 | SmgX2)
    }
}

/// Weapon stats of an item (attributes present on every `weapon=` item, plus optional ones).
#[derive(Debug, Clone)]
pub struct Weapon {
    pub kind: WeaponKind,
    pub damage: u32,
    /// Fire/swing delay in ms.
    pub delay: u32,
    pub magazine: u32,
    pub max_bullet: Option<u32>,
    /// Raw zitem `reloadtime` (3..10); unit unknown, not seconds (reload clips are 1.3-2 s).
    pub reload_time: Option<u32>,
    /// Melee reach.
    pub range: Option<u32>,
    /// Melee swing angle in degrees.
    pub angle: Option<u32>,
    /// Recoil/spread control.
    pub ctrl_ability: Option<u32>,
    pub slug_output: bool,
    pub effect_id: Option<u32>,
    /// Potions: points per second (`itempower`) for `damagetime` seconds, `damagetype` = heal | repair.
    pub item_power: Option<u32>,
    pub damage_time: Option<u32>,
    pub damage_type: Option<String>,
    /// Grenades: effect radius in cm (`handweaponcolldist`), fuse/life in ms (`handweaponlife`),
    /// flash/smoke duration in ms (`handweaponstatetime`).
    pub coll_dist: Option<u32>,
    pub life: Option<u32>,
    pub state_time: Option<u32>,
    pub snd_fire: Option<String>,
    pub snd_reload: Option<String>,
    pub snd_dryfire: Option<String>,
}

impl Weapon {
    /// zitem `reloadtime` as a number (a few pistol items omit it; 4 like the other pistols is
    /// *inferred*). Gameplay uses the reload clip length instead; see `actor.rs`.
    pub fn reload_secs(&self) -> f32 {
        self.reload_time.unwrap_or(4) as f32
    }
}

#[derive(Debug, Clone)]
pub struct Item {
    pub id: u32,
    /// Resolved from `system/strings.xml`; `None` for legacy NPC weapons (300011-300026).
    pub name: Option<String>,
    /// zitem `type`: equip, melee, range, custom, ticket, profile, customize_effect.
    pub kind: String,
    pub slot: String,
    /// `res_sex`: 'a' any, 'm', 'f'.
    pub sex: char,
    pub level: u32,
    pub weight: u32,
    pub mesh_name: Option<String>,
    pub weapon: Option<Weapon>,
}

/// One `<AddWeaponElu>` of `model/weapon.xml`.
#[derive(Debug, Clone)]
pub struct WeaponModel {
    pub name: String,
    /// Key into character `<AddAnimation motion_type>`.
    pub motion_type: u32,
    /// Finer weapon class (1 katana .. 18 2h dagger; legend in weapon.xml).
    pub weapon_type: u32,
    /// VFS path of the `.elu` (lowercased, exists in the VFS).
    pub elu: String,
}

pub struct Items {
    pub items: BTreeMap<u32, Item>,
    /// Keyed by lowercased mesh name.
    models: HashMap<String, WeaponModel>,
}

impl Items {
    pub fn load(vfs: &Vfs) -> io::Result<Self> {
        let strings = read_strings(vfs)?;
        let models = read_models(vfs)?;
        let text = xml_text(vfs, "system/zitem.xml")?;
        let doc = roxmltree::Document::parse(&text).map_err(|e| bad(format!("zitem.xml: {e}")))?;
        let mut items = BTreeMap::new();
        for n in doc
            .root_element()
            .children()
            .filter(|n| n.has_tag_name("ITEM"))
        {
            let item = read_item(n, &strings).map_err(|e| bad(format!("zitem.xml: {e}")))?;
            if let Some(m) = &item.mesh_name
                && item.weapon.is_some()
                && !models.contains_key(&m.to_ascii_lowercase())
            {
                return Err(bad(format!(
                    "item {}: mesh {m:?} not in weapon.xml",
                    item.id
                )));
            }
            let id = item.id;
            if items.insert(id, item).is_some() {
                return Err(bad(format!("zitem.xml: duplicate item id {id}")));
            }
        }
        Ok(Items { items, models })
    }

    pub fn get(&self, id: u32) -> Option<&Item> {
        self.items.get(&id)
    }

    /// First item (lowest id) whose name equals `name`, ignoring ASCII case.
    pub fn find(&self, name: &str) -> Option<&Item> {
        self.items.values().find(|i| {
            i.name
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
    }

    /// Items with weapon stats (melee, range, custom/consumables).
    pub fn weapons(&self) -> impl Iterator<Item = &Item> {
        self.items.values().filter(|i| i.weapon.is_some())
    }

    /// Model of a weapon item; `None` for items without `mesh_name` (legacy NPC weapons).
    pub fn model(&self, item: &Item) -> Option<&WeaponModel> {
        self.models
            .get(&item.mesh_name.as_ref()?.to_ascii_lowercase())
    }

    pub fn model_by_name(&self, mesh: &str) -> Option<&WeaponModel> {
        self.models.get(&mesh.to_ascii_lowercase())
    }

    pub fn models(&self) -> impl Iterator<Item = &WeaponModel> {
        self.models.values()
    }
}

fn xml_text(vfs: &Vfs, path: &str) -> io::Result<String> {
    let bytes = vfs.read(path)?;
    let text = String::from_utf8(bytes).map_err(|e| bad(format!("{path}: {e}")))?;
    Ok(text.trim_start_matches('\u{feff}').to_owned())
}

fn read_strings(vfs: &Vfs) -> io::Result<HashMap<String, String>> {
    let text = xml_text(vfs, "system/strings.xml")?;
    let doc = roxmltree::Document::parse(&text).map_err(|e| bad(format!("strings.xml: {e}")))?;
    Ok(doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name("STR"))
        .filter_map(|n| Some((n.attribute("id")?.to_owned(), n.text()?.to_owned())))
        .collect())
}

fn read_models(vfs: &Vfs) -> io::Result<HashMap<String, WeaponModel>> {
    let text = xml_text(vfs, "model/weapon.xml")?;
    let doc = roxmltree::Document::parse(&text).map_err(|e| bad(format!("weapon.xml: {e}")))?;
    let mut out: HashMap<String, WeaponModel> = HashMap::new();
    for n in doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name("AddWeaponElu"))
    {
        let name = req(n, "name")?.to_owned();
        let base = n
            .children()
            .find(|c| c.has_tag_name("AddBaseModel"))
            .ok_or_else(|| bad(format!("weapon.xml {name}: no AddBaseModel")))?;
        let elu = req(base, "filename")?
            .replace('\\', "/")
            .to_ascii_lowercase();
        if !vfs.exists(&elu) {
            return Err(bad(format!("weapon.xml {name}: {elu} not in VFS")));
        }
        let model = WeaponModel {
            motion_type: num(n, "weapon_motion_type")?,
            weapon_type: num(n, "weapon_type")?,
            name,
            elu,
        };
        // pistol20x2 is listed twice, identically.
        match out.insert(model.name.to_ascii_lowercase(), model.clone()) {
            Some(old)
                if (old.elu.as_str(), old.motion_type)
                    != (model.elu.as_str(), model.motion_type) =>
            {
                return Err(bad(format!(
                    "weapon.xml: conflicting duplicate {}",
                    model.name
                )));
            }
            _ => {}
        }
    }
    Ok(out)
}

fn req<'a>(n: roxmltree::Node<'a, '_>, attr: &str) -> io::Result<&'a str> {
    n.attribute(attr)
        .ok_or_else(|| bad(format!("<{}> missing {attr}", n.tag_name().name())))
}

fn parse<T: FromStr>(n: roxmltree::Node, attr: &str, v: &str) -> io::Result<T> {
    v.parse().map_err(|_| {
        bad(format!(
            "<{}> {attr}={v:?} is not a number",
            n.tag_name().name()
        ))
    })
}

fn num<T: FromStr>(n: roxmltree::Node, attr: &str) -> io::Result<T> {
    parse(n, attr, req(n, attr)?)
}

fn opt<T: FromStr>(n: roxmltree::Node, attr: &str) -> io::Result<Option<T>> {
    n.attribute(attr).map(|v| parse(n, attr, v)).transpose()
}

fn read_item(n: roxmltree::Node, strings: &HashMap<String, String>) -> io::Result<Item> {
    let id: u32 = num(n, "id")?;
    let ctx = |e: io::Error| bad(format!("item {id}: {e}"));
    let name = req(n, "name").map_err(ctx)?;
    let name = name
        .strip_prefix("STR:")
        .and_then(|k| strings.get(k))
        .filter(|s| s.as_str() != "#none")
        .cloned();
    let sex = req(n, "res_sex").map_err(ctx)?;
    let weapon = match n.attribute("weapon") {
        None => None,
        Some(w) => {
            let kind = WeaponKind::parse(w)
                .ok_or_else(|| bad(format!("item {id}: unknown weapon {w:?}")))?;
            let w = || -> io::Result<Weapon> {
                Ok(Weapon {
                    kind,
                    damage: num(n, "damage")?,
                    delay: num(n, "delay")?,
                    magazine: num(n, "magazine")?,
                    max_bullet: opt(n, "maxbullet")?,
                    reload_time: opt(n, "reloadtime")?,
                    range: opt(n, "range")?,
                    angle: opt(n, "angle")?,
                    ctrl_ability: opt(n, "ctrl_ability")?,
                    // retail also spells it "flase" (a typo for false)
                    slug_output: req(n, "slug_output")?.eq_ignore_ascii_case("true"),
                    effect_id: opt(n, "effect_id")?,
                    item_power: opt(n, "itempower")?,
                    damage_time: opt(n, "damagetime")?,
                    damage_type: n.attribute("damagetype").map(str::to_owned),
                    coll_dist: opt(n, "handweaponcolldist")?,
                    life: opt(n, "handweaponlife")?,
                    state_time: opt(n, "handweaponstatetime")?,
                    snd_fire: n.attribute("snd_fire").map(str::to_owned),
                    snd_reload: n.attribute("snd_reload").map(str::to_owned),
                    snd_dryfire: n.attribute("snd_dryfire").map(str::to_owned),
                })
            };
            Some(w().map_err(ctx)?)
        }
    };
    Ok(Item {
        id,
        name,
        kind: req(n, "type").map_err(ctx)?.to_owned(),
        slot: req(n, "slot").map_err(ctx)?.to_owned(),
        sex: sex
            .chars()
            .next()
            .ok_or_else(|| bad(format!("item {id}: empty res_sex")))?,
        level: opt(n, "res_level").map_err(ctx)?.unwrap_or(0),
        weight: num(n, "weight").map_err(ctx)?,
        mesh_name: n.attribute("mesh_name").map(str::to_owned),
        weapon,
    })
}
