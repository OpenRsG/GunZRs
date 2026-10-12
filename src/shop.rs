//! The offline shop and inventory (two pages of the main menu): what the shop sells
//! (`system/gshop.xml` bounty prices, stats and sell prices from `system/zitem.xml`, icons from
//! `interface/default/itemicon.xml`), and the rules for buying, selling and equipping against
//! the local [`Profile`]. Formats and constants: `docs/formats.md` (shop.xml, gshop.xml).

use crate::{
    hud::try_image,
    item::{Item, Items},
    menu::{Page, State, Toast},
    mrs::Vfs,
    phase::MenuSystems,
    profile::{self, Profile, SLOTS},
    quest::QItems,
    ui::{
        ACCENT, BACKDROP, Chosen, Cut, DANGER, DIM, EDGE, GOLD, INK, MUTE, OK, ON_ACCENT, OnFill,
        RAISED, Segs, TEXT, body, caption, digits, display, flat_cut, hatch, mono, panel, section,
        segs, shape, strong, tracking,
    },
    view::decode,
};
use bevy::{
    image::ImageSampler,
    prelude::*,
    text::{Justify, LineBreak},
};
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
/// Rows the details panel always has room for (item stats plus restriction and rental).
const STAT_SLOTS: usize = 8;
/// Height of the three panels (the mock's 804 px).
const PANEL_H: f32 = 804.0;
/// Segments of a list row's stat bar and of a details stat bar.
const ROW_SEGS: usize = 12;
const STAT_SEGS: usize = 20;
/// Numbered page squares under the list.
const PAGE_BTNS: usize = 5;

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

    /// Category caption of the details panel (`MELEE`, `RANGED`, `HEAD`, ...).
    fn category(&self) -> String {
        match self.kind.as_str() {
            "melee" => "MELEE".into(),
            "range" => "RANGED".into(),
            "custom" => "ITEM".into(),
            "quest" => "QUEST ITEM".into(),
            _ => self.slot.to_uppercase(),
        }
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

    /// Why `e` cannot be sold right now: not owned, still equipped or a rental. A rental cannot
    /// be sold (**inferred**: the retail dialog sells cash items for their remaining period,
    /// which has no bounty price in the data).
    pub fn can_sell(&self, e: &Entry) -> Result<(), &'static str> {
        if e.kind == "quest" {
            self.quest_items
                .contains_key(&e.id)
                .then_some(())
                .ok_or("Not owned")
        } else if !self.owned.contains(&e.id) {
            Err("Not owned")
        } else if self.equipped.contains(&e.id) {
            Err("Unequip it first")
        } else if self.rented.contains_key(&e.id) {
            Err("A rental cannot be sold")
        } else {
            Ok(())
        }
    }

    /// Sells an owned, unequipped item for its `sell_bt_price`, or one quest item for its
    /// `zquestitem.xml` `price`.
    pub fn sell(&mut self, e: &Entry) -> Result<(), &'static str> {
        self.can_sell(e)?;
        if e.kind == "quest" {
            self.spend_quest_items(&[e.id]);
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
    /// Select row `.1` of the page.
    Row(Page, usize),
    Prev(Page),
    Next(Page),
    /// The `.1`-th numbered page square.
    Goto(Page, usize),
    Buy,
    Sell,
    /// Inventory: put the selected item into its slot, or empty the slot.
    Equip,
}

/// Texts [`refresh`] rewrites. Those inside a `Flat` button (`OnFill`) keep their colour.
#[derive(Component, Clone, Copy)]
enum Txt {
    RowName(Page, usize),
    RowTag(Page, usize),
    RowLevel(Page, usize),
    RowPrice(Page, usize),
    CatCount(usize),
    CatItem(usize),
    StatHead(Page),
    PageNum(Page, usize),
    PageNo(Page),
    Name(Page),
    Sub(Page),
    Desc(Page),
    Tag(Page),
    Equipped(Page),
    StatLabel(Page, usize),
    StatValue(Page, usize),
    StatDelta(Page, usize),
    Well(Page, usize),
    Main(Page),
    Sell(Page),
}

/// A node [`refresh`] shows or hides.
#[derive(Component, Clone, Copy)]
enum Vis {
    /// The `02 <SLOT>` title of one slot.
    Sec(Page, usize),
    Row(Page, usize),
    Goto(Page, usize),
    Stat(Page, usize),
    /// The bar under a stat row (rows without a number have none).
    Bar(Page, usize),
}

/// Segment bars [`refresh`] lights: the key stat of a list row, a stat row of the details.
#[derive(Component, Clone, Copy)]
enum Bar {
    Row(Page, usize),
    Stat(Page, usize),
}

/// The equipped item's value on a details stat bar.
#[derive(Component, Clone, Copy)]
struct Marker(Page, usize);

/// A list row; [`paint_rows`] colours its ring and fill.
#[derive(Component)]
struct Tile;

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

/// Index of the first numbered page square: a window of [`PAGE_BTNS`] around `page`.
fn window(page: usize, pages: usize) -> usize {
    page.saturating_sub(PAGE_BTNS / 2)
        .min(pages.saturating_sub(PAGE_BTNS))
}

fn slot_name(slot: usize) -> &'static str {
    SLOT_NAMES.get(slot).copied().unwrap_or("Quest items")
}

/// 12200 -> `12,200`.
fn commas(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

fn text(txt: Txt, font: TextFont, color: Color) -> impl Bundle {
    (txt, Text::new(""), font, TextColor(color))
}

fn cap(txt: Txt, size: f32) -> impl Bundle {
    (text(txt, mono(size), DIM), tracking(0.14, size))
}

fn pic(p: Pic, size: f32) -> impl Bundle {
    (
        p,
        Node {
            width: px(size),
            height: px(size),
            ..default()
        },
        ImageNode {
            image_mode: NodeImageMode::Stretch,
            ..default()
        },
    )
}

/// A panel of the page's height.
fn tall<'a>(p: &'a mut ChildSpawnerCommands<'_>, w: f32) -> EntityCommands<'a> {
    let mut e = p.spawn(panel(w, AlignItems::FlexStart));
    e.entry::<Node>().and_modify(|mut n| n.height = px(PANEL_H));
    e
}

/// A tall Notch button of the left panel; the caller adds its label.
fn cat<'a>(c: &'a mut ChildSpawnerCommands<'_>, act: ShopAct) -> EntityCommands<'a> {
    let mut e = c.spawn((act, flat_cut(216.0, 56.0, Cut::Notch(14))));
    e.entry::<Node>().and_modify(|mut n| {
        n.justify_content = JustifyContent::SpaceBetween;
        n.padding = UiRect::horizontal(px(16));
    });
    e
}

/// A square button of the pagination; `num` makes its label a page number.
fn square<'a>(
    n: &'a mut ChildSpawnerCommands,
    w: f32,
    act: ShopAct,
    label: &str,
    num: Option<Txt>,
) -> EntityCommands<'a> {
    let mut e = n.spawn((act, flat_cut(w, 44.0, Cut::Notch(8))));
    e.with_children(|b| {
        let mut t = b.spawn((Text::new(label), mono(16.0), TextColor(TEXT), OnFill));
        if let Some(num) = num {
            t.insert(num);
        }
    });
    e
}

/// The contents of the shop or inventory page: slot list, 8 rows of items and a details panel.
pub(crate) fn fill(p: &mut ChildSpawnerCommands, page: Page) {
    let shop = page == Page::Shop;
    let col = |gap: f32| Node {
        flex_direction: FlexDirection::Column,
        row_gap: px(gap),
        width: percent(100),
        ..default()
    };
    tall(p, 260.0).with_children(|m| {
        m.spawn(section(1, if shop { "ARMORY" } else { "EQUIPPED" }));
        m.spawn(col(6.0)).with_children(|c| {
            if shop {
                for (slot, name) in CATS {
                    cat(c, ShopAct::Cat(page, slot)).with_children(|b| {
                        b.spawn((
                            Text::new(name.to_uppercase()),
                            display(24.0),
                            TextColor(TEXT),
                            tracking(0.08, 24.0),
                            OnFill,
                        ));
                        b.spawn((
                            Txt::CatCount(slot),
                            Text::new(""),
                            mono(13.0),
                            TextColor(TEXT),
                            OnFill,
                        ));
                    });
                }
            } else {
                for slot in 0..=QUEST_SLOT {
                    // the two texts are direct children: `hover` only recolours those
                    let mut e = cat(c, ShopAct::Cat(page, slot));
                    e.entry::<Node>().and_modify(|mut n| {
                        n.flex_direction = FlexDirection::Column;
                        n.align_items = AlignItems::FlexStart;
                        n.justify_content = JustifyContent::Center;
                    });
                    e.with_children(|b| {
                        b.spawn((
                            Text::new(slot_name(slot).to_uppercase()),
                            mono(11.0),
                            TextColor(TEXT),
                            tracking(0.14, 11.0),
                            OnFill,
                        ));
                        b.spawn((
                            Txt::CatItem(slot),
                            Text::new(""),
                            strong(15.0),
                            TextColor(TEXT),
                            TextLayout {
                                linebreak: LineBreak::NoWrap,
                                ..default()
                            },
                            Node {
                                width: px(184),
                                overflow: Overflow::clip(),
                                ..default()
                            },
                            OnFill,
                        ));
                    });
                }
            }
        });
    });
    tall(p, 860.0).with_children(|m| {
        let titles: Vec<(usize, &str)> = if shop {
            CATS.to_vec()
        } else {
            (0..=QUEST_SLOT).map(|s| (s, slot_name(s))).collect()
        };
        for (slot, name) in titles {
            m.spawn((Vis::Sec(page, slot), section(2, name)));
        }
        // column heads, aligned with the row cells below
        let right = || TextLayout {
            justify: Justify::Right,
            ..default()
        };
        m.spawn(Node {
            width: percent(100),
            column_gap: px(16),
            padding: UiRect::new(px(7), px(17), px(0), px(0)),
            ..default()
        })
        .with_children(|h| {
            h.spawn(Node {
                width: px(64),
                ..default()
            });
            h.spawn((
                caption("ITEM", 11.0),
                Node {
                    flex_grow: 1.0,
                    flex_basis: px(0),
                    ..default()
                },
            ));
            h.spawn((
                caption("LV", 11.0),
                Node {
                    width: px(60),
                    ..default()
                },
            ));
            h.spawn((
                cap(Txt::StatHead(page), 11.0),
                Node {
                    width: px(150),
                    ..default()
                },
            ));
            h.spawn((
                caption(if shop { "PRICE" } else { "SELLS FOR" }, 11.0),
                right(),
                Node {
                    width: px(110),
                    ..default()
                },
            ));
        });
        m.spawn(col(6.0)).with_children(|rows| {
            for r in 0..ROWS {
                rows.spawn((
                    Button,
                    Tile,
                    ShopAct::Row(page, r),
                    Vis::Row(page, r),
                    Node {
                        width: percent(100),
                        height: px(68),
                        padding: UiRect::all(px(1)),
                        ..default()
                    },
                    shape(Cut::Notch(10), EDGE),
                ))
                .with_children(|t| {
                    t.spawn((
                        Node {
                            width: percent(100),
                            height: percent(100),
                            align_items: AlignItems::Center,
                            column_gap: px(16),
                            padding: UiRect::new(px(6), px(16), px(0), px(0)),
                            ..default()
                        },
                        shape(Cut::Notch(8), RAISED),
                    ))
                    .with_children(|c| {
                        c.spawn((
                            Node {
                                width: px(64),
                                height: px(54),
                                flex_shrink: 0.0,
                                justify_content: JustifyContent::Center,
                                align_items: AlignItems::Center,
                                ..default()
                            },
                            hatch(Color::srgba(1.0, 1.0, 1.0, 0.012)),
                        ))
                        .with_children(|w| drop(w.spawn(pic(Pic::Row(page, r), 46.0))));
                        c.spawn(Node {
                            flex_direction: FlexDirection::Column,
                            flex_grow: 1.0,
                            flex_basis: px(0),
                            min_width: px(0),
                            row_gap: px(4),
                            overflow: Overflow::clip(),
                            ..default()
                        })
                        .with_children(|n| {
                            n.spawn((
                                text(Txt::RowName(page, r), strong(17.0), TEXT),
                                TextLayout {
                                    linebreak: LineBreak::NoWrap,
                                    ..default()
                                },
                            ));
                            n.spawn(cap(Txt::RowTag(page, r), 11.0));
                        });
                        c.spawn((
                            text(Txt::RowLevel(page, r), digits(15.0), TEXT),
                            Node {
                                width: px(60),
                                ..default()
                            },
                        ));
                        c.spawn(Node {
                            width: px(150),
                            ..default()
                        })
                        .with_children(|b| {
                            b.spawn((Bar::Row(page, r), segs(ROW_SEGS, 0, 5.0, 2.0)));
                        });
                        c.spawn((
                            text(Txt::RowPrice(page, r), digits(18.0), GOLD),
                            right(),
                            Node {
                                width: px(110),
                                ..default()
                            },
                        ));
                    });
                });
            }
        });
        m.spawn(Node {
            width: percent(100),
            align_items: AlignItems::Center,
            column_gap: px(6),
            margin: UiRect::top(Val::Auto),
            ..default()
        })
        .with_children(|n| {
            square(n, 48.0, ShopAct::Prev(page), "<", None);
            for i in 0..PAGE_BTNS {
                square(
                    n,
                    44.0,
                    ShopAct::Goto(page, i),
                    "",
                    Some(Txt::PageNum(page, i)),
                )
                .insert(Vis::Goto(page, i));
            }
            square(n, 48.0, ShopAct::Next(page), ">", None);
            n.spawn((
                text(Txt::PageNo(page), mono(13.0), DIM),
                Node {
                    margin: UiRect::left(Val::Auto),
                    ..default()
                },
            ));
        });
    });
    tall(p, 680.0).with_children(|m| {
        m.spawn((
            Node {
                width: percent(100),
                height: px(200),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            hatch(Color::srgba(1.0, 1.0, 1.0, 0.007)),
        ))
        .with_children(|a| {
            a.spawn(pic(Pic::Detail(page), 170.0));
            a.spawn((
                Text::new("03 · DETAILS"),
                mono(13.0),
                TextColor(ACCENT),
                Node {
                    position_type: PositionType::Absolute,
                    left: px(14),
                    top: px(12),
                    ..default()
                },
            ));
            a.spawn((
                text(Txt::Tag(page), strong(12.0), ON_ACCENT),
                tracking(0.2, 12.0),
                Node {
                    position_type: PositionType::Absolute,
                    right: px(0),
                    top: px(0),
                    padding: UiRect::axes(px(16), px(8)),
                    ..default()
                },
                BackgroundColor(Color::NONE),
            ));
        });
        m.spawn(Node {
            margin: UiRect::top(px(8)),
            ..col(4.0)
        })
        .with_children(|n| {
            n.spawn((
                text(Txt::Name(page), display(44.0), INK),
                tracking(0.01, 44.0),
                TextLayout {
                    linebreak: LineBreak::NoWrap,
                    ..default()
                },
                Node {
                    width: percent(100),
                    overflow: Overflow::clip(),
                    ..default()
                },
            ));
            n.spawn(cap(Txt::Sub(page), 13.0));
            n.spawn((
                text(Txt::Desc(page), body(14.0), DIM),
                Node {
                    width: percent(100),
                    ..default()
                },
            ));
        });
        m.spawn(Node {
            flex_grow: 1.0,
            ..col(10.0)
        })
        .with_children(|s| {
            for i in 0..STAT_SLOTS {
                s.spawn((Vis::Stat(page, i), col(4.0)))
                    .with_children(|row| {
                        row.spawn(Node {
                            width: percent(100),
                            align_items: AlignItems::Center,
                            column_gap: px(10),
                            ..default()
                        })
                        .with_children(|h| {
                            h.spawn((
                                cap(Txt::StatLabel(page, i), 12.0),
                                Node {
                                    flex_grow: 1.0,
                                    ..default()
                                },
                            ));
                            h.spawn(text(Txt::StatDelta(page, i), mono(13.0), OK));
                            h.spawn(text(Txt::StatValue(page, i), digits(20.0), INK));
                        });
                        row.spawn((
                            Vis::Bar(page, i),
                            Node {
                                width: percent(100),
                                height: px(10),
                                ..default()
                            },
                        ))
                        .with_children(|b| {
                            b.spawn((Bar::Stat(page, i), segs(STAT_SEGS, 0, 10.0, 3.0)));
                            b.spawn((
                                Marker(page, i),
                                Node {
                                    position_type: PositionType::Absolute,
                                    top: px(-4),
                                    width: px(2),
                                    height: px(18),
                                    ..default()
                                },
                                BackgroundColor(INK),
                            ));
                        });
                    });
            }
            s.spawn(cap(Txt::Equipped(page), 12.0));
        });
        let labels = if shop {
            ["PRICE", "SELLS FOR", "AFTER BUY"]
        } else {
            ["SELLS FOR", "BOUNTY", "AFTER SELL"]
        };
        m.spawn(Node {
            width: percent(100),
            column_gap: px(2),
            ..default()
        })
        .with_children(|w| {
            for (i, label) in labels.into_iter().enumerate() {
                w.spawn((
                    Node {
                        flex_grow: 1.0,
                        flex_basis: px(0),
                        flex_direction: FlexDirection::Column,
                        row_gap: px(4),
                        padding: UiRect::axes(px(14), px(12)),
                        ..default()
                    },
                    BackgroundColor(BACKDROP),
                ))
                .with_children(|c| {
                    c.spawn(caption(label, 11.0));
                    c.spawn(text(Txt::Well(page, i), digits(20.0), INK));
                });
            }
        });
        m.spawn(Node {
            width: percent(100),
            column_gap: px(8),
            ..default()
        })
        .with_children(|b| {
            let main = if shop { ShopAct::Buy } else { ShopAct::Equip };
            b.spawn((main, flat_cut(420.0, 64.0, Cut::Notch(14))))
                .with_children(|t| {
                    t.spawn((
                        text(Txt::Main(page), display(30.0), ON_ACCENT),
                        tracking(0.1, 30.0),
                    ));
                });
            b.spawn((ShopAct::Sell, flat_cut(208.0, 64.0, Cut::Notch(14))))
                .with_children(|t| {
                    t.spawn((
                        text(Txt::Sell(page), display(26.0), TEXT),
                        tracking(0.1, 26.0),
                    ));
                });
        });
    });
}

pub struct ShopPlugin;

impl Plugin for ShopPlugin {
    fn build(&self, app: &mut App) {
        // headless shots: `GUNZ_INV_SLOT=N` opens the inventory on equipment slot N (9 = quest
        // items), `GUNZ_SHOP_SLOT=N` the shop on category N (a slot of `CATS`)
        let mut ss = ShopState::default();
        let slot = |k: &str| std::env::var(k).ok().and_then(|s| s.parse().ok());
        ss.inv.slot = slot("GUNZ_INV_SLOT")
            .filter(|&s| s <= QUEST_SLOT)
            .unwrap_or(0);
        ss.shop.slot = slot("GUNZ_SHOP_SLOT")
            .filter(|&s| CATS.iter().any(|c| c.0 == s))
            .unwrap_or(0);
        // headless shots: `GUNZ_INV_SELL=1` presses SELL once on the first inventory row
        if std::env::var_os("GUNZ_INV_SELL").is_some() {
            app.add_systems(Startup, |mut c: Commands| {
                c.spawn((ShopAct::Sell, Interaction::Pressed));
            });
        }
        app.insert_resource(ss).add_systems(
            Update,
            (
                (act, refresh, profile::save).chain().in_set(MenuSystems),
                paint_rows.in_set(MenuSystems),
            ),
        );
    }
}

fn act(
    clicks: Query<(&Interaction, &ShopAct), Changed<Interaction>>,
    state: Res<State>,
    data: Res<ShopData>,
    mut ss: ResMut<ShopState>,
    mut profile: ResMut<Profile>,
    mut toasts: MessageWriter<Toast>,
) {
    let woman = state.cfg.woman;
    for (i, a) in &clicks {
        if *i != Interaction::Pressed {
            continue;
        }
        let mut toast = |s: String| {
            toasts.write(Toast(s));
        };
        match *a {
            ShopAct::Cat(p, slot) => *ss.view_mut(p) = View { slot, ..default() },
            ShopAct::Row(p, r) => ss.view_mut(p).sel = Some(r),
            ShopAct::Prev(p) | ShopAct::Next(p) | ShopAct::Goto(p, _) => {
                let v = ss.view(p);
                let pages = listing(p, v.slot, &data, &profile, woman)
                    .len()
                    .div_ceil(ROWS);
                let v = ss.view_mut(p);
                v.page = match *a {
                    ShopAct::Next(_) => (v.page + 1).min(pages.saturating_sub(1)),
                    ShopAct::Goto(_, i) => window(v.page, pages) + i,
                    _ => v.page.saturating_sub(1),
                };
                v.sel = Some(0);
            }
            ShopAct::Equip => {
                let slot = ss.view(Page::Inventory).slot;
                let id = selected(&ss, Page::Inventory, &data, &profile, woman);
                let Some(id) = id else {
                    toast("SELECT AN ITEM".into());
                    continue;
                };
                if slot == QUEST_SLOT {
                    toast("QUEST ITEMS ARE SACRIFICED IN THE MATCH MENU".into());
                    continue;
                }
                // a second press on an equipped item empties an emptyable slot
                let target = if id == 0 || (profile.equipped[slot] == id && slot >= 3) {
                    None
                } else {
                    data.entries.get(&id)
                };
                toast(match (profile.equip(slot, target, woman), target) {
                    (Ok(()), Some(e)) => format!("EQUIPPED · {}", e.name.to_uppercase()),
                    (Ok(()), None) => format!("UNEQUIPPED · {}", slot_name(slot).to_uppercase()),
                    (Err(m), _) => m.to_uppercase(),
                });
            }
            ShopAct::Buy | ShopAct::Sell => {
                let Some(e) = selected(&ss, state.page, &data, &profile, woman)
                    .and_then(|id| data.entries.get(&id))
                else {
                    toast("SELECT AN ITEM".into());
                    continue;
                };
                toast(match (a, profile_op(&mut profile, a, e)) {
                    (ShopAct::Buy, Ok(())) => format!("PURCHASED · {}", e.name.to_uppercase()),
                    (_, Ok(())) => {
                        println!(
                            "shop: sold {} ({}) for {} bounty -> bounty {}",
                            e.name, e.id, e.sell, profile.bounty
                        );
                        format!("SOLD · +{}", commas(e.sell.into()))
                    }
                    (_, Err(m)) => m.to_uppercase(),
                });
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

/// The leading number of a stat value: `326 ms` -> 326, `+120` -> 120.
fn number(s: &str) -> Option<f32> {
    let s = s.trim_start_matches('+');
    s[..s
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(s.len())]
        .parse()
        .ok()
}

fn stat_of(e: &Entry, label: &str) -> Option<f32> {
    e.stats
        .iter()
        .find(|(l, _)| l == label)
        .and_then(|(_, v)| number(v))
}

/// The largest `label` value among the items that fit `slot`: the end of its bars.
fn top(data: &ShopData, slot: usize, woman: bool, label: &str) -> f32 {
    data.entries
        .values()
        .filter(|e| e.fits(slot, woman))
        .filter_map(|e| stat_of(e, label))
        .fold(1.0, f32::max)
}

/// EQUIPPED or a rental's time left, else empty.
fn tag(page: Page, slot: usize, id: u32, profile: &Profile) -> String {
    let equipped = if page == Page::Shop {
        profile.equipped.contains(&id)
    } else {
        profile.equipped.get(slot) == Some(&id)
    };
    if equipped {
        "EQUIPPED".into()
    } else if let Some(l) = left(profile, id) {
        l.to_uppercase()
    } else {
        String::new()
    }
}

/// One stat row of the details panel: `bar` is (value, equipped item's value) as fractions.
struct Stat {
    label: String,
    value: String,
    delta: String,
    better: bool,
    bar: Option<(f32, Option<f32>)>,
}

impl Stat {
    fn plain(label: &str, value: String) -> Self {
        Self {
            label: label.into(),
            value,
            delta: String::new(),
            better: true,
            bar: None,
        }
    }
}

/// Everything the details panel of a page shows.
struct Detail {
    name: String,
    sub: String,
    desc: String,
    tag: String,
    stats: Vec<Stat>,
    equipped: String,
    wells: [(String, Color); 3],
    /// BUY / EQUIP: label, colour, whether it is the accent button.
    main: (String, Color, bool),
    sell: Color,
}

fn detail(
    page: Page,
    slot: usize,
    id: Option<u32>,
    data: &ShopData,
    profile: &Profile,
    woman: bool,
) -> Detail {
    let shop = page == Page::Shop;
    let e = id.and_then(|id| data.entries.get(&id));
    let eq_id = profile.equipped.get(slot).copied().unwrap_or(0);
    // what the slot holds now, unless that is the selected item itself
    let eq = data.entries.get(&eq_id).filter(|q| Some(q.id) != id);
    let bounty = i64::from(profile.bounty);
    let money = |n: i64, c: Color| (commas(n), c);
    let dash = || ("-".to_string(), DIM);
    let main: (String, Color, bool) = if shop {
        match e {
            None => ("BUY".into(), MUTE, false),
            Some(e) => match e.price {
                _ if profile.owned.contains(&e.id) => ("OWNED".into(), MUTE, false),
                None => ("NOT SOLD".into(), MUTE, false),
                Some(_) if profile.level() < e.level => {
                    (format!("LEVEL {}", e.level), DANGER, false)
                }
                Some(p) if profile.bounty < p => ("NOT ENOUGH".into(), DANGER, false),
                Some(_) => ("BUY".into(), ON_ACCENT, true),
            },
        }
    } else if slot == QUEST_SLOT {
        ("QUEST ITEM".into(), MUTE, false)
    } else {
        match id {
            None => ("EQUIP".into(), MUTE, false),
            Some(0) if eq_id == 0 => ("EMPTY".into(), MUTE, false),
            Some(0) => ("UNEQUIP".into(), TEXT, false),
            Some(i) if i == eq_id && slot >= 3 => ("UNEQUIP".into(), TEXT, false),
            Some(i) if i == eq_id => ("EQUIPPED".into(), MUTE, false),
            Some(_) => ("EQUIP".into(), ON_ACCENT, true),
        }
    };
    let mut d = Detail {
        name: if id == Some(0) {
            "EMPTY SLOT"
        } else {
            "NO ITEMS"
        }
        .into(),
        sub: String::new(),
        desc: String::new(),
        tag: String::new(),
        stats: Vec::new(),
        equipped: eq.map_or(String::new(), |q| {
            format!("EQUIPPED: {}", q.name.to_uppercase())
        }),
        wells: [dash(), dash(), dash()],
        main,
        sell: if e.is_some_and(|e| profile.can_sell(e).is_ok()) {
            TEXT
        } else {
            MUTE
        },
    };
    let Some(e) = e else { return d };
    d.name = e.name.clone();
    d.sub = if e.kind == "quest" && e.level == 0 {
        e.category()
    } else {
        format!("{} · LEVEL {} REQUIRED", e.category(), e.level)
    };
    d.desc = e.desc.clone();
    d.tag = tag(page, slot, e.id, profile);
    if d.tag.is_empty() && shop && profile.owned.contains(&e.id) {
        d.tag = "OWNED".into();
    }
    d.wells = if shop {
        let own = profile.owned.contains(&e.id);
        let price = e.price.map(i64::from);
        [
            price.map_or_else(dash, |p| {
                money(p, if p > bounty && !own { DANGER } else { GOLD })
            }),
            money(e.sell.into(), INK),
            match price {
                Some(p) if !own => money(bounty - p, if bounty < p { DANGER } else { TEXT }),
                _ => dash(),
            },
        ]
    } else {
        [
            money(e.sell.into(), GOLD),
            money(bounty, INK),
            if profile.can_sell(e).is_ok() {
                money(bounty + i64::from(e.sell), TEXT)
            } else {
                dash()
            },
        ]
    };
    if e.kind == "quest" {
        let have = profile.quest_items.get(&e.id).copied().unwrap_or(0);
        d.stats.push(Stat::plain("OWNED", format!("x{have}")));
        return d;
    }
    for (label, value) in &e.stats {
        let v = number(value);
        let ev = eq.and_then(|q| stat_of(q, label));
        let end = top(data, slot, woman, label);
        let (delta, better) = match (v, ev) {
            (Some(v), Some(ev)) if v != ev => (
                format!("{:+}", (v - ev).round() as i32),
                (v > ev) != (label == "DELAY"),
            ),
            _ => (String::new(), true),
        };
        d.stats.push(Stat {
            label: label.clone(),
            value: value.clone(),
            delta,
            better,
            bar: v.map(|v| ((v / end).min(1.0), ev.map(|ev| (ev / end).min(1.0)))),
        });
    }
    match e.sex {
        'm' => d.stats.push(Stat::plain("RESTRICTION", "MEN ONLY".into())),
        'f' => d
            .stats
            .push(Stat::plain("RESTRICTION", "WOMEN ONLY".into())),
        _ => {}
    }
    if let Some(l) = left(profile, e.id) {
        d.stats.push(Stat::plain("RENTED", l.to_uppercase()));
    }
    d
}

/// What the list of a page shows.
struct Shown {
    list: Vec<u32>,
    page: usize,
    pages: usize,
    sel: Option<u32>,
    /// The stat the rows' bars show (the first stat of the first item) and the end of its scale.
    key: String,
    top: f32,
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn refresh(
    mut commands: Commands,
    state: Res<State>,
    ss: Res<ShopState>,
    profile: Res<Profile>,
    data: Res<ShopData>,
    icons: Res<Icons>,
    mut texts: Query<(&Txt, &mut Text, &mut TextColor, &mut BackgroundColor)>,
    mut pics: Query<(&Pic, &mut ImageNode)>,
    tiles: Query<(Entity, &ShopAct, Has<Chosen>)>,
    mut vis: Query<(&Vis, &mut Node)>,
    mut marks: Query<(&Marker, &mut Node), Without<Vis>>,
    mut bars: Query<(&Bar, &mut Segs)>,
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
        let key = list
            .iter()
            .filter_map(|id| data.entries.get(id))
            .find_map(|e| e.stats.first())
            .map_or(String::new(), |s| s.0.clone());
        Shown {
            top: top(&data, v.slot, woman, &key),
            list,
            page,
            pages,
            sel,
            key,
        }
    });
    let of = |p: Page| &shown[(p != Page::Shop) as usize];
    let id_at = |p: Page, r: usize| {
        let s = of(p);
        s.list.get(s.page * ROWS + r).copied()
    };
    let entry_at = |p: Page, r: usize| id_at(p, r).and_then(|id| data.entries.get(&id));
    let details = [Page::Shop, Page::Inventory]
        .map(|p| detail(p, ss.view(p).slot, of(p).sel, &data, &profile, woman));
    let det = |p: Page| &details[(p != Page::Shop) as usize];
    let first = |p: Page| window(of(p).page, of(p).pages);
    let bounty = profile.bounty;
    for (t, mut text, mut color, mut bg) in &mut texts {
        // `None` leaves the colour to `hover` (labels inside buttons)
        let (s, c): (String, Option<Color>) = match *t {
            Txt::RowName(p, r) => match id_at(p, r) {
                None => (String::new(), None),
                Some(0) => ("NONE".into(), Some(DIM)),
                Some(id) => (
                    data.entries[&id].name.clone(),
                    Some(if of(p).sel == Some(id) { INK } else { TEXT }),
                ),
            },
            Txt::RowTag(p, r) => {
                let s =
                    id_at(p, r).map_or(String::new(), |id| tag(p, ss.view(p).slot, id, &profile));
                let c = if s.ends_with("LEFT") { DIM } else { OK };
                (s, Some(c))
            }
            Txt::RowLevel(p, r) => match entry_at(p, r) {
                None => (String::new(), None),
                Some(e) => (
                    e.level.to_string(),
                    Some(if p == Page::Shop && e.level > profile.level() {
                        DANGER
                    } else {
                        TEXT
                    }),
                ),
            },
            Txt::RowPrice(p, r) => match entry_at(p, r) {
                None => (String::new(), None),
                Some(e) if p == Page::Shop => match e.price {
                    _ if profile.owned.contains(&e.id) => ("OWNED".into(), Some(OK)),
                    Some(price) => (
                        commas(price.into()),
                        Some(if bounty < price { DANGER } else { GOLD }),
                    ),
                    None => ("-".into(), Some(DIM)),
                },
                Some(e) if e.kind == "quest" => {
                    (format!("x{}", profile.quest_items[&e.id]), Some(INK))
                }
                Some(e) => (commas(e.sell.into()), Some(GOLD)),
            },
            Txt::CatCount(slot) => (
                data.sale
                    .iter()
                    .filter(|id| data.entries.get(id).is_some_and(|e| e.fits(slot, woman)))
                    .count()
                    .to_string(),
                None,
            ),
            Txt::CatItem(QUEST_SLOT) => (
                format!("{} ITEMS", profile.quest_items.values().sum::<u32>()),
                None,
            ),
            Txt::CatItem(slot) => (
                data.entries
                    .get(&profile.equipped[slot])
                    .map_or("-".into(), |e| e.name.clone()),
                None,
            ),
            Txt::StatHead(p) => (of(p).key.clone(), Some(DIM)),
            Txt::PageNum(p, i) => ((first(p) + i + 1).to_string(), None),
            Txt::PageNo(p) => (
                format!("PAGE {} / {}", of(p).page + 1, of(p).pages),
                Some(DIM),
            ),
            Txt::Name(p) => (det(p).name.clone(), Some(INK)),
            Txt::Sub(p) => (det(p).sub.clone(), Some(DIM)),
            Txt::Desc(p) => (det(p).desc.clone(), Some(DIM)),
            Txt::Tag(p) => (det(p).tag.clone(), Some(ON_ACCENT)),
            Txt::Equipped(p) => (det(p).equipped.clone(), Some(DIM)),
            Txt::StatLabel(p, i) => (
                det(p)
                    .stats
                    .get(i)
                    .map_or(String::new(), |s| s.label.clone()),
                Some(DIM),
            ),
            Txt::StatValue(p, i) => (
                det(p)
                    .stats
                    .get(i)
                    .map_or(String::new(), |s| s.value.clone()),
                Some(INK),
            ),
            Txt::StatDelta(p, i) => match det(p).stats.get(i) {
                Some(s) => (s.delta.clone(), Some(if s.better { OK } else { DANGER })),
                None => (String::new(), None),
            },
            Txt::Well(p, i) => {
                let (s, c) = &det(p).wells[i];
                (s.clone(), Some(*c))
            }
            Txt::Main(p) => (det(p).main.0.clone(), Some(det(p).main.1)),
            Txt::Sell(p) => ("SELL".into(), Some(det(p).sell)),
        };
        set(&mut text, s);
        if let Some(c) = c {
            color.set_if_neq(TextColor(c));
        }
        if let Txt::Tag(_) = *t {
            bg.set_if_neq(BackgroundColor(match text.0.as_str() {
                "" => Color::NONE,
                "EQUIPPED" => OK,
                _ => ACCENT,
            }));
        }
    }
    for (p, mut img) in &mut pics {
        let id = match *p {
            Pic::Row(p, r) => id_at(p, r).unwrap_or(0),
            Pic::Detail(p) => of(p).sel.unwrap_or(0),
        };
        set_icon(&mut img, icons.get(&data, id));
    }
    for (b, mut segs) in &mut bars {
        let (n, frac) = match *b {
            Bar::Row(p, r) => (
                ROW_SEGS,
                entry_at(p, r)
                    .and_then(|e| stat_of(e, &of(p).key))
                    .map_or(0.0, |v| (v / of(p).top).min(1.0)),
            ),
            Bar::Stat(p, i) => (
                STAT_SEGS,
                det(p).stats.get(i).and_then(|s| s.bar).map_or(0.0, |b| b.0),
            ),
        };
        let lit = if frac > 0.0 {
            ((frac * n as f32).ceil() as usize).clamp(1, n)
        } else {
            0
        };
        segs.set_if_neq(Segs { lit });
    }
    for (&Marker(p, i), mut node) in &mut marks {
        let at = det(p).stats.get(i).and_then(|s| s.bar).and_then(|b| b.1);
        let (display, left) = match at {
            Some(f) => (Display::Flex, percent(f * 100.0)),
            None => (Display::None, node.left),
        };
        if node.display != display {
            node.display = display;
        }
        if node.left != left {
            node.left = left;
        }
    }
    for (e, a, chosen) in &tiles {
        let on = match *a {
            ShopAct::Cat(p, slot) => ss.view(p).slot == slot,
            ShopAct::Row(p, r) => ss.view(p).sel == Some(r) && id_at(p, r).is_some(),
            ShopAct::Goto(p, i) => of(p).page == first(p) + i,
            ShopAct::Buy => details[0].main.2,
            ShopAct::Equip => details[1].main.2,
            _ => continue,
        };
        match (on, chosen) {
            (true, false) => drop(commands.entity(e).insert(Chosen)),
            (false, true) => drop(commands.entity(e).remove::<Chosen>()),
            _ => {}
        }
    }
    for (v, mut node) in &mut vis {
        let shown = match *v {
            Vis::Sec(p, slot) => ss.view(p).slot == slot,
            Vis::Row(p, r) => id_at(p, r).is_some(),
            Vis::Goto(p, i) => first(p) + i < of(p).pages,
            Vis::Stat(p, i) => i < det(p).stats.len(),
            Vis::Bar(p, i) => det(p).stats.get(i).is_some_and(|s| s.bar.is_some()),
        };
        let display = if shown { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
}

/// Ring and fill of the list rows: accent when selected, lighter under the pointer.
fn paint_rows(
    mut rows: Query<(&Interaction, Has<Chosen>, &mut ImageNode, &Children), With<Tile>>,
    mut fills: Query<&mut ImageNode, Without<Tile>>,
) {
    for (i, on, mut ring, kids) in &mut rows {
        let (edge, fill) = match (on, *i) {
            (true, _) => (ACCENT, RAISED.mix(&ACCENT, 0.2)),
            (_, Interaction::None) => (EDGE, RAISED),
            _ => (MUTE, RAISED.mix(&Color::WHITE, 0.05)),
        };
        if ring.color != edge {
            ring.color = edge;
        }
        if let Some(Ok(mut inner)) = kids.first().map(|k| fills.get_mut(*k))
            && inner.color != fill
        {
            inner.color = fill;
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

    #[test]
    fn thousands_separators() {
        assert_eq!(commas(0), "0");
        assert_eq!(commas(999), "999");
        assert_eq!(commas(12200), "12,200");
        assert_eq!(commas(-1234567), "-1,234,567");
        assert_eq!(number("326 ms"), Some(326.0));
        assert_eq!(number("+120"), Some(120.0));
    }
}
