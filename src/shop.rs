//! The offline shop and inventory (two pages of the main menu): what the shop sells
//! (`system/gshop.xml` bounty prices, stats and sell prices from `system/zitem.xml`, icons from
//! `interface/default/itemicon.xml`), and the rules for buying, selling and equipping against
//! the local [`Profile`]. Formats and constants: `docs/formats.md` (shop.xml, gshop.xml).

use crate::{
    hud::try_image,
    item::{Item, Items},
    menu::{Page, State},
    mrs::Vfs,
    phase::MenuSystems,
    profile::{self, Profile, SLOTS},
    quest::QItems,
    ui::{
        ACCENT, Chosen, DIM, Flat, INK, LINE, RAISED, body, button, compact, digits, display,
        heading, panel, primary, strong,
    },
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
/// Rows the details panel always has room for (stats plus level/price/rent/restriction).
const STAT_SLOTS: usize = 13;

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
    /// Label/value stat rows (damage, magazine, health bonus, ...) for the details panel.
    pub stats: Vec<(String, String)>,
    /// Quest item description (`zquestitem.xml`); empty for every other entry.
    pub desc: String,
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

fn stats(i: &Item, hp: u32, ap: u32) -> Vec<(String, String)> {
    let mut s = Vec::new();
    if let Some(w) = &i.weapon {
        s.push(("DAMAGE".into(), w.damage.to_string()));
        s.push(("DELAY".into(), format!("{} ms", w.delay)));
        if w.magazine > 0 {
            s.push(("MAGAZINE".into(), w.magazine.to_string()));
            s.push(("RESERVE".into(), w.max_bullet.unwrap_or(0).to_string()));
        }
        if let Some(r) = w.range {
            s.push(("REACH".into(), r.to_string()));
        }
        if let (Some(p), Some(t), Some(k)) = (w.item_power, w.damage_time, &w.damage_type) {
            s.push((k.to_uppercase(), format!("{p}/s for {t}s")));
        }
    }
    if hp > 0 {
        s.push(("HEALTH".into(), format!("+{hp}")));
    }
    if ap > 0 {
        s.push(("ARMOUR".into(), format!("+{ap}")));
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
                    desc: String::new(),
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
                stats: Vec::new(),
                desc: i.desc.clone(),
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
    RowName(Page, usize),
    RowLevel(Page, usize),
    RowPrice(Page, usize),
    Cat(usize),
    Name(Page),
    Desc(Page),
    StatLabel(Page, usize),
    StatValue(Page, usize),
    Msg,
    PageNo(Page),
}

/// One row of the details panel's stat list, hidden past the item's row count.
#[derive(Component, Clone, Copy)]
struct StatRow(Page, usize);

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
            column_gap: px(10),
            padding: UiRect::horizontal(px(6)),
            border: UiRect::all(px(1)),
            border_radius: BorderRadius::all(px(3)),
            ..default()
        },
        BackgroundColor(RAISED),
        BorderColor::all(LINE),
    )
}

fn text(txt: Txt, font: TextFont, color: Color) -> impl Bundle {
    (txt, Text::new(""), font, TextColor(color))
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
                        .with_children(|t| drop(t.spawn(text(Txt::Cat(slot), compact(14.0), INK))));
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
                        t.spawn((
                            text(Txt::RowName(page, r), strong(15.0), INK),
                            Node {
                                flex_grow: 1.0,
                                flex_basis: px(0),
                                ..default()
                            },
                        ));
                        t.spawn((
                            text(Txt::RowLevel(page, r), digits(13.0), DIM),
                            TextLayout {
                                justify: Justify::Right,
                                ..default()
                            },
                            Node {
                                width: px(50),
                                ..default()
                            },
                        ));
                        t.spawn((
                            text(Txt::RowPrice(page, r), digits(15.0), ACCENT),
                            TextLayout {
                                justify: Justify::Right,
                                ..default()
                            },
                            Node {
                                width: px(80),
                                ..default()
                            },
                        ));
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
                    text(Txt::PageNo(page), compact(16.0), DIM),
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
                text(Txt::Name(page), display(20.0), ACCENT),
                Node {
                    width: px(350),
                    margin: UiRect::top(px(8)),
                    ..default()
                },
            ));
            m.spawn((
                text(Txt::Desc(page), compact(14.0), DIM),
                Node {
                    width: px(350),
                    ..default()
                },
            ));
            m.spawn(Node {
                flex_direction: FlexDirection::Column,
                width: px(350),
                row_gap: px(3),
                margin: UiRect::vertical(px(8)),
                ..default()
            })
            .with_children(|s| {
                for i in 0..STAT_SLOTS {
                    s.spawn((
                        StatRow(page, i),
                        Node {
                            display: Display::None,
                            justify_content: JustifyContent::SpaceBetween,
                            ..default()
                        },
                    ))
                    .with_children(|row| {
                        row.spawn(text(Txt::StatLabel(page, i), compact(14.0), DIM));
                        row.spawn(text(Txt::StatValue(page, i), digits(14.0), INK));
                    });
                }
            });
            m.spawn(Node {
                column_gap: px(8),
                ..default()
            })
            .with_children(|b| {
                if shop {
                    b.spawn(primary(160.0, 40.0, "BUY", 20.0, ShopAct::Buy));
                }
                b.spawn(button(160.0, 40.0, "SELL", 20.0, ShopAct::Sell));
            });
            m.spawn((
                text(Txt::Msg, body(14.0), ACCENT),
                Node {
                    width: px(350),
                    margin: UiRect::top(px(6)),
                    ..default()
                },
            ));
        });
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
        app.insert_resource(ss).add_systems(
            Update,
            (act, refresh, profile::save).chain().in_set(MenuSystems),
        );
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

/// Label/value rows and whether the value is numeric (digits font), for the details panel.
fn detail_rows(
    e: &Entry,
    profile: &Profile,
    quest_have: Option<u32>,
) -> Vec<(String, String, bool)> {
    let mut rows = Vec::new();
    if let Some(have) = quest_have {
        rows.push(("OWNED".into(), format!("x{have}"), true));
        rows.push(("TYPE".into(), e.kind.to_uppercase(), false));
        rows.push(("SELLS FOR".into(), e.sell.to_string(), true));
        return rows;
    }
    for (label, value) in &e.stats {
        rows.push((label.clone(), value.clone(), true));
    }
    rows.push(("LEVEL".into(), e.level.to_string(), true));
    match e.sex {
        'm' => rows.push(("RESTRICTION".into(), "MEN ONLY".into(), false)),
        'f' => rows.push(("RESTRICTION".into(), "WOMEN ONLY".into(), false)),
        _ => {}
    }
    match e.price {
        Some(p) => {
            rows.push(("PRICE".into(), p.to_string(), true));
            rows.push(("SELLS FOR".into(), e.sell.to_string(), true));
        }
        None => {
            rows.push(("STATUS".into(), "NOT SOLD".into(), false));
            rows.push(("SELLS FOR".into(), e.sell.to_string(), true));
        }
    }
    if let Some(l) = left(profile, e.id) {
        rows.push(("RENTED".into(), l.to_uppercase(), false));
    }
    rows
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
    mut texts: Query<(&Txt, &mut Text)>,
    mut pics: Query<(&Pic, &mut ImageNode)>,
    mut tiles: Query<(Entity, &ShopAct, &mut Node, Has<Chosen>)>,
    mut stat_rows: Query<(&StatRow, &mut Node), Without<ShopAct>>,
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
    let rows = [Page::Shop, Page::Inventory].map(|p| {
        of(p)
            .sel
            .and_then(|id| data.entries.get(&id))
            .map(|e| {
                let quest_have = (p == Page::Inventory && ss.view(p).slot == QUEST_SLOT)
                    .then(|| profile.quest_items.get(&e.id).copied().unwrap_or(0));
                detail_rows(e, &profile, quest_have)
            })
            .unwrap_or_default()
    });
    let rows_of = |p: Page| &rows[(p != Page::Shop) as usize];
    for (t, mut text) in &mut texts {
        let s = match *t {
            Txt::RowName(p, r) => match id_at(p, r) {
                None => String::new(),
                Some(0) => "- none -".into(),
                Some(id) => data.entries[&id].name.clone(),
            },
            Txt::RowLevel(p, r) => match id_at(p, r) {
                Some(id) if id != 0 && p == Page::Shop => data.entries[&id].level.to_string(),
                _ => String::new(),
            },
            Txt::RowPrice(p, r) => match id_at(p, r) {
                None | Some(0) => String::new(),
                Some(id) => {
                    let slot = ss.view(p).slot;
                    if p == Page::Inventory && slot == QUEST_SLOT {
                        format!("x{}", profile.quest_items[&id])
                    } else if p == Page::Shop && profile.owned.contains(&id) {
                        "OWNED".into()
                    } else if p == Page::Shop {
                        data.entries[&id].price.unwrap_or(0).to_string()
                    } else if profile.equipped[slot] == id {
                        "EQUIPPED".into()
                    } else if let Some(l) = left(&profile, id) {
                        l.to_uppercase()
                    } else {
                        String::new()
                    }
                }
            },
            Txt::Cat(QUEST_SLOT) => {
                format!("QUEST ITEMS: {}", profile.quest_items.values().sum::<u32>())
            }
            Txt::Cat(slot) => format!(
                "{}: {}",
                SLOT_NAMES[slot].to_uppercase(),
                name(profile.equipped[slot])
            ),
            Txt::Name(p) => match of(p).sel.and_then(|id| data.entries.get(&id)) {
                None => "SELECT AN ITEM".into(),
                Some(e) => e.name.clone(),
            },
            Txt::Desc(p) => of(p)
                .sel
                .and_then(|id| data.entries.get(&id))
                .map(|e| e.desc.clone())
                .unwrap_or_default(),
            Txt::StatLabel(p, i) => rows_of(p).get(i).map_or(String::new(), |r| r.0.clone()),
            Txt::StatValue(p, i) => rows_of(p).get(i).map_or(String::new(), |r| r.1.clone()),
            Txt::Msg => ss.msg.clone(),
            Txt::PageNo(p) => format!("PAGE {}/{}", of(p).page + 1, of(p).pages),
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
    for (&StatRow(p, i), mut node) in &mut stat_rows {
        let display = if rows_of(p).len() > i {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
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
            desc: String::new(),
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
