//! The offline shop and inventory (two pages of the main menu): what the shop sells
//! (`system/gshop.xml` bounty prices, stats and sell prices from `system/zitem.xml`, icons from
//! `interface/default/itemicon.xml`), and the rules for buying, selling and equipping against
//! the local [`Profile`]. Formats and constants: `docs/formats.md` (shop.xml, gshop.xml).

use crate::{
    hud::try_image,
    item::{Item, Items},
    menu::{Chosen, Flat, Page, State, button, heading, panel},
    mrs::Vfs,
    profile::{self, Profile, Ranks, SLOTS, progress},
    quest::QItems,
    view::decode,
};
use bevy::{image::ImageSampler, prelude::*, text::Justify};
use std::{
    collections::{BTreeMap, HashMap},
    io,
};

pub const SLOT_NAMES: [&str; SLOTS] = [
    "Melee",
    "Primary",
    "Secondary",
    "Item",
    "Head",
    "Chest",
    "Hands",
    "Legs",
    "Feet",
];
/// zitem `slot` of the armour slots 4..9.
const GEAR: [&str; 5] = ["head", "chest", "hands", "legs", "feet"];
/// Shop categories as equipment slots (secondary takes the same ranged weapons as primary).
const CATS: [(usize, &str); 8] = [
    (0, "Melee"),
    (1, "Ranged"),
    (3, "Item"),
    (4, "Head"),
    (5, "Chest"),
    (6, "Hands"),
    (7, "Legs"),
    (8, "Feet"),
];
/// The inventory's extra category after the equipment slots: quest items (`zquestitem.xml`).
pub const QUEST_SLOT: usize = SLOTS;
const ROWS: usize = 8;

/// One item the profile can own: the zitem record plus its bounty price.
#[derive(Clone, Debug)]
pub struct Entry {
    pub id: u32,
    pub name: String,
    /// zitem `type`: melee, range, custom or equip.
    pub kind: String,
    /// zitem `slot` (armour: head, chest, hands, legs, feet).
    pub slot: String,
    /// `res_sex`: 'a' any, 'm', 'f'.
    pub sex: char,
    /// `res_level`.
    pub level: u32,
    /// Permanent bounty price from `gshop.xml`; `None` = not sold.
    pub price: Option<u32>,
    /// `sell_bt_price`.
    pub sell: u32,
    /// Armour bonus to maximum health / armour (`hp`, `ap`).
    pub hp: u32,
    pub ap: u32,
    pub stats: Vec<String>,
}

impl Entry {
    /// Whether the item can go into equipment `slot` ([`SLOT_NAMES`]) of a man or woman.
    pub fn fits(&self, slot: usize, woman: bool) -> bool {
        let kind = match slot {
            0 => self.kind == "melee",
            1 | 2 => self.kind == "range",
            3 => self.kind == "custom",
            QUEST_SLOT => self.kind == "quest",
            _ => self.kind == "equip" && self.slot == GEAR[slot - 4],
        };
        kind && (self.sex == 'a' || (self.sex == 'f') == woman)
    }
}

#[derive(Resource)]
pub struct ShopData {
    /// Every weapon, consumable and armour item the game can use, by zitem id.
    pub entries: BTreeMap<u32, Entry>,
    /// The ids on sale, cheapest requirement first.
    pub sale: Vec<u32>,
    /// Item id -> (atlas file in `interface/default/`, the 100x100 icon's cell in it).
    icons: HashMap<u32, (String, Rect)>,
}

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

fn xml(vfs: &Vfs, path: &str) -> io::Result<String> {
    let text = String::from_utf8(vfs.read(path)?).map_err(|e| bad(format!("{path}: {e}")))?;
    Ok(text.trim_start_matches('\u{feff}').to_owned())
}

fn stats(i: &Item, hp: u32, ap: u32) -> Vec<String> {
    let mut s = Vec::new();
    if let Some(w) = &i.weapon {
        s.push(format!("Damage {}   Delay {} ms", w.damage, w.delay));
        if w.magazine > 0 {
            s.push(format!(
                "Magazine {}   Reserve {}",
                w.magazine,
                w.max_bullet.unwrap_or(0)
            ));
        }
        if let Some(r) = w.range {
            s.push(format!("Reach {r}"));
        }
        if let (Some(p), Some(t), Some(k)) = (w.item_power, w.damage_time, &w.damage_type) {
            s.push(format!("{k} {p} per second for {t} s"));
        }
    }
    if hp > 0 {
        s.push(format!("Health +{hp}"));
    }
    if ap > 0 {
        s.push(format!("Armour +{ap}"));
    }
    s
}

impl ShopData {
    pub fn load(vfs: &Vfs, items: &Items) -> io::Result<Self> {
        // zitem.xml: sell price and armour bonuses (the rest comes from `Items`).
        let text = xml(vfs, "system/zitem.xml")?;
        let doc = roxmltree::Document::parse(&text).map_err(|e| bad(format!("zitem.xml: {e}")))?;
        let mut raw: HashMap<u32, [u32; 3]> = HashMap::new();
        for n in doc
            .root_element()
            .children()
            .filter(|n| n.has_tag_name("ITEM"))
        {
            let num = |k: &str| n.attribute(k).and_then(|v| v.parse().ok()).unwrap_or(0);
            if let Some(id) = n.attribute("id").and_then(|v| v.parse().ok()) {
                raw.insert(id, [num("sell_bt_price"), num("hp"), num("ap")]);
            }
        }
        // gshop.xml: permanent (EXPIRATION_DATE 0) bounty (SELL_GROUP 1) offers that are visible.
        let text = xml(vfs, "system/gshop.xml")?;
        let doc = roxmltree::Document::parse(&text).map_err(|e| bad(format!("gshop.xml: {e}")))?;
        let mut price: HashMap<u32, u32> = HashMap::new();
        for n in doc
            .root_element()
            .children()
            .filter(|n| n.has_tag_name("SHOP_ITEM"))
        {
            let at = |k: &str| n.attribute(k).unwrap_or("");
            if at("SELL_GROUP") != "1"
                || at("EXPIRATION_DATE") != "0"
                || !at("VISIBLE").eq_ignore_ascii_case("true")
            {
                continue;
            }
            if let (Ok(id), Ok(p)) = (at("ITEM_ID").parse(), at("PRICE").parse::<u32>()) {
                let low = price.entry(id).or_insert(p);
                *low = (*low).min(p);
            }
        }
        let mut entries = BTreeMap::new();
        for i in items.items.values() {
            let usable = match i.kind.as_str() {
                "equip" => true,
                "melee" | "range" | "custom" => items.model(i).is_some(),
                _ => false,
            };
            let (true, Some(name)) = (usable, &i.name) else {
                continue;
            };
            let [sell, hp, ap] = raw.get(&i.id).copied().unwrap_or_default();
            entries.insert(
                i.id,
                Entry {
                    id: i.id,
                    name: name.clone(),
                    kind: i.kind.clone(),
                    slot: i.slot.clone(),
                    sex: i.sex,
                    level: i.level,
                    price: price.get(&i.id).copied(),
                    sell,
                    hp,
                    ap,
                    stats: stats(i, hp, ap),
                },
            );
        }
        // quest items are inventory entries too (kind "quest"; `level` is the level a page needs)
        let strings = xml(vfs, "system/strings.xml")?;
        let quest = QItems::parse(&xml(vfs, "system/zquestitem.xml")?, &strings).map_err(bad)?;
        for i in quest.0.values() {
            let entry = Entry {
                id: i.id,
                name: i.name.clone(),
                kind: "quest".into(),
                slot: "quest".into(),
                sex: 'a',
                level: i.level,
                price: None,
                sell: i.price,
                hp: 0,
                ap: 0,
                stats: [
                    i.desc.clone(),
                    format!("Type {}   Sells for {}", i.kind, i.price),
                ]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect(),
            };
            entries.insert(i.id, entry);
        }
        let mut sale: Vec<u32> = entries
            .values()
            .filter(|e| e.price.is_some())
            .map(|e| e.id)
            .collect();
        sale.sort_by_key(|id| (entries[id].level, entries[id].price, *id));
        let icons = read_icons(vfs, &entries)?;
        Ok(Self {
            entries,
            sale,
            icons,
        })
    }
}

/// `S<itemid>` icons of `itemicon.xml`: `OFFSET` counts cells of `BOUNDS` size row by row in
/// the `FILESIZE` atlas.
fn read_icons(
    vfs: &Vfs,
    entries: &BTreeMap<u32, Entry>,
) -> io::Result<HashMap<u32, (String, Rect)>> {
    let text = xml(vfs, "interface/default/itemicon.xml")?;
    let doc = roxmltree::Document::parse(&text).map_err(|e| bad(format!("itemicon.xml: {e}")))?;
    let mut icons = HashMap::new();
    for n in doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name("ITEMICONS"))
    {
        let at = |path: &[&str]| {
            path.iter()
                .try_fold(n, |n, t| n.children().find(|c| c.has_tag_name(*t)))
                .and_then(|n| n.text())
                .map(str::trim)
        };
        let num = |path: &[&str]| at(path).and_then(|v| v.parse::<f32>().ok());
        let id = at(&["ICONID"])
            .and_then(|v| v.strip_prefix('S')?.parse::<u32>().ok())
            .filter(|id| entries.contains_key(id));
        let (Some(id), Some(src)) = (id, at(&["SOURCE"])) else {
            continue;
        };
        let (Some(off), Some(w), Some(h), Some(fw)) = (
            num(&["OFFSET"]),
            num(&["BOUNDS", "W"]),
            num(&["BOUNDS", "H"]),
            num(&["FILESIZE", "W"]),
        ) else {
            return Err(bad(format!("itemicon.xml: incomplete icon S{id}")));
        };
        let cols = (fw / w).floor().max(1.0);
        let (x, y) = ((off % cols) * w, (off / cols).floor() * h);
        icons.insert(
            id,
            (src.to_ascii_lowercase(), Rect::new(x, y, x + w, y + h)),
        );
    }
    Ok(icons)
}

impl Profile {
    /// Buys the permanent offer for `e` with bounty.
    pub fn buy(&mut self, e: &Entry) -> Result<(), &'static str> {
        let price = e.price.ok_or("Not for sale")?;
        if self.owned.contains(&e.id) {
            Err("Already owned")
        } else if self.level() < e.level {
            Err("Level too low")
        } else if self.bounty < price {
            Err("Not enough bounty")
        } else {
            self.bounty -= price;
            self.owned.insert(e.id);
            Ok(())
        }
    }

    /// Sells an owned, unequipped item for its `sell_bt_price`, or one quest item for its
    /// `zquestitem.xml` `price`. A rental cannot be sold (**inferred**: the retail dialog sells
    /// cash items for their remaining period, which has no bounty price in the data).
    pub fn sell(&mut self, e: &Entry) -> Result<(), &'static str> {
        if e.kind == "quest" {
            if !self.quest_items.contains_key(&e.id) {
                return Err("Not owned");
            }
            self.spend_quest_items(&[e.id]);
        } else if !self.owned.contains(&e.id) {
            return Err("Not owned");
        } else if self.equipped.contains(&e.id) {
            return Err("Unequip it first");
        } else if self.rented.contains_key(&e.id) {
            return Err("A rental cannot be sold");
        } else {
            self.owned.remove(&e.id);
        }
        self.bounty = self.bounty.saturating_add(e.sell);
        Ok(())
    }

    /// Puts an owned item into `slot`, or empties it (`None`; melee and ranged slots cannot be
    /// empty).
    pub fn equip(
        &mut self,
        slot: usize,
        e: Option<&Entry>,
        woman: bool,
    ) -> Result<(), &'static str> {
        match e {
            None if slot < 3 => Err("This slot cannot be empty"),
            None => {
                self.equipped[slot] = 0;
                Ok(())
            }
            Some(e) if !self.owned.contains(&e.id) => Err("Not owned"),
            Some(e) if !e.fits(slot, woman) => Err("Does not fit this slot"),
            Some(e) => {
                self.equipped[slot] = e.id;
                Ok(())
            }
        }
    }
}

// ---- menu pages -------------------------------------------------------------------------

/// Atlas textures of the item icons.
#[derive(Resource)]
pub struct Icons {
    atlas: HashMap<String, Handle<Image>>,
    unknown: Handle<Image>,
}

impl Icons {
    pub fn load(vfs: &Vfs, data: &ShopData, images: &mut Assets<Image>) -> Self {
        let mut atlas = HashMap::new();
        for (src, _) in data.icons.values() {
            if !atlas.contains_key(src)
                && let Ok(bytes) = vfs.read(&format!("interface/loadable/{src}"))
                && let Some(img) = decode(&bytes, "png", true, ImageSampler::linear())
            {
                atlas.insert(src.clone(), images.add(img));
            }
        }
        Self {
            atlas,
            unknown: try_image(vfs, images, "slot_icon_unknown.tga").unwrap_or_default(),
        }
    }

    fn get(&self, data: &ShopData, id: u32) -> (Handle<Image>, Option<Rect>) {
        data.icons
            .get(&id)
            .and_then(|(src, r)| Some((self.atlas.get(src)?.clone(), Some(*r))))
            .unwrap_or_else(|| (self.unknown.clone(), None))
    }

    /// The icon of item `id` as a UI image (the "unknown" slot when `itemicon.xml` has none).
    pub fn node(&self, data: &ShopData, id: u32) -> ImageNode {
        let (image, rect) = self.get(data, id);
        ImageNode {
            rect,
            image_mode: NodeImageMode::Stretch,
            ..ImageNode::new(image)
        }
    }
}

#[derive(Component, Clone, Copy)]
pub enum ShopAct {
    /// Select equipment slot `.1` as the category (shop) or target slot (inventory).
    Cat(Page, usize),
    /// Select row `.1` of the page; in the inventory it equips the item.
    Row(Page, usize),
    Prev(Page),
    Next(Page),
    Buy,
    Sell,
}

/// Texts [`refresh`] rewrites.
#[derive(Component, Clone, Copy)]
enum Txt {
    Row(Page, usize),
    Cat(usize),
    Detail(Page),
    Msg,
    PageNo(Page),
    Card,
}

#[derive(Component, Clone, Copy)]
enum Pic {
    Row(Page, usize),
    Detail(Page),
}

#[derive(Clone)]
struct View {
    /// Equipment slot shown (shop: the category).
    slot: usize,
    page: usize,
    /// Selected row of the page.
    sel: Option<usize>,
}

impl Default for View {
    fn default() -> Self {
        Self {
            slot: 0,
            page: 0,
            sel: Some(0),
        }
    }
}

#[derive(Resource, Default)]
pub struct ShopState {
    shop: View,
    inv: View,
    msg: String,
}

impl ShopState {
    fn view(&self, p: Page) -> &View {
        if p == Page::Shop {
            &self.shop
        } else {
            &self.inv
        }
    }

    fn view_mut(&mut self, p: Page) -> &mut View {
        if p == Page::Shop {
            &mut self.shop
        } else {
            &mut self.inv
        }
    }
}

/// The item ids of a page's list for its slot (0 = "none" row of an emptyable inventory slot).
fn listing(page: Page, slot: usize, data: &ShopData, profile: &Profile, woman: bool) -> Vec<u32> {
    let fits = |id: &u32| data.entries.get(id).is_some_and(|e| e.fits(slot, woman));
    if page == Page::Shop {
        return data.sale.iter().copied().filter(fits).collect();
    }
    if slot == QUEST_SLOT {
        return profile.quest_items.keys().copied().filter(fits).collect();
    }
    let mut v = Vec::new();
    if slot >= 3 {
        v.push(0);
    }
    v.extend(profile.owned.iter().copied().filter(fits));
    v
}

/// The list's id under the selected row.
fn selected(
    ss: &ShopState,
    page: Page,
    data: &ShopData,
    profile: &Profile,
    woman: bool,
) -> Option<u32> {
    let v = ss.view(page);
    listing(page, v.slot, data, profile, woman)
        .get(v.page * ROWS + v.sel?)
        .copied()
}

/// A selectable row; the caller adds the content.
fn tile(w: f32, h: f32, act: ShopAct) -> impl Bundle {
    (
        Button,
        Flat,
        UiTransform::default(),
        act,
        Node {
            width: px(w),
            height: px(h),
            align_items: AlignItems::Center,
            padding: UiRect::horizontal(px(6)),
            border: UiRect::all(px(1)),
            border_radius: BorderRadius::all(px(6)),
            ..default()
        },
        BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.07)),
        BorderColor::all(Color::srgba(1.0, 1.0, 1.0, 0.14)),
    )
}

fn text(txt: Txt, size: f32) -> impl Bundle {
    (
        txt,
        Text::new(""),
        TextFont::from_font_size(size),
        TextColor(Color::WHITE),
    )
}

fn pic(p: Pic, size: f32) -> impl Bundle {
    (
        p,
        Node {
            width: px(size),
            height: px(size),
            margin: UiRect::right(px(8)),
            ..default()
        },
        ImageNode {
            image_mode: NodeImageMode::Stretch,
            ..default()
        },
    )
}

/// The contents of the shop or inventory page: categories, a list of 8 rows and a detail panel.
pub(crate) fn fill(p: &mut ChildSpawnerCommands, page: Page) {
    let shop = page == Page::Shop;
    p.spawn(panel(250.0, AlignItems::FlexStart))
        .with_children(|m| {
            m.spawn(heading(if shop { "CATEGORY" } else { "EQUIPPED" }));
            if shop {
                for (slot, name) in CATS {
                    m.spawn(button(220.0, 34.0, name, 16.0, ShopAct::Cat(page, slot)));
                }
            } else {
                for slot in 0..=QUEST_SLOT {
                    m.spawn(tile(220.0, 34.0, ShopAct::Cat(page, slot)))
                        .with_children(|t| drop(t.spawn(text(Txt::Cat(slot), 14.0))));
                }
            }
        });
    p.spawn(panel(560.0, AlignItems::FlexStart))
        .with_children(|m| {
            m.spawn(heading(if shop { "SHOP" } else { "INVENTORY" }));
            for r in 0..ROWS {
                m.spawn(tile(530.0, 40.0, ShopAct::Row(page, r)))
                    .with_children(|t| {
                        t.spawn(pic(Pic::Row(page, r), 32.0));
                        t.spawn(text(Txt::Row(page, r), 16.0));
                    });
            }
            m.spawn(Node {
                align_items: AlignItems::Center,
                column_gap: px(8),
                ..default()
            })
            .with_children(|n| {
                n.spawn(button(34.0, 30.0, "<", 16.0, ShopAct::Prev(page)));
                n.spawn((
                    text(Txt::PageNo(page), 16.0),
                    TextLayout {
                        justify: Justify::Center,
                        ..default()
                    },
                    Node {
                        width: px(110),
                        ..default()
                    },
                ));
                n.spawn(button(34.0, 30.0, ">", 16.0, ShopAct::Next(page)));
            });
        });
    p.spawn(panel(380.0, AlignItems::FlexStart))
        .with_children(|m| {
            m.spawn(heading("DETAILS"));
            m.spawn(pic(Pic::Detail(page), 96.0));
            m.spawn((
                text(Txt::Detail(page), 16.0),
                Node {
                    width: px(350),
                    min_height: px(150),
                    ..default()
                },
            ));
            m.spawn(Node {
                column_gap: px(8),
                ..default()
            })
            .with_children(|b| {
                if shop {
                    b.spawn(button(160.0, 40.0, "BUY", 20.0, ShopAct::Buy));
                }
                b.spawn(button(160.0, 40.0, "SELL", 20.0, ShopAct::Sell));
            });
            m.spawn((
                Txt::Msg,
                Text::new(""),
                TextFont::from_font_size(16.0),
                TextColor(Color::srgb(1.0, 0.85, 0.4)),
                Node {
                    width: px(350),
                    ..default()
                },
            ));
        });
}

/// The profile line shown at the top right of every page.
pub(crate) fn card() -> impl Bundle {
    (
        text(Txt::Card, 18.0),
        Node {
            position_type: PositionType::Absolute,
            right: px(24),
            top: px(92),
            ..default()
        },
    )
}

pub struct ShopPlugin;

impl Plugin for ShopPlugin {
    fn build(&self, app: &mut App) {
        // headless shots: `GUNZ_INV_SLOT=N` opens the inventory on equipment slot N (9 = quest items)
        let mut ss = ShopState::default();
        let slot = std::env::var("GUNZ_INV_SLOT")
            .ok()
            .and_then(|s| s.parse().ok());
        ss.inv.slot = slot.filter(|&s| s <= QUEST_SLOT).unwrap_or(0);
        // headless shots: `GUNZ_INV_SELL=1` presses SELL once on the first inventory row
        if std::env::var_os("GUNZ_INV_SELL").is_some() {
            app.add_systems(Startup, |mut c: Commands| {
                c.spawn((ShopAct::Sell, Interaction::Pressed));
            });
        }
        app.insert_resource(ss)
            .add_systems(Update, (act, refresh, profile::save).chain());
    }
}

fn act(
    clicks: Query<(&Interaction, &ShopAct), Changed<Interaction>>,
    state: Res<State>,
    data: Res<ShopData>,
    mut ss: ResMut<ShopState>,
    mut profile: ResMut<Profile>,
) {
    let woman = state.cfg.woman;
    for (i, a) in &clicks {
        if *i != Interaction::Pressed {
            continue;
        }
        match *a {
            ShopAct::Cat(p, slot) => {
                *ss.view_mut(p) = View { slot, ..default() };
                ss.msg.clear();
            }
            ShopAct::Row(p, r) => {
                ss.view_mut(p).sel = Some(r);
                if p == Page::Inventory {
                    let id = selected(&ss, p, &data, &profile, woman);
                    let slot = ss.view(p).slot;
                    let Some(id) = id else { continue };
                    if slot == QUEST_SLOT {
                        ss.msg = "Quest items are sacrificed in the match menu".into();
                        continue;
                    }
                    ss.msg = match profile.equip(slot, data.entries.get(&id), woman) {
                        Ok(()) => format!("{} equipped", SLOT_NAMES[slot]),
                        Err(e) => e.into(),
                    };
                } else {
                    ss.msg.clear();
                }
            }
            ShopAct::Prev(p) | ShopAct::Next(p) => {
                let v = ss.view(p);
                let pages = listing(p, v.slot, &data, &profile, woman)
                    .len()
                    .div_ceil(ROWS);
                let next = matches!(a, ShopAct::Next(_));
                let v = ss.view_mut(p);
                v.page = if next {
                    (v.page + 1).min(pages.saturating_sub(1))
                } else {
                    v.page.saturating_sub(1)
                };
                v.sel = Some(0);
            }
            ShopAct::Buy | ShopAct::Sell => {
                let Some(e) = selected(&ss, state.page, &data, &profile, woman)
                    .and_then(|id| data.entries.get(&id))
                else {
                    ss.msg = "Select an item first".into();
                    continue;
                };
                ss.msg = match (a, profile_op(&mut profile, a, e)) {
                    (ShopAct::Buy, Ok(())) => format!("Bought {}", e.name),
                    (_, Ok(())) => {
                        println!(
                            "shop: sold {} ({}) for {} bounty -> bounty {}",
                            e.name, e.id, e.sell, profile.bounty
                        );
                        format!("Sold {} for {}", e.name, e.sell)
                    }
                    (_, Err(m)) => m.into(),
                };
            }
        }
    }
}

fn profile_op(profile: &mut Profile, a: &ShopAct, e: &Entry) -> Result<(), &'static str> {
    match a {
        ShopAct::Buy => profile.buy(e),
        _ => profile.sell(e),
    }
}

/// Time left on a rented item, e.g. `2d 5h left`.
fn left(profile: &Profile, id: u32) -> Option<String> {
    let secs = profile
        .rented
        .get(&id)?
        .saturating_sub(crate::profile::now());
    let h = secs.div_ceil(3600);
    Some(format!("{}d {}h left", h / 24, h % 24))
}

fn set(t: &mut Text, s: String) {
    if t.0 != s {
        t.0 = s;
    }
}

fn set_icon(node: &mut ImageNode, (image, rect): (Handle<Image>, Option<Rect>)) {
    if node.image != image {
        node.image = image;
    }
    if node.rect != rect {
        node.rect = rect;
    }
}

/// What the list of a page shows.
struct Shown {
    list: Vec<u32>,
    page: usize,
    pages: usize,
    sel: Option<u32>,
}

#[allow(clippy::too_many_arguments)]
fn refresh(
    mut commands: Commands,
    state: Res<State>,
    ss: Res<ShopState>,
    profile: Res<Profile>,
    data: Res<ShopData>,
    icons: Res<Icons>,
    ranks: Res<Ranks>,
    mut texts: Query<(&Txt, &mut Text)>,
    mut pics: Query<(&Pic, &mut ImageNode)>,
    mut tiles: Query<(Entity, &ShopAct, &mut Node, Has<Chosen>)>,
) {
    if !(state.is_changed() || ss.is_changed() || profile.is_changed()) {
        return;
    }
    let woman = state.cfg.woman;
    let shown = [Page::Shop, Page::Inventory].map(|p| {
        let v = ss.view(p);
        let list = listing(p, v.slot, &data, &profile, woman);
        let pages = list.len().div_ceil(ROWS).max(1);
        let page = v.page.min(pages - 1);
        let sel = v.sel.and_then(|r| list.get(page * ROWS + r)).copied();
        Shown {
            list,
            page,
            pages,
            sel,
        }
    });
    let of = |p: Page| &shown[(p != Page::Shop) as usize];
    let id_at = |p: Page, r: usize| {
        let s = of(p);
        s.list.get(s.page * ROWS + r).copied()
    };
    let name = |id: u32| data.entries.get(&id).map_or("-", |e| e.name.as_str());
    for (t, mut text) in &mut texts {
        let s = match *t {
            Txt::Row(p, r) => match id_at(p, r) {
                None => String::new(),
                Some(0) => "- none -".into(),
                Some(id) => {
                    let e = &data.entries[&id];
                    if p == Page::Inventory && ss.view(p).slot == QUEST_SLOT {
                        format!("{}   x{}", e.name, profile.quest_items[&id])
                    } else if p == Page::Shop && profile.owned.contains(&id) {
                        format!("{}   (owned)", e.name)
                    } else if p == Page::Shop {
                        format!("{}   Lv {}   {}", e.name, e.level, e.price.unwrap_or(0))
                    } else {
                        let mut s = e.name.clone();
                        if profile.equipped[ss.view(p).slot] == id {
                            s += "   [equipped]";
                        }
                        if let Some(l) = left(&profile, id) {
                            s += &format!("   [rented, {l}]");
                        }
                        s
                    }
                }
            },
            Txt::Cat(QUEST_SLOT) => {
                format!("Quest items: {}", profile.quest_items.values().sum::<u32>())
            }
            Txt::Cat(slot) => format!("{}: {}", SLOT_NAMES[slot], name(profile.equipped[slot])),
            Txt::Detail(p) if ss.view(p).slot == QUEST_SLOT => {
                match of(p).sel.and_then(|id| data.entries.get(&id)) {
                    None => "Select an item".into(),
                    Some(e) => {
                        let have = profile.quest_items.get(&e.id).copied().unwrap_or(0);
                        let mut s = vec![format!("{}   x{have}", e.name)];
                        s.extend(e.stats.iter().cloned());
                        s.join("\n")
                    }
                }
            }
            Txt::Detail(p) => match of(p).sel.and_then(|id| data.entries.get(&id)) {
                None => "Select an item".into(),
                Some(e) => {
                    let mut s = vec![e.name.clone()];
                    s.extend(e.stats.iter().cloned());
                    s.push(format!(
                        "Level {}{}",
                        e.level,
                        match e.sex {
                            'm' => "   Men only",
                            'f' => "   Women only",
                            _ => "",
                        }
                    ));
                    match e.price {
                        Some(p) => s.push(format!("Price {p}   Sells for {}", e.sell)),
                        None => s.push(format!("Not sold   Sells for {}", e.sell)),
                    }
                    if let Some(l) = left(&profile, e.id) {
                        s.push(format!("Rented, {l}"));
                    }
                    s.join("\n")
                }
            },
            Txt::Msg => ss.msg.clone(),
            Txt::PageNo(p) => format!("Page {}/{}", of(p).page + 1, of(p).pages),
            Txt::Card => {
                let (level, into, need) = progress(profile.xp);
                let xp = if need == 0 {
                    format!("XP {}", profile.xp)
                } else {
                    format!("XP {into}/{need}")
                };
                format!(
                    "{}  [{}]  Level {level}  {xp}  Bounty {}  Medals {}",
                    profile.name,
                    ranks.code(level),
                    profile.bounty,
                    profile.medals
                )
            }
        };
        set(&mut text, s);
    }
    for (p, mut img) in &mut pics {
        let id = match *p {
            Pic::Row(p, r) => id_at(p, r).unwrap_or(0),
            Pic::Detail(p) => of(p).sel.unwrap_or(0),
        };
        set_icon(&mut img, icons.get(&data, id));
    }
    for (e, a, mut node, chosen) in &mut tiles {
        let (on, display) = match *a {
            ShopAct::Cat(p, slot) => (ss.view(p).slot == slot, Display::Flex),
            ShopAct::Row(p, r) => (
                ss.view(p).sel == Some(r) && id_at(p, r).is_some(),
                if id_at(p, r).is_some() {
                    Display::Flex
                } else {
                    Display::None
                },
            ),
            _ => continue,
        };
        if node.display != display {
            node.display = display;
        }
        match (on, chosen) {
            (true, false) => drop(commands.entity(e).insert(Chosen)),
            (false, true) => drop(commands.entity(e).remove::<Chosen>()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u32, kind: &str, price: u32, level: u32) -> Entry {
        Entry {
            id,
            name: format!("item{id}"),
            kind: kind.into(),
            slot: if kind == "equip" { "chest" } else { kind }.into(),
            sex: 'a',
            level,
            price: Some(price),
            sell: price / 10,
            hp: 0,
            ap: 0,
            stats: Vec::new(),
        }
    }

    #[test]
    fn buy_sell_equip() {
        let mut p = Profile::new();
        let (gun, vest) = (entry(7, "range", 5000, 1), entry(8, "equip", 100, 1));
        let start = p.bounty;
        p.buy(&gun).unwrap();
        assert_eq!(p.bounty, start - 5000);
        assert_eq!(p.buy(&gun), Err("Already owned"));
        assert_eq!(p.buy(&entry(9, "range", 1, 99)), Err("Level too low"));
        assert_eq!(
            p.buy(&entry(9, "range", start, 1)),
            Err("Not enough bounty")
        );
        p.equip(1, Some(&gun), false).unwrap();
        assert_eq!(p.sell(&gun), Err("Unequip it first"));
        assert_eq!(p.equip(3, Some(&gun), false), Err("Does not fit this slot"));
        assert_eq!(p.equip(5, Some(&vest), false), Err("Not owned"));
        assert_eq!(p.equip(0, None, false), Err("This slot cannot be empty"));
        p.buy(&vest).unwrap();
        p.equip(5, Some(&vest), false).unwrap();
        p.equip(5, None, false).unwrap();
        // Back to the starter revolver: the bought gun can then be sold.
        p.equip(1, Some(&entry(2050000, "range", 0, 0)), false)
            .unwrap();
        p.sell(&gun).unwrap();
        assert_eq!(p.bounty, start - 5000 - 100 + 500);
        assert!(!p.owned.contains(&7));
    }

    #[test]
    fn quest_items_sell_and_rentals_do_not() {
        let mut p = Profile::new();
        let q = Entry {
            sell: 40,
            ..entry(200008, "quest", 0, 0)
        };
        assert_eq!(p.sell(&q), Err("Not owned"));
        p.add_quest_items(&[(200008, 2)]);
        let start = p.bounty;
        p.sell(&q).unwrap();
        assert_eq!((p.bounty, p.quest_items[&200008]), (start + 40, 1));
        p.sell(&q).unwrap();
        assert!(p.quest_items.is_empty());
        let gun = entry(7, "range", 5000, 1);
        p.rent(gun.id, 72, 0);
        assert_eq!(p.sell(&gun), Err("A rental cannot be sold"));
    }
}
