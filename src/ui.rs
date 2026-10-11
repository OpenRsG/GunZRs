//! The game's look, shared by the menus and the HUD: colour tokens, the Barlow type
//! (`assets/fonts`, SIL Open Font License, see `CREDITS.md`) and the widgets every screen builds
//! from (buttons, panels, headings). UI colours and fonts come from here, nowhere else.

use bevy::{asset::uuid_handle, prelude::*, text::FontFeatureTag};

/// Primary text: warm off-white.
pub const INK: Color = Color::srgb(0.94, 0.93, 0.9);
/// Secondary text and captions.
pub const DIM: Color = Color::srgb(0.62, 0.66, 0.72);
/// The accent (amber, from the retail headings): selection, the main action, highlights.
pub const ACCENT: Color = Color::srgb(1.0, 0.78, 0.3);
/// Text and icons drawn on the accent.
pub const ON_ACCENT: Color = Color::srgb(0.09, 0.07, 0.02);
/// Damage, death, low health, errors.
pub const DANGER: Color = Color::srgb(0.95, 0.27, 0.22);
/// Healing and armour, confirmations.
pub const OK: Color = Color::srgb(0.45, 0.85, 0.5);
/// Team colours (names, scores, bars).
pub const RED: Color = Color::srgb(1.0, 0.4, 0.34);
pub const BLUE: Color = Color::srgb(0.4, 0.68, 1.0);
/// Panel fill: near-black, cool, translucent over the game or the lobby art.
pub const SURFACE: Color = Color::srgba(0.025, 0.03, 0.04, 0.86);
/// Fill of a control or a row on a panel.
pub const RAISED: Color = Color::srgba(1.0, 1.0, 1.0, 0.035);
/// HUD slab under text that must stay legible over the game (feed rows, score plate).
pub const PLATE: Color = Color::srgba(0.02, 0.025, 0.03, 0.6);
/// Hairlines: separators and outlines.
pub const LINE: Color = Color::srgba(1.0, 1.0, 1.0, 0.1);
/// Dims the game behind a full-screen overlay (pause, death).
pub const SCRIM: Color = Color::srgba(0.0, 0.0, 0.0, 0.55);
/// Drop shadows (panels, text over the 3D view).
pub const SHADOW: Color = Color::srgba(0.0, 0.0, 0.0, 0.65);

/// Barlow Condensed SemiBold, Bold and Medium; body text is Barlow Medium on the default handle.
const STRONG: Handle<Font> = uuid_handle!("cf47e716-9480-4657-8089-90179f7a64ad");
const DISPLAY: Handle<Font> = uuid_handle!("ccd195c1-da71-4241-9e1e-e63f2be22b12");
const CONDENSED: Handle<Font> = uuid_handle!("f90dad06-53ff-46cd-90e2-ba4718c3676f");

/// Installs the fonts. Barlow Medium replaces Bevy's default font, so any text spawned with
/// `TextFont::from_font_size` reads as body text.
pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        let mut fonts = app.world_mut().resource_mut::<Assets<Font>>();
        for (id, bytes) in [
            (
                AssetId::default(),
                &include_bytes!("../assets/fonts/Barlow-Medium.ttf")[..],
            ),
            (
                STRONG.id(),
                include_bytes!("../assets/fonts/BarlowCondensed-SemiBold.ttf"),
            ),
            (
                DISPLAY.id(),
                include_bytes!("../assets/fonts/BarlowCondensed-Bold.ttf"),
            ),
            (
                CONDENSED.id(),
                include_bytes!("../assets/fonts/BarlowCondensed-Medium.ttf"),
            ),
        ] {
            fonts
                .insert(id, Font::from_bytes(bytes.to_vec()))
                .expect("font asset slot");
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

/// Compact labels and list rows (condensed medium).
pub fn compact(size: f32) -> TextFont {
    TextFont::from_font_size(size).with_font(CONDENSED)
}

/// Buttons, names, tags (condensed semibold).
pub fn strong(size: f32) -> TextFont {
    TextFont::from_font_size(size).with_font(STRONG)
}

/// Titles and banners (condensed bold).
pub fn display(size: f32) -> TextFont {
    TextFont::from_font_size(size).with_font(DISPLAY)
}

/// Numbers that change in place (clock, ammo, health, scores): condensed bold with tabular
/// figures, so a value does not jitter as its digits change.
pub fn digits(size: f32) -> TextFont {
    TextFont {
        font_features: [FontFeatureTag::TABULAR_FIGURES].into(),
        ..display(size)
    }
}

/// The shadow that keeps text readable over the 3D view.
pub fn shadow() -> TextShadow {
    TextShadow {
        offset: Vec2::new(0.0, 1.5),
        color: SHADOW,
    }
}

/// Marks the selected button of a group (drawn with the accent).
#[derive(Component)]
pub struct Chosen;

/// A button [`hover`] paints; the main call to action (START, RESUME) is [`Primary`].
#[derive(Component)]
pub struct Flat;

#[derive(Component)]
pub struct Primary;

/// The look every flat button starts with (`hover` repaints it by state): a square tile with
/// a 2 px bar under it that lights up on hover and selection.
pub fn flat(w: f32, h: f32) -> impl Bundle {
    (
        Button,
        Flat,
        UiTransform::default(),
        Node {
            width: px(w),
            height: px(h),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            border: UiRect::bottom(px(2)),
            border_radius: BorderRadius::all(px(2)),
            ..default()
        },
        BackgroundColor(RAISED),
        BorderColor::all(Color::NONE),
    )
}

/// A clickable button; `act` is the system-specific action component.
pub fn button<A: Component>(w: f32, h: f32, label: &str, size: f32, act: A) -> impl Bundle {
    (
        act,
        flat(w, h),
        children![(Text::new(label), strong(size), TextColor(INK))],
    )
}

/// The main button of a screen: filled with the accent, its label in bold capitals.
pub fn primary<A: Component>(w: f32, h: f32, label: &str, size: f32, act: A) -> impl Bundle {
    (
        act,
        Primary,
        flat(w, h),
        BoxShadow::new(SHADOW, px(0), px(4), px(0), px(12)),
        children![(
            Text::new(label.to_uppercase()),
            display(size * 1.1),
            TextColor(ON_ACCENT)
        )],
    )
}

/// Hover, pressed and selected look of every [`Flat`] button.
#[allow(clippy::type_complexity)]
pub fn hover(
    mut buttons: Query<
        (
            &Interaction,
            Has<Chosen>,
            Has<Primary>,
            &mut BackgroundColor,
            &mut BorderColor,
            &mut UiTransform,
        ),
        With<Flat>,
    >,
) {
    let bright = |c: Color, by: f32| c.mix(&Color::WHITE, by);
    for (i, chosen, primary, mut bg, mut border, mut tf) in &mut buttons {
        let (fill, bar) = match (primary, *i, chosen) {
            (true, Interaction::Pressed, _) => (ACCENT.mix(&Color::BLACK, 0.15), INK),
            (true, Interaction::Hovered, _) => (bright(ACCENT, 0.25), INK),
            (true, _, _) => (ACCENT, Color::NONE),
            (_, Interaction::Pressed, _) => (ACCENT.with_alpha(0.3), ACCENT),
            (_, Interaction::Hovered, true) => (ACCENT.with_alpha(0.26), ACCENT),
            (_, _, true) => (ACCENT.with_alpha(0.16), ACCENT),
            (_, Interaction::Hovered, _) => (Color::srgba(1.0, 1.0, 1.0, 0.13), INK),
            _ => (RAISED, Color::NONE),
        };
        bg.set_if_neq(BackgroundColor(fill));
        border.set_if_neq(BorderColor::all(bar));
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

/// A dark panel lifted off the background by its shadow.
pub fn panel(width: f32, align: AlignItems) -> impl Bundle {
    (
        Node {
            width: px(width),
            align_items: align,
            flex_direction: FlexDirection::Column,
            row_gap: px(8),
            padding: UiRect::all(px(16)),
            border_radius: BorderRadius::all(px(3)),
            ..default()
        },
        BackgroundColor(SURFACE),
        BoxShadow::new(SHADOW, px(0), px(10), px(0), px(28)),
    )
}

/// A panel title: bold accent capitals over a hairline.
pub fn heading(s: &str) -> impl Bundle {
    (
        Text::new(s.to_uppercase()),
        display(20.0),
        TextColor(ACCENT),
        Node {
            width: percent(100),
            padding: UiRect::bottom(px(6)),
            margin: UiRect::bottom(px(4)),
            border: UiRect::bottom(px(1)),
            ..default()
        },
        BorderColor::all(LINE),
    )
}
