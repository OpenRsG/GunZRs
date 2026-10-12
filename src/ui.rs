//! The game's look, shared by the menus and the HUD: colour tokens, the type (`assets/fonts`, SIL
//! Open Font License, see `CREDITS.md`), the angular shapes and patterns, and the widgets every
//! screen builds from (buttons, panels, section headings, segment bars, key caps). UI colours and
//! fonts come from here, nowhere else. Rules: `DESIGN.md`.

use crate::{mrs::Vfs, phase::Mount, view::Shot};
use bevy::{
    asset::{RenderAssetUsages, embedded_asset, uuid::Uuid, uuid_handle},
    prelude::*,
    render::render_resource::{AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat},
    shader::ShaderRef,
    sprite::{BorderRect, SliceScaleMode, TextureSlicer},
    text::{FontFeatureTag, LetterSpacing},
    ui::VisualBox,
};
use std::{collections::HashMap, marker::PhantomData, sync::Arc};

/// Primary text: warm off-white.
pub const INK: Color = Color::srgb(0.914, 0.906, 0.886);
/// Button labels and list rows: a cooler, slightly darker white.
pub const TEXT: Color = Color::srgb(0.839, 0.843, 0.855);
/// Secondary text and captions.
pub const DIM: Color = Color::srgb(0.545, 0.561, 0.592);
/// Tertiary text: key hints, unlit indices, disabled labels.
pub const MUTE: Color = Color::srgb(0.298, 0.314, 0.345);
/// The accent (ember orange): selection, the main action, highlights.
pub const ACCENT: Color = Color::srgb(1.0, 0.416, 0.122);
/// The accent's warm light: prices, bounty, the ring of a chosen tile.
pub const GOLD: Color = Color::srgb(1.0, 0.702, 0.278);
/// Text and icons drawn on the accent.
pub const ON_ACCENT: Color = Color::srgb(0.078, 0.031, 0.012);
/// Damage, death, low health, errors, unaffordable prices.
pub const DANGER: Color = Color::srgb(1.0, 0.302, 0.239);
/// Healing and armour, confirmations, owned and equipped items, better stats.
pub const OK: Color = Color::srgb(0.608, 0.89, 0.235);
/// Team colours (names, scores, bars).
pub const RED: Color = Color::srgb(1.0, 0.4, 0.34);
pub const BLUE: Color = Color::srgb(0.4, 0.68, 1.0);
/// The screen behind the menus.
pub const BACKDROP: Color = Color::srgb(0.039, 0.043, 0.051);
/// Panel fill: near-black, cool, nearly opaque.
pub const SURFACE: Color = Color::srgba(0.086, 0.094, 0.11, 0.94);
/// Tiles and rows on a panel (map tiles, list rows, stepper wells).
pub const RAISED: Color = Color::srgb(0.114, 0.125, 0.145);
/// Buttons on a panel.
pub const BUTTON: Color = Color::srgb(0.133, 0.145, 0.169);
/// Unlit segments, outlines of tiles and swatches, empty bar tracks.
pub const EDGE: Color = Color::srgb(0.165, 0.176, 0.2);
/// HUD slab under text that must stay legible over the game (feed rows, score plate).
pub const PLATE: Color = Color::srgba(0.02, 0.025, 0.03, 0.6);
/// Hairlines: separators and outlines.
pub const LINE: Color = Color::srgba(1.0, 1.0, 1.0, 0.07);
/// Dims the game behind a full-screen overlay (pause, death).
pub const SCRIM: Color = Color::srgba(0.0, 0.0, 0.0, 0.6);
/// Drop shadows (panels, text over the 3D view).
pub const SHADOW: Color = Color::srgba(0.0, 0.0, 0.0, 0.65);

/// Big Shoulders Display ExtraBold (titles), Chakra Petch SemiBold and Bold (rows, buttons) and
/// JetBrains Mono Bold (numbers, captions); running text is Chakra Petch Medium on the default
/// handle.
const DISPLAY: Handle<Font> = uuid_handle!("ccd195c1-da71-4241-9e1e-e63f2be22b12");
const STRONG: Handle<Font> = uuid_handle!("cf47e716-9480-4657-8089-90179f7a64ad");
const CONDENSED: Handle<Font> = uuid_handle!("f90dad06-53ff-46cd-90e2-ba4718c3676f");
const MONO: Handle<Font> = uuid_handle!("6f1c2a5e-0b7d-4a43-9d0e-3c8f5b1e2a71");

/// Installs the fonts and the shape and pattern images, and paints [`Segs`] bars.
pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        let mut fonts = app.world_mut().resource_mut::<Assets<Font>>();
        for (id, bytes) in [
            (
                AssetId::default(),
                &include_bytes!("../assets/fonts/ChakraPetch-Medium.ttf")[..],
            ),
            (
                STRONG.id(),
                include_bytes!("../assets/fonts/ChakraPetch-Bold.ttf"),
            ),
            (
                CONDENSED.id(),
                include_bytes!("../assets/fonts/ChakraPetch-SemiBold.ttf"),
            ),
            (
                DISPLAY.id(),
                include_bytes!("../assets/fonts/BigShouldersDisplay-ExtraBold.ttf"),
            ),
            (
                MONO.id(),
                include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf"),
            ),
        ] {
            fonts
                .insert(id, Font::from_bytes(bytes.to_vec()))
                .expect("font asset slot");
        }
        let mut images = app.world_mut().resource_mut::<Assets<Image>>();
        for kind in [Cut::Notch(0), Cut::Chamfer(0), Cut::Slant(0)] {
            for size in CUT_SIZES {
                let cut = kind.with(size);
                images
                    .insert(cut.handle().id(), cut.mask())
                    .expect("mask asset slot");
            }
        }
        images
            .insert(
                HATCH.id(),
                stripes(HATCH_PERIOD, |on| on.then_some([255; 4])),
            )
            .expect("pattern asset slot");
        images
            .insert(
                HAZARD.id(),
                stripes(HAZARD_PERIOD, |on| {
                    Some(if on {
                        [255, 106, 31, 255]
                    } else {
                        [10, 11, 13, 255]
                    })
                }),
            )
            .expect("pattern asset slot");
        embedded_asset!(app, "metal.wgsl");
        // `hover` runs in each screen's own chain, after the systems that pick `Chosen`
        app.add_plugins(UiMaterialPlugin::<MetalMaterial>::default())
            .add_message::<UiSound>()
            .add_systems(Update, (paint_segs, metal_make, metal_look).chain())
            .add_systems(Update, sounds);
    }
}

/// Interface sounds: stems of `sound/effect/` that `effect.xml` lists as 2D (`type="1"`/`"3"`),
/// which cue is which is **inferred** from the names and lengths (`docs/formats.md`, "Interface
/// sounds"). Other modules send a [`UiSound`] with one of these; buttons need nothing, [`sounds`]
/// plays the hover and click of every [`Interaction`] change.
pub mod cue {
    /// Every button, in every screen: hover, click.
    pub const OVER: &str = "mouse_over";
    pub const CLICK: &str = "mouse_click";
    /// Refused action, unaffordable item, nothing selected.
    pub const ERROR: &str = "if_error";
    pub const POP: &str = "pop_up";
    /// A window opens / closes (the pause menu).
    pub const OPEN: &str = "mail";
    pub const CLOSE: &str = "mail_close";
    pub const EQUIP: &str = "item";
    pub const UNEQUIP: &str = "item_off";
    pub const BUY: &str = "item_buy";
    pub const SELL: &str = "trash";
    /// START: the loading screen opens.
    pub const ENTER: &str = "match_enter";
    /// A round's countdown begins.
    pub const COUNTDOWN: &str = "match_countdown";
    pub const WIN: &str = "result_mark_win";
    pub const LOSE: &str = "result_mark_lose";
    pub const DRAW: &str = "result_mark_draw";
    /// A level gained in the match (end screen).
    pub const LEVEL_UP: &str = "tier_up";
    /// Blitzkrieg: class chosen, honor reward.
    pub const CONFIRM: &str = "match_accept";
    pub const REWARD: &str = "if_score_get";
    /// The XP bar of the end screen fills.
    pub const BAR: &str = "result_bar";

    pub const ALL: [&str; 19] = [
        OVER, CLICK, ERROR, POP, OPEN, CLOSE, EQUIP, UNEQUIP, BUY, SELL, ENTER, COUNTDOWN, WIN,
        LOSE, DRAW, LEVEL_UP, CONFIRM, REWARD, BAR,
    ];
}

/// Play interface sound `.0` (one of [`cue`]) in 2D.
#[derive(Message, Clone, Copy, Debug)]
pub struct UiSound(pub &'static str);

/// The sound of a button's [`Interaction`] change, if any: hover on entering (not when a held
/// press falls back to hover), click on pressing.
fn feel(was: Interaction, now: Interaction) -> Option<&'static str> {
    match (was, now) {
        (Interaction::None, Interaction::Hovered) => Some(cue::OVER),
        (_, Interaction::Pressed) if was != Interaction::Pressed => Some(cue::CLICK),
        _ => None,
    }
}

/// Reads, decodes and logs every [`cue::ALL`] stem; a missing or undecodable one is skipped.
fn load_cues(
    vfs: &Vfs,
    assets: &mut Assets<AudioSource>,
) -> HashMap<&'static str, Handle<AudioSource>> {
    let mut out = HashMap::new();
    for stem in cue::ALL {
        let path = format!("sound/effect/{stem}.wav");
        let Ok(bytes) = vfs.read(&path) else {
            warn!("ui sound: {path} is not in the archives");
            continue;
        };
        let bytes: Arc<[u8]> = bytes.into();
        let Some((ch, hz, secs)) = crate::music::decode(&bytes, None) else {
            warn!("ui sound: {stem} does not decode");
            continue;
        };
        info!(
            "ui sound: {stem}: {} kB, {ch} ch, {hz} Hz, {secs:.2} s, decodes",
            bytes.len() / 1000
        );
        out.insert(stem, assets.add(AudioSource { bytes }));
    }
    out
}

/// Plays [`UiSound`]s and the hover/click of every button, in every screen. `--shot` runs (no
/// audio device) log what would play instead.
#[allow(clippy::too_many_arguments)]
fn sounds(
    mount: Option<Res<Mount>>,
    shot: Option<Res<Shot>>,
    changed: Query<(Entity, &Interaction), Changed<Interaction>>,
    mut asked: MessageReader<UiSound>,
    mut was: Local<HashMap<Entity, Interaction>>,
    mut cues: Local<Option<HashMap<&'static str, Handle<AudioSource>>>>,
    mut assets: ResMut<Assets<AudioSource>>,
    mut commands: Commands,
) {
    let Some(mount) = mount else { return };
    let cues = cues.get_or_insert_with(|| load_cues(&mount.0, &mut assets));
    let mut play: Vec<&'static str> = changed
        .iter()
        .filter_map(|(e, now)| feel(was.insert(e, *now).unwrap_or_default(), *now))
        .collect();
    if was.len() > 4096 {
        was.clear();
    }
    play.extend(asked.read().map(|m| m.0));
    play.dedup();
    for stem in play {
        info!("ui sound: play {stem}");
        if let (Some(h), None) = (cues.get(stem), &shot) {
            commands.spawn((AudioPlayer::new(h.clone()), PlaybackSettings::DESPAWN));
        }
    }
}

/// The UI's one motion curve: cubic ease-out over `t` in 0..1 (clamped), for entrances that
/// start fast and settle.
pub fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// Running text: descriptions, hints, settings help.
pub fn body(size: f32) -> TextFont {
    TextFont::from_font_size(size)
}

/// Compact labels and list rows (semibold).
pub fn compact(size: f32) -> TextFont {
    TextFont::from_font_size(size).with_font(CONDENSED)
}

/// Buttons, names, tags (bold).
pub fn strong(size: f32) -> TextFont {
    TextFont::from_font_size(size).with_font(STRONG)
}

/// Titles, banners, tabs and big call-to-action labels (condensed display).
pub fn display(size: f32) -> TextFont {
    TextFont::from_font_size(size).with_font(DISPLAY)
}

/// Captions, indices and key caps (monospace, usually letter-spaced capitals).
pub fn mono(size: f32) -> TextFont {
    TextFont::from_font_size(size).with_font(MONO)
}

/// Numbers that change in place (clock, ammo, health, scores, prices): monospace, so a value
/// does not jitter as its digits change.
pub fn digits(size: f32) -> TextFont {
    TextFont {
        font_features: [FontFeatureTag::TABULAR_FIGURES].into(),
        ..mono(size)
    }
}

/// Letter spacing of `em` times the font size `size` (the mock tracks captions 0.14 em, button
/// labels about 0.06 em).
pub fn tracking(em: f32, size: f32) -> LetterSpacing {
    LetterSpacing::Px(em * size)
}

/// The shadow that keeps text readable over the 3D view.
pub fn shadow() -> TextShadow {
    TextShadow {
        offset: Vec2::new(0.0, 1.5),
        color: SHADOW,
    }
}

/// The angular cuts of the look, drawn by a 9-sliced white mask tinted with the fill ([`shape`]):
/// `Notch` cuts the bottom-right corner (tiles, rows, secondary buttons), `Chamfer` the top-left
/// and bottom-right corners (panels, hero frames), `Slant` slants both sides into a
/// parallelogram (choice buttons, tabs of a switch). The size is the cut in px; sizes are
/// rounded to the nearest of [`CUT_SIZES`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cut {
    Notch(u8),
    Chamfer(u8),
    Slant(u8),
}

/// The cut sizes that have a mask.
pub const CUT_SIZES: [u8; 9] = [4, 6, 8, 10, 12, 14, 16, 18, 20];

/// Mask texels per logical px: the cut edges stay sharp up to this UI scale (a 4K window scales
/// the menus about 2x; a 1-texel-per-px mask was magnified into a soft, stepped edge).
const MASK_RES: usize = 4;

impl Cut {
    fn size(self) -> u8 {
        let (Cut::Notch(s) | Cut::Chamfer(s) | Cut::Slant(s)) = self;
        *CUT_SIZES
            .iter()
            .min_by_key(|c| c.abs_diff(s))
            .expect("cut sizes")
    }

    fn with(self, size: u8) -> Self {
        match self {
            Cut::Notch(_) => Cut::Notch(size),
            Cut::Chamfer(_) => Cut::Chamfer(size),
            Cut::Slant(_) => Cut::Slant(size),
        }
    }

    fn handle(self) -> Handle<Image> {
        let kind = match self {
            Cut::Notch(_) => 1u128,
            Cut::Chamfer(_) => 2,
            Cut::Slant(_) => 3,
        };
        let id = 0x5a1c_e000_0000_4000_8000_0000_0000_0000u128 | kind << 8 | self.size() as u128;
        Handle::Uuid(Uuid::from_u128(id), PhantomData)
    }

    /// The mask: white, alpha = coverage (4 x 4 samples a texel). Corners are `c` x
    /// [`MASK_RES`] texels, which the slicer draws `c` logical px.
    fn mask(self) -> Image {
        let c = self.size() as usize * MASK_RES;
        let (w, h) = match self {
            Cut::Slant(_) => (2 * c + 2 * MASK_RES, c),
            _ => (2 * c + 2 * MASK_RES, 2 * c + 2 * MASK_RES),
        };
        let (wf, hf, cf) = (w as f32, h as f32, c as f32);
        let inside = |x: f32, y: f32| match self {
            // left edge from (0, h) to (c, 0), right edge from (w, 0) to (w - c, h)
            Cut::Slant(_) => x >= cf * (1.0 - y / hf) && x <= wf - cf * y / hf,
            Cut::Notch(_) => x + y <= wf + hf - cf,
            Cut::Chamfer(_) => x + y <= wf + hf - cf && x + y >= cf,
        };
        let mut data = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let mut n = 0;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let px = x as f32 + (sx as f32 + 0.5) / 4.0;
                        let py = y as f32 + (sy as f32 + 0.5) / 4.0;
                        n += inside(px, py) as u32;
                    }
                }
                data.extend_from_slice(&[255, 255, 255, (n * 255 / 16) as u8]);
            }
        }
        image(w as u32, h as u32, data)
    }

    fn slicer(self) -> TextureSlicer {
        let c = (self.size() as usize * MASK_RES) as f32;
        let border = match self {
            Cut::Slant(_) => BorderRect {
                min_inset: Vec2::new(c, 0.0),
                max_inset: Vec2::new(c, 0.0),
            },
            _ => BorderRect::all(c),
        };
        TextureSlicer {
            border,
            center_scale_mode: SliceScaleMode::Stretch,
            sides_scale_mode: SliceScaleMode::Stretch,
            max_corner_scale: 1.0 / MASK_RES as f32,
        }
    }
}

/// A node filled with `color` in the `cut` shape. Keep the node's `BackgroundColor` clear: it
/// would paint the full rectangle under the cut corners. [`hover`] tints it on [`Flat`] buttons.
pub fn shape(cut: Cut, color: Color) -> ImageNode {
    ImageNode {
        image: cut.handle(),
        color,
        image_mode: NodeImageMode::Sliced(cut.slicer()),
        // the whole node, not just its content box inside the padding
        visual_box: VisualBox::BorderBox,
        ..default()
    }
}

const HATCH: Handle<Image> = uuid_handle!("2b8e5a0c-7d41-4f2a-9c63-1e0f4d7b9a52");
const HAZARD: Handle<Image> = uuid_handle!("8d3f6c21-5e9a-4b07-a1d8-6c2e9f0b3a74");
/// Stripe periods in texels along x (45° stripes, half on, half off).
const HATCH_PERIOD: usize = 8;
const HAZARD_PERIOD: usize = 28;

/// A tileable square of 45° stripes: `paint(on)` gives the texel of the lit and the unlit half.
fn stripes(period: usize, paint: impl Fn(bool) -> Option<[u8; 4]>) -> Image {
    let mut data = Vec::with_capacity(period * period * 4);
    for y in 0..period {
        for x in 0..period {
            // 135° (top-left to bottom-right falling), like the mock's hazard band
            let on = (x + period - y) % period < period / 2;
            data.extend_from_slice(&paint(on).unwrap_or([0; 4]));
        }
    }
    image(period as u32, period as u32, data)
}

/// A button plate of brushed orange metal (`metal.wgsl`) in the `Notch(notch)` shape, `w` x `h`
/// px: [`metal_look`] feeds the shader its sweep, hover and pressed state. Put the label in it.
pub fn metal(w: f32, h: f32, notch: f32) -> impl Bundle {
    (
        Button,
        UiTransform::default(),
        Node {
            width: px(w),
            height: px(h),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        Metal { w, h, notch },
    )
}

/// [`primary`] on metal: the pause menu's RESUME.
pub fn metal_primary<A: Component>(w: f32, h: f32, label: &str, size: f32, act: A) -> impl Bundle {
    (
        act,
        metal(w, h, 14.0),
        children![(
            Text::new(label.to_uppercase()),
            display(size * 1.2),
            TextColor(ON_ACCENT),
            tracking(0.04, size * 1.2),
        )],
    )
}

/// The size (px) and notch of a [`metal`] plate; [`metal_make`] gives it its material.
#[derive(Component, Clone, Copy)]
pub struct Metal {
    w: f32,
    h: f32,
    notch: f32,
}

#[derive(Asset, TypePath, AsBindGroup, Clone)]
struct MetalMaterial {
    #[uniform(0)]
    params: MetalParams,
}

/// `metal.wgsl`'s `Metal`: tint (linear); width, height, notch, brightness; sweep position
/// (negative: none).
#[derive(ShaderType, Clone, Copy)]
struct MetalParams {
    color: Vec4,
    shape: Vec4,
    anim: Vec4,
}

impl UiMaterial for MetalMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://gunz/metal.wgsl".into()
    }
}

/// Seconds between the light strip's sweeps and of one sweep.
const SWEEP_EVERY: f32 = 3.5;
const SWEEP_SECS: f32 = 0.9;
/// The shader's brightness: 1.0 is the rest state (`metal.wgsl` tone maps with it).
const METAL_LIGHT: f32 = 1.0;

fn metal_make(
    mut commands: Commands,
    new: Query<(Entity, &Metal), Added<Metal>>,
    mut materials: ResMut<Assets<MetalMaterial>>,
) {
    for (e, m) in &new {
        let material = MetalMaterial {
            params: MetalParams {
                color: ACCENT.to_linear().to_vec4(),
                shape: Vec4::new(m.w, m.h, m.notch, METAL_LIGHT),
                anim: Vec4::NEG_ONE,
            },
        };
        commands
            .entity(e)
            .insert(MaterialNode(materials.add(material)));
    }
}

/// Hover, pressed and the sweeping strip of every [`Metal`] plate. Real time: the pause menu
/// stops the game clock.
fn metal_look(
    real: Res<Time<Real>>,
    mut materials: ResMut<Assets<MetalMaterial>>,
    mut plates: Query<(&MaterialNode<MetalMaterial>, &Interaction, &mut UiTransform), With<Metal>>,
) {
    let t = (real.elapsed_secs() % SWEEP_EVERY) / SWEEP_SECS;
    let sweep = if t < 1.0 { ease_out(t) } else { -1.0 };
    for (node, i, mut tf) in &mut plates {
        if let Some(mut m) = materials.get_mut(&node.0) {
            m.params.anim.x = sweep;
            m.params.shape.w = METAL_LIGHT
                * match i {
                    Interaction::Pressed => 0.8,
                    Interaction::Hovered => 1.15,
                    Interaction::None => 1.0,
                };
        }
        let scale = Vec2::splat(if *i == Interaction::Pressed {
            0.97
        } else {
            1.0
        });
        if tf.scale != scale {
            tf.scale = scale;
        }
    }
}

fn image(w: u32, h: u32, data: Vec<u8>) -> Image {
    Image::new(
        Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    )
}

fn tiled(image: Handle<Image>, color: Color) -> ImageNode {
    ImageNode {
        image,
        color,
        image_mode: NodeImageMode::Tiled {
            tile_x: true,
            tile_y: true,
            stretch_value: 1.0,
        },
        visual_box: VisualBox::BorderBox,
        ..default()
    }
}

/// Faint diagonal hatching (rules beside section titles, empty art frames, placeholder wells):
/// white stripes tinted by `color`, e.g. `Color::srgba(1.0, 1.0, 1.0, 0.05)`.
pub fn hatch(color: Color) -> ImageNode {
    tiled(HATCH, color)
}

/// A map's picture ([`crate::view::PREVIEW`]) filling a `w` x `h` box, cropped around its centre
/// to the box's shape; `tint` multiplies it (white as is).
pub fn art(image: Handle<Image>, w: f32, h: f32, tint: Color) -> ImageNode {
    let full = Vec2::new(crate::view::PREVIEW.0 as f32, crate::view::PREVIEW.1 as f32);
    let keep = if w / h > full.x / full.y {
        Vec2::new(full.x, full.x * h / w)
    } else {
        Vec2::new(full.y * w / h, full.y)
    };
    ImageNode {
        rect: Some(Rect::from_center_size(full / 2.0, keep)),
        color: tint,
        ..ImageNode::new(image)
    }
}

/// The orange and black hazard band (top edge of the screen, the SELECTED flag).
pub fn hazard() -> ImageNode {
    tiled(HAZARD, Color::WHITE)
}

/// Marks the selected button of a group (filled with the accent).
#[derive(Component)]
pub struct Chosen;

/// A button [`hover`] paints; the main call to action (START, RESUME) is [`Primary`].
#[derive(Component)]
pub struct Flat;

#[derive(Component)]
pub struct Primary;

/// A text that takes its [`Flat`] parent's label colour: `ON_ACCENT` on the accent, `TEXT`
/// (`INK` hovered) otherwise.
#[derive(Component)]
pub struct OnFill;

/// The look every flat button starts with (`hover` repaints it by state): a [`BUTTON`] tile in
/// the `Slant(8)` shape.
pub fn flat(w: f32, h: f32) -> impl Bundle {
    flat_cut(w, h, Cut::Slant(8))
}

/// [`flat`] in another shape.
pub fn flat_cut(w: f32, h: f32, cut: Cut) -> impl Bundle {
    (
        Button,
        Flat,
        UiTransform::default(),
        Node {
            width: px(w),
            height: px(h),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        shape(cut, BUTTON),
    )
}

/// A clickable button; `act` is the system-specific action component.
pub fn button<A: Component>(w: f32, h: f32, label: &str, size: f32, act: A) -> impl Bundle {
    (
        act,
        flat(w, h),
        children![(
            Text::new(label),
            strong(size),
            TextColor(TEXT),
            tracking(0.06, size),
            OnFill
        )],
    )
}

/// The main button of a screen: filled with the accent, its label in display capitals.
pub fn primary<A: Component>(w: f32, h: f32, label: &str, size: f32, act: A) -> impl Bundle {
    (
        act,
        Primary,
        flat_cut(w, h, Cut::Notch(14)),
        children![(
            Text::new(label.to_uppercase()),
            display(size * 1.2),
            TextColor(ON_ACCENT),
            tracking(0.04, size * 1.2),
            OnFill
        )],
    )
}

/// Hover, pressed and selected look of every [`Flat`] button and its [`OnFill`] labels.
#[allow(clippy::type_complexity)]
pub fn hover(
    mut buttons: Query<
        (
            &Interaction,
            Has<Chosen>,
            Has<Primary>,
            &mut BackgroundColor,
            Option<&mut ImageNode>,
            &mut UiTransform,
            Option<&Children>,
        ),
        With<Flat>,
    >,
    mut labels: Query<&mut TextColor, With<OnFill>>,
) {
    let bright = |c: Color, by: f32| c.mix(&Color::WHITE, by);
    for (i, chosen, primary, mut bg, image, mut tf, children) in &mut buttons {
        let lit = primary || chosen;
        let fill = match (*i, lit) {
            (Interaction::Pressed, true) => ACCENT.mix(&Color::BLACK, 0.15),
            (Interaction::Hovered, true) => bright(ACCENT, 0.15),
            (_, true) => ACCENT,
            (Interaction::Pressed, false) => bright(BUTTON, 0.12),
            (Interaction::Hovered, false) => bright(BUTTON, 0.07),
            _ => BUTTON,
        };
        match image {
            Some(mut image) => {
                if image.color != fill {
                    image.color = fill;
                }
                bg.set_if_neq(BackgroundColor(Color::NONE));
            }
            None => {
                bg.set_if_neq(BackgroundColor(fill));
            }
        }
        let label = match (*i, lit) {
            (_, true) => ON_ACCENT,
            (Interaction::None, false) => TEXT,
            _ => INK,
        };
        for child in children.into_iter().flatten() {
            if let Ok(mut c) = labels.get_mut(*child) {
                c.set_if_neq(TextColor(label));
            }
        }
        let scale = Vec2::splat(if *i == Interaction::Pressed {
            0.97
        } else {
            1.0
        });
        if tf.scale != scale {
            tf.scale = scale;
        }
    }
}

/// A dark panel in the `Chamfer(18)` shape.
pub fn panel(width: f32, align: AlignItems) -> impl Bundle {
    (
        Node {
            width: px(width),
            align_items: align,
            flex_direction: FlexDirection::Column,
            row_gap: px(10),
            padding: UiRect::all(px(22)),
            ..default()
        },
        shape(Cut::Chamfer(18), SURFACE),
    )
}

/// A small panel title: display capitals.
pub fn heading(s: &str) -> impl Bundle {
    (
        Text::new(s.to_uppercase()),
        display(24.0),
        TextColor(INK),
        tracking(0.02, 24.0),
        Node {
            width: percent(100),
            margin: UiRect::bottom(px(4)),
            ..default()
        },
    )
}

/// A section title as in the mock's "01 MAP": the accent mono index, display capitals, then a
/// hatched rule that fills the row. Add right-hand items (counts, a RANDOM button) as further
/// children of the returned row.
pub fn section(index: u32, title: &str) -> impl Bundle {
    (
        Node {
            width: percent(100),
            align_items: AlignItems::Center,
            column_gap: px(12),
            margin: UiRect::bottom(px(6)),
            ..default()
        },
        children![
            (
                Text::new(format!("{index:02}")),
                mono(13.0),
                TextColor(ACCENT)
            ),
            (
                Text::new(title.to_uppercase()),
                display(30.0),
                TextColor(INK),
                tracking(0.02, 30.0)
            ),
            (
                Node {
                    flex_grow: 1.0,
                    height: px(6),
                    margin: UiRect::horizontal(px(8)),
                    ..default()
                },
                hatch(Color::srgba(1.0, 1.0, 1.0, 0.09)),
            ),
        ],
    )
}

/// A caption: letter-spaced mono capitals in [`DIM`] (field labels, column heads).
pub fn caption(s: &str, size: f32) -> impl Bundle {
    (
        Text::new(s.to_uppercase()),
        mono(size),
        TextColor(DIM),
        tracking(0.14, size),
    )
}

/// A key cap (ESC, F1, ENTER): mono capitals in a hairline box.
pub fn key(s: &str) -> impl Bundle {
    (
        Node {
            padding: UiRect::axes(px(5), px(1)),
            border: UiRect::all(px(1)),
            border_radius: BorderRadius::all(px(2)),
            ..default()
        },
        BorderColor::all(MUTE),
        children![(Text::new(s.to_uppercase()), mono(10.0), TextColor(DIM))],
    )
}

/// A segment bar: [`paint_segs`] lights the first `lit` of its children in the accent, the rest
/// in [`EDGE`]. Spawn it with [`segs`].
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub struct Segs {
    pub lit: usize,
}

/// A row of `n` segments `h` px tall, `gap` px apart, filling the row's width; `lit` start lit.
pub fn segs(n: usize, lit: usize, h: f32, gap: f32) -> impl Bundle {
    (
        Segs { lit },
        Node {
            width: percent(100),
            height: px(h),
            column_gap: px(gap),
            ..default()
        },
        Children::spawn(SpawnIter((0..n).map(move |i| {
            (
                Node {
                    flex_grow: 1.0,
                    height: percent(100),
                    ..default()
                },
                BackgroundColor(if i < lit { ACCENT } else { EDGE }),
            )
        }))),
    )
}

/// Paints [`Segs`] bars whose count changed.
pub fn paint_segs(
    bars: Query<(&Segs, &Children), Changed<Segs>>,
    mut fills: Query<&mut BackgroundColor>,
) {
    for (segs, children) in &bars {
        for (i, child) in children.iter().enumerate() {
            if let Ok(mut bg) = fills.get_mut(child) {
                bg.set_if_neq(BackgroundColor(if i < segs.lit { ACCENT } else { EDGE }));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_hover_and_click() {
        use Interaction::{Hovered, None, Pressed};
        assert_eq!(feel(None, Hovered), Some(cue::OVER));
        assert_eq!(feel(Hovered, Pressed), Some(cue::CLICK));
        assert_eq!(feel(None, Pressed), Some(cue::CLICK));
        // a release falls back to hover without a second hover sound
        assert_eq!(feel(Pressed, Hovered), std::option::Option::None);
        assert_eq!(feel(Hovered, None), std::option::Option::None);
    }

    /// Every interface stem is in the extract (when there is one) and decodes.
    #[test]
    fn interface_stems_decode() {
        let dir = std::path::Path::new(".local/extract/sound/effect");
        if !dir.exists() {
            return;
        }
        for stem in cue::ALL {
            let bytes: Arc<[u8]> = std::fs::read(dir.join(format!("{stem}.wav")))
                .unwrap_or_else(|e| panic!("{stem}: {e}"))
                .into();
            assert!(crate::music::decode(&bytes, None).is_some(), "{stem}");
        }
    }

    fn alpha(img: &Image, x: usize, y: usize) -> u8 {
        let w = img.width() as usize;
        img.data.as_ref().expect("mask data")[(y * w + x) * 4 + 3]
    }

    /// Each mask is opaque in its middle, clear in the corners its shape cuts and solid in the
    /// others (the slant's edge runs through its top-right texel, which is half covered).
    #[test]
    fn masks_cut_their_corners() {
        for (cut, tl, br, tr) in [
            (Cut::Notch(12), 255, 0, 255),
            (Cut::Chamfer(12), 0, 0, 255),
            (Cut::Slant(12), 0, 0, 159),
        ] {
            let m = cut.mask();
            let (w, h) = (m.width() as usize, m.height() as usize);
            assert_eq!(alpha(&m, w / 2, h / 2), 255, "{cut:?} middle");
            assert_eq!(alpha(&m, 0, 0), tl, "{cut:?} top left");
            assert_eq!(alpha(&m, w - 1, h - 1), br, "{cut:?} bottom right");
            assert_eq!(alpha(&m, w - 1, 0), tr, "{cut:?} top right");
        }
    }
}
