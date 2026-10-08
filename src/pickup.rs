//! World items: `spawn_item_{solo|team}_{hp|ap|bullet}NN_MM` map dummies become pickups whose
//! amount, respawn time and model come from `system/worlditem.xml` (`TYPE`, `AMOUNT`, `TIME` in
//! ms, `MODELNAME` -> `MeshInfo/AddWorldItemElu`). The model's own `.elu.ani` is the idle
//! spin/bob. Solo dummies serve deathmatch, team dummies team deathmatch.
//!
//! Bots/others query pickups with `Query<(&WorldItem, &Transform)>`; see [`WorldItem::useful_for`].

use crate::{
    actor::{ActorData, HEIGHT, RADIUS},
    ani::{self, Ani},
    anim::Loop,
    effect::{FxAssets, Loader},
    elu::{self, Elu},
    game::{Loadout, Vitals},
    level::Level,
    session::Rules,
    view::{SCALE, to_bevy},
};
use bevy::prelude::*;
use std::{collections::HashMap, sync::Arc};

/// Horizontal pickup reach beyond the actor capsule radius (inferred; retail radius is not in
/// the data).
const REACH: f32 = 0.5;
/// Vertical slack under the feet (items hover; the dummy sits near the floor).
const BELOW: f32 = 0.3;
/// Sound stem (`sound/effect/<stem>.wav`) of a pickup; Feedback plays it on [`Picked`].
pub const SOUND: &str = "fx_itemget";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ItemKind {
    Hp,
    Ap,
    /// `AMOUNT` magazines added to the reserve of every gun (inferred: the XML only says 1 or 2).
    Bullet,
}

/// A spawned world item. `cooldown > 0` means it is taken and will respawn.
#[derive(Component, Debug)]
pub struct WorldItem {
    pub kind: ItemKind,
    pub amount: u32,
    /// Respawn delay in seconds (`TIME`).
    pub respawn: f32,
    pub cooldown: f32,
}

impl WorldItem {
    pub fn ready(&self) -> bool {
        self.cooldown <= 0.0
    }

    /// Ready and it would change `vitals`/`load` (full actors leave items alone).
    pub fn useful_for(&self, vitals: &Vitals, load: &Loadout, data: &ActorData) -> bool {
        self.ready()
            && match self.kind {
                ItemKind::Hp => vitals.hp < vitals.max_hp,
                ItemKind::Ap => vitals.ap < vitals.max_ap,
                ItemKind::Bullet => load.slots.iter().any(|s| room(s, data) > 0),
            }
    }
}

/// An actor took an item (for the pickup sound/effect at `at`).
#[derive(Message, Clone, Debug)]
pub struct Picked {
    pub actor: Entity,
    pub at: Vec3,
    pub kind: ItemKind,
    pub amount: u32,
}

pub struct PickupPlugin;

impl Plugin for PickupPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Picked>()
            .add_systems(PostStartup, spawn_items)
            .add_systems(Update, collect);
    }
}

struct Def {
    kind: ItemKind,
    amount: u32,
    secs: f32,
    model: String,
}

type Models = HashMap<String, (Arc<Elu>, Option<Arc<Ani>>)>;

fn spawn_items(
    mut commands: Commands,
    level: Res<Level>,
    rules: Option<Res<Rules>>,
    mut fx: FxAssets,
) {
    let Some(mode) = rules.map(|r| r.mode).filter(|m| m.items()) else {
        return;
    };
    let team = mode.team_items();
    let (defs, mut models) = match read_defs(&level) {
        Ok(d) => d,
        Err(e) => {
            warn!("worlditem.xml: {e}");
            return;
        }
    };
    let mut loader = Loader::new(&level.vfs, "model/");
    let want = if team {
        "spawn_item_team_"
    } else {
        "spawn_item_solo_"
    };
    let mut count = 0;
    for d in &level.map.dummies {
        let Some(rest) = d
            .name
            .to_ascii_lowercase()
            .strip_prefix(want)
            .map(str::to_owned)
        else {
            continue;
        };
        let name = rest.rsplit_once('_').map_or(rest.as_str(), |(n, _)| n);
        let Some(def) = defs.get(name) else {
            warn!("{}: no worlditem {name}", d.name);
            continue;
        };
        let at = Vec3::from(to_bevy(d.pos)) * SCALE;
        let root = commands
            .spawn((
                Name::new(d.name.clone()),
                // the ELU root inherits this scale (see `Model::attach`)
                Transform::from_translation(at).with_scale(Vec3::splat(SCALE)),
                Visibility::default(),
                WorldItem {
                    kind: def.kind,
                    amount: def.amount,
                    respawn: def.secs,
                    cooldown: 0.0,
                },
            ))
            .id();
        // the glow base (`baseEffect`) first, then the item model, both looping
        for m in ["baseEffect", def.model.as_str()] {
            let Some((elu, ani)) = models.get_mut(m) else {
                continue;
            };
            let model = loader.spawn(
                &mut fx,
                &mut commands,
                "model/worlditem/",
                elu,
                ani.clone().map(|a| (a, Loop::Wrap)),
                Transform::IDENTITY,
            );
            model.attach(&mut commands, root);
        }
        count += 1;
    }
    println!(
        "{count} world items ({})",
        if team { "team" } else { "solo" }
    );
}

fn read_defs(level: &Level) -> std::io::Result<(HashMap<String, Def>, Models)> {
    let bad = |e: String| std::io::Error::new(std::io::ErrorKind::InvalidData, e);
    let text = String::from_utf8_lossy(&level.vfs.read("system/worlditem.xml")?).into_owned();
    let doc = roxmltree::Document::parse(text.trim_start_matches('\u{feff}'))
        .map_err(|e| bad(e.to_string()))?;
    fn field<'a>(n: roxmltree::Node<'a, '_>, tag: &str) -> Option<&'a str> {
        n.children()
            .find(|c| c.has_tag_name(tag))
            .and_then(|c| c.text())
            .map(str::trim)
    }
    let mut defs = HashMap::new();
    for n in doc.descendants().filter(|n| n.has_tag_name("WORLDITEM")) {
        let kind = match field(n, "TYPE") {
            Some("hp") => ItemKind::Hp,
            Some("ap") => ItemKind::Ap,
            Some("bullet") => ItemKind::Bullet,
            _ => continue,
        };
        let num = |t| field(n, t).and_then(|v| v.parse::<u32>().ok());
        let (Some(name), Some(amount), Some(ms), Some(model)) = (
            n.attribute("name"),
            num("AMOUNT"),
            num("TIME"),
            field(n, "MODELNAME"),
        ) else {
            continue;
        };
        defs.insert(
            name.to_ascii_lowercase(),
            Def {
                kind,
                amount,
                secs: ms as f32 / 1000.0,
                model: model.to_owned(),
            },
        );
    }
    let mut models = Models::new();
    for n in doc
        .descendants()
        .filter(|n| n.has_tag_name("AddWorldItemElu"))
    {
        let file = |tag: &str| {
            n.children()
                .find(|c| c.has_tag_name(tag))
                .and_then(|c| c.attribute("filename"))
                .map(|f| f.replace('\\', "/").to_ascii_lowercase())
        };
        let (Some(name), Some(path)) = (n.attribute("name"), file("AddBaseModel")) else {
            continue;
        };
        let Ok(elu) = level.vfs.read(&path).and_then(|b| elu::load(&b)) else {
            continue;
        };
        let ani = file("AddAnimation")
            .and_then(|p| level.vfs.read(&p).ok())
            .and_then(|b| ani::load(&b).ok());
        models.insert(name.to_owned(), (Arc::new(elu), ani.map(Arc::new)));
    }
    Ok((defs, models))
}

/// Rounds of room left in `slot`'s reserve: the cap is `maxbullet - magazine` as in `actor::gear`.
fn room(slot: &crate::game::Slot, data: &ActorData) -> u32 {
    let Some(w) = data
        .items
        .get(slot.item)
        .and_then(|i| i.weapon.as_ref())
        .filter(|w| w.magazine > 0)
    else {
        return 0;
    };
    let cap = w.max_bullet.unwrap_or(w.magazine * 4);
    cap.saturating_sub(w.magazine).saturating_sub(slot.reserve)
}

fn collect(
    time: Res<Time>,
    data: Res<ActorData>,
    mut round: MessageReader<crate::game::NewRound>,
    mut items: Query<(&mut WorldItem, &GlobalTransform, &mut Visibility)>,
    mut actors: Query<(Entity, &Transform, &mut Vitals, &mut Loadout), Without<crate::game::Dead>>,
    mut picked: MessageWriter<Picked>,
) {
    let dt = time.delta_secs();
    // retail resets items each round (inferred, per Modes)
    let refill = round.read().count() > 0;
    for (mut it, at, mut vis) in &mut items {
        if refill {
            it.cooldown = 0.0;
            *vis = Visibility::Inherited;
        }
        if it.cooldown > 0.0 {
            it.cooldown -= dt;
            if it.cooldown <= 0.0 {
                *vis = Visibility::Inherited;
                println!("respawn {:?} +{}", it.kind, it.amount);
            }
            continue;
        }
        let p = at.translation();
        for (actor, tf, mut v, mut load) in &mut actors {
            let d = tf.translation - p;
            let near = d.x.hypot(d.z) < RADIUS + REACH && (-HEIGHT..BELOW).contains(&d.y);
            if !near || !it.useful_for(&v, &load, &data) {
                continue;
            }
            let a = it.amount;
            match it.kind {
                ItemKind::Hp => v.hp = (v.hp + a as f32).min(v.max_hp),
                ItemKind::Ap => v.ap = (v.ap + a as f32).min(v.max_ap),
                ItemKind::Bullet => {
                    for s in &mut load.slots {
                        let mag = data
                            .items
                            .get(s.item)
                            .and_then(|i| i.weapon.as_ref())
                            .map_or(0, |w| w.magazine);
                        s.reserve += (a * mag).min(room(s, &data));
                    }
                }
            }
            it.cooldown = it.respawn;
            *vis = Visibility::Hidden;
            picked.write(Picked {
                actor,
                at: p,
                kind: it.kind,
                amount: a,
            });
            println!(
                "pickup {:?} +{a} actor {actor} (hp {:.0} ap {:.0})",
                it.kind, v.hp, v.ap
            );
            break;
        }
    }
}
