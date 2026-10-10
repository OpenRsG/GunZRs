//! Mouse and keyboard controls, saved in the profile ([`Controls`]): rebindable actions (two
//! bindings each: keys, mouse buttons, wheel) named and defaulted after the retail `config.xml`
//! `<KEYBOARD>` list, mouse sensitivity, vertical ratio, invert, acceleration and raw input,
//! and the CONTROLS panel the main menu (a page) and the pause menu (an overlay) share. Profile
//! keys: `docs/formats.md` "Controls".

use crate::{
    menu::{button, heading, panel, primary},
    profile::Profile,
};
use bevy::{
    ecs::system::SystemParam,
    input::{
        InputSystems,
        mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll},
    },
    prelude::*,
    ui::{RelativeCursorPosition, UiSystems},
};
use std::{f32::consts::TAU, sync::LazyLock};

/// Look speed at sensitivity 1, radians per mouse count (8 cm per turn at 800 DPI).
pub const LOOK: f32 = 0.0025;
/// Extra gain per count/ms of mouse speed at acceleration 1 (100 %).
const ACCEL_RATE: f32 = 0.05;
/// Ranges of the mouse settings.
const SENS: (f32, f32) = (0.1, 5.0);
const VERTICAL: (f32, f32) = (0.5, 2.0);
const ACCEL: (f32, f32) = (0.0, 1.0);
const CAP: (f32, f32) = (1.0, 4.0);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bind {
    Key(KeyCode),
    Mouse(MouseButton),
    WheelUp,
    WheelDown,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Forward,
    Back,
    Left,
    Right,
    Jump,
    Dash,
    Fire,
    Guard,
    Reload,
    PrevWeapon,
    NextWeapon,
    Melee,
    Primary,
    Secondary,
    Item1,
    Item2,
    Score,
    Taunt,
    Bow,
    Wave,
    Laugh,
    Cry,
    Dance,
    SensDown,
    SensUp,
}

const N: usize = 25;

impl Action {
    pub const ALL: [Action; N] = [
        Action::Forward,
        Action::Back,
        Action::Left,
        Action::Right,
        Action::Jump,
        Action::Dash,
        Action::Fire,
        Action::Guard,
        Action::Reload,
        Action::PrevWeapon,
        Action::NextWeapon,
        Action::Melee,
        Action::Primary,
        Action::Secondary,
        Action::Item1,
        Action::Item2,
        Action::Score,
        Action::Taunt,
        Action::Bow,
        Action::Wave,
        Action::Laugh,
        Action::Cry,
        Action::Dance,
        Action::SensDown,
        Action::SensUp,
    ];

    pub fn label(self) -> &'static str {
        TABLE[self as usize].1
    }
}

const fn k(c: KeyCode) -> Option<Bind> {
    Some(Bind::Key(c))
}

const fn m(b: MouseButton) -> Option<Bind> {
    Some(Bind::Mouse(b))
}

/// Profile key, label and default bindings of each [`Action`], in its order. The defaults are
/// the retail `config.xml` ones (DirectInput scan codes; its alternates 256..259 are **inferred**
/// to be wheel up, wheel down, left and right button: USEWEAPON 29/258, USEWEAPON2 259,
/// PREVOUSWEAPON 16/256, NEXTWEAPON 18/257). USEWEAPON2 and DEFENCE are one guard here. Dash
/// (retail AUTODASH_LEFT/RIGHT, unbound there) tumbles the held direction, forward if none;
/// T and F5..F9 stay as the second taunt/emote keys this port had before rebinding.
const TABLE: [(&str, &str, [Option<Bind>; 2]); N] = [
    ("forward", "Forward", [k(KeyCode::KeyW), None]),
    ("back", "Back", [k(KeyCode::KeyS), None]),
    ("left", "Left", [k(KeyCode::KeyA), None]),
    ("right", "Right", [k(KeyCode::KeyD), None]),
    ("jump", "Jump", [k(KeyCode::Space), None]),
    ("dash", "Dash", [k(KeyCode::KeyC), None]),
    (
        "fire",
        "Fire",
        [m(MouseButton::Left), k(KeyCode::ControlLeft)],
    ),
    (
        "guard",
        "Guard",
        [m(MouseButton::Right), k(KeyCode::ShiftLeft)],
    ),
    ("reload", "Reload", [k(KeyCode::KeyR), None]),
    (
        "prev_weapon",
        "Prev. weapon",
        [k(KeyCode::KeyQ), Some(Bind::WheelUp)],
    ),
    (
        "next_weapon",
        "Next weapon",
        [k(KeyCode::KeyE), Some(Bind::WheelDown)],
    ),
    ("melee", "Melee", [k(KeyCode::Digit1), None]),
    ("primary", "Primary", [k(KeyCode::Digit2), None]),
    ("secondary", "Secondary", [k(KeyCode::Digit3), None]),
    ("item1", "Item 1", [k(KeyCode::Digit4), None]),
    ("item2", "Item 2", [k(KeyCode::Digit5), None]),
    ("score", "Scoreboard", [k(KeyCode::Tab), None]),
    ("taunt", "Taunt", [k(KeyCode::Numpad1), k(KeyCode::KeyT)]),
    ("bow", "Bow", [k(KeyCode::Numpad2), k(KeyCode::F5)]),
    ("wave", "Wave", [k(KeyCode::Numpad3), k(KeyCode::F6)]),
    ("laugh", "Laugh", [k(KeyCode::Numpad4), k(KeyCode::F8)]),
    ("cry", "Cry", [k(KeyCode::Numpad5), k(KeyCode::F7)]),
    ("dance", "Dance", [k(KeyCode::Numpad6), k(KeyCode::F9)]),
    ("sens_down", "Sens. down", [k(KeyCode::BracketLeft), None]),
    ("sens_up", "Sens. up", [k(KeyCode::BracketRight), None]),
];

/// The panel's columns.
const GROUPS: [(&str, &[Action]); 3] = [
    (
        "MOVE",
        &[
            Action::Forward,
            Action::Back,
            Action::Left,
            Action::Right,
            Action::Jump,
            Action::Dash,
        ],
    ),
    (
        "FIGHT",
        &[
            Action::Fire,
            Action::Guard,
            Action::Reload,
            Action::PrevWeapon,
            Action::NextWeapon,
            Action::Melee,
            Action::Primary,
            Action::Secondary,
            Action::Item1,
            Action::Item2,
        ],
    ),
    (
        "MORE",
        &[
            Action::Score,
            Action::Taunt,
            Action::Bow,
            Action::Wave,
            Action::Laugh,
            Action::Cry,
            Action::Dance,
            Action::SensDown,
            Action::SensUp,
        ],
    ),
];

/// Keys a binding can take. Esc stays the pause key; Delete and Backspace clear a binding.
const KEYS: [KeyCode; 95] = {
    use KeyCode::*;
    [
        KeyA,
        KeyB,
        KeyC,
        KeyD,
        KeyE,
        KeyF,
        KeyG,
        KeyH,
        KeyI,
        KeyJ,
        KeyK,
        KeyL,
        KeyM,
        KeyN,
        KeyO,
        KeyP,
        KeyQ,
        KeyR,
        KeyS,
        KeyT,
        KeyU,
        KeyV,
        KeyW,
        KeyX,
        KeyY,
        KeyZ,
        Digit0,
        Digit1,
        Digit2,
        Digit3,
        Digit4,
        Digit5,
        Digit6,
        Digit7,
        Digit8,
        Digit9,
        F1,
        F2,
        F3,
        F4,
        F5,
        F6,
        F7,
        F8,
        F9,
        F10,
        F11,
        F12,
        Numpad0,
        Numpad1,
        Numpad2,
        Numpad3,
        Numpad4,
        Numpad5,
        Numpad6,
        Numpad7,
        Numpad8,
        Numpad9,
        NumpadAdd,
        NumpadSubtract,
        NumpadMultiply,
        NumpadDivide,
        NumpadDecimal,
        NumpadEnter,
        Space,
        Tab,
        Enter,
        ShiftLeft,
        ShiftRight,
        ControlLeft,
        ControlRight,
        AltLeft,
        AltRight,
        CapsLock,
        ArrowUp,
        ArrowDown,
        ArrowLeft,
        ArrowRight,
        Insert,
        Home,
        End,
        PageUp,
        PageDown,
        Backquote,
        Minus,
        Equal,
        BracketLeft,
        BracketRight,
        Backslash,
        Semicolon,
        Quote,
        Comma,
        Period,
        Slash,
        IntlBackslash,
    ]
};
const BUTTONS: [(MouseButton, &str); 5] = [
    (MouseButton::Left, "MouseLeft"),
    (MouseButton::Right, "MouseRight"),
    (MouseButton::Middle, "MouseMiddle"),
    (MouseButton::Back, "MouseBack"),
    (MouseButton::Forward, "MouseForward"),
];

/// Profile text of a binding (`-` = none).
fn token(b: Option<Bind>) -> String {
    match b {
        None => "-".into(),
        Some(Bind::Key(k)) => format!("{k:?}"),
        Some(Bind::Mouse(b)) => BUTTONS
            .iter()
            .find(|x| x.0 == b)
            .map_or("-", |x| x.1)
            .into(),
        Some(Bind::WheelUp) => "WheelUp".into(),
        Some(Bind::WheelDown) => "WheelDown".into(),
    }
}

fn parse_token(s: &str) -> Option<Option<Bind>> {
    match s.trim() {
        "-" => Some(None),
        "WheelUp" => Some(Some(Bind::WheelUp)),
        "WheelDown" => Some(Some(Bind::WheelDown)),
        s => BUTTONS
            .iter()
            .find(|x| x.1 == s)
            .map(|x| Bind::Mouse(x.0))
            .or_else(|| {
                KEYS.iter()
                    .find(|k| format!("{k:?}") == s)
                    .map(|k| Bind::Key(*k))
            })
            .map(Some),
    }
}

/// On-screen name of a binding.
pub fn label(b: Bind) -> String {
    let key = match b {
        Bind::Mouse(b) => {
            let n = BUTTONS.iter().position(|x| x.0 == b).map_or(0, |i| i + 1);
            return format!("MOUSE {n}");
        }
        Bind::WheelUp => return "WHEEL UP".into(),
        Bind::WheelDown => return "WHEEL DN".into(),
        Bind::Key(k) => format!("{k:?}"),
    };
    let fixed = match key.as_str() {
        "ShiftLeft" => "L-SHIFT",
        "ShiftRight" => "R-SHIFT",
        "ControlLeft" => "L-CTRL",
        "ControlRight" => "R-CTRL",
        "AltLeft" => "L-ALT",
        "AltRight" => "R-ALT",
        "CapsLock" => "CAPS",
        "PageUp" => "PG UP",
        "PageDown" => "PG DN",
        "Insert" => "INS",
        "Backquote" => "`",
        "Minus" => "-",
        "Equal" => "=",
        "BracketLeft" => "[",
        "BracketRight" => "]",
        "Backslash" => "\\",
        "IntlBackslash" => "ISO \\",
        "Semicolon" => ";",
        "Quote" => "'",
        "Comma" => ",",
        "Period" => ".",
        "Slash" => "/",
        "NumpadAdd" => "NUM +",
        "NumpadSubtract" => "NUM -",
        "NumpadMultiply" => "NUM *",
        "NumpadDivide" => "NUM /",
        "NumpadDecimal" => "NUM .",
        "NumpadEnter" => "NUM ENT",
        _ => "",
    };
    if !fixed.is_empty() {
        return fixed.into();
    }
    let short = ["Key", "Digit", "Arrow"]
        .iter()
        .find_map(|p| key.strip_prefix(p))
        .map_or_else(|| key.replace("Numpad", "NUM "), str::to_owned);
    short.to_uppercase()
}

/// The player's mouse and keyboard settings (`Profile::controls`).
#[derive(Clone, Debug, PartialEq)]
pub struct Controls {
    /// Look speed, times [`LOOK`].
    pub sens: f32,
    /// Vertical look speed relative to horizontal.
    pub vertical: f32,
    pub invert: bool,
    /// Acceleration strength 0..=1: the gain grows with mouse speed ([`Controls::gain`]).
    pub accel: f32,
    /// Highest acceleration gain.
    pub accel_cap: f32,
    /// Browser build: unaccelerated pointer-lock movement (`unadjustedMovement`). The desktop
    /// build always reads the raw device motion.
    pub raw: bool,
    pub binds: [[Option<Bind>; 2]; N],
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            sens: 1.0,
            vertical: 1.0,
            invert: false,
            accel: 0.0,
            accel_cap: 2.0,
            raw: true,
            binds: TABLE.map(|t| t.2),
        }
    }
}

/// For systems that run without a profile.
pub static DEFAULTS: LazyLock<Controls> = LazyLock::new(Controls::default);

fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

impl Controls {
    /// Sensitivity gain at a mouse speed in counts per millisecond: 1 without acceleration,
    /// growing linearly with speed up to the cap.
    pub fn gain(&self, speed: f32) -> f32 {
        (1.0 + self.accel * ACCEL_RATE * speed).min(self.accel_cap.max(1.0))
    }

    /// The look turn (yaw, pitch radians to subtract) for this frame's mouse `delta` in counts
    /// over `dt` seconds.
    pub fn look(&self, delta: Vec2, dt: f32) -> Vec2 {
        let speed = delta.length() / (dt * 1000.0).max(1.0);
        let y = self.vertical * if self.invert { -1.0 } else { 1.0 };
        Vec2::new(delta.x, delta.y * y) * LOOK * self.sens * self.gain(speed)
    }

    /// `steps` of 0.05 on the sensitivity (the Sens. up/down keys).
    pub fn nudge_sens(&mut self, steps: i32) {
        self.sens = round2(self.sens + 0.05 * steps as f32).clamp(SENS.0, SENS.1);
    }

    /// Binds slot `slot` of `a` to `b`, taking `b` off whatever had it; returns that action.
    pub fn bind(&mut self, a: Action, slot: usize, b: Option<Bind>) -> Option<Action> {
        let mut from = None;
        if b.is_some() {
            for (i, pair) in self.binds.iter_mut().enumerate() {
                for s in pair.iter_mut().filter(|s| **s == b) {
                    *s = None;
                    from = Some(Action::ALL[i]);
                }
            }
        }
        self.binds[a as usize][slot] = b;
        from.filter(|&f| f != a)
    }

    /// Profile lines (`mouse_*`, `bind_NAME=FIRST,SECOND`).
    pub fn to_text(&self) -> String {
        let mut t = format!(
            "mouse_sens={}\nmouse_vertical={}\nmouse_invert={}\nmouse_accel={}\nmouse_accel_cap={}\nmouse_raw={}\n",
            self.sens, self.vertical, self.invert as u8, self.accel, self.accel_cap, self.raw as u8
        );
        for (row, b) in TABLE.iter().zip(&self.binds) {
            t += &format!("bind_{}={},{}\n", row.0, token(b[0]), token(b[1]));
        }
        t
    }

    /// Takes one profile line; `Ok(false)`: not a controls key.
    pub fn set(&mut self, k: &str, v: &str) -> Result<bool, String> {
        let bad = || format!("{k}: bad value {v:?}");
        let num = |r: (f32, f32)| {
            v.trim()
                .parse::<f32>()
                .ok()
                .filter(|x| x.is_finite())
                .map(|x| x.clamp(r.0, r.1))
                .ok_or_else(bad)
        };
        let flag = || match v.trim() {
            "0" => Ok(false),
            "1" => Ok(true),
            _ => Err(bad()),
        };
        match k {
            "mouse_sens" => self.sens = num(SENS)?,
            "mouse_vertical" => self.vertical = num(VERTICAL)?,
            "mouse_invert" => self.invert = flag()?,
            "mouse_accel" => self.accel = num(ACCEL)?,
            "mouse_accel_cap" => self.accel_cap = num(CAP)?,
            "mouse_raw" => self.raw = flag()?,
            _ => {
                let Some(i) = k
                    .strip_prefix("bind_")
                    .and_then(|n| TABLE.iter().position(|t| t.0 == n))
                else {
                    return Ok(false);
                };
                let mut parts = v.split(',');
                for s in 0..2 {
                    self.binds[i][s] = parse_token(parts.next().unwrap_or("-")).ok_or_else(bad)?;
                }
            }
        }
        Ok(true)
    }
}

/// Actions held by the browser's on-screen touch controls (`web.rs`) or a `--script`, on top of
/// the bindings: `now` this frame, `before` the frame before (for presses).
#[derive(Resource, Default)]
pub struct Pad {
    pub now: [bool; N],
    pub before: [bool; N],
}

/// Reads actions through a [`Controls`]' bindings.
pub struct Reader<'a> {
    pub keys: &'a ButtonInput<KeyCode>,
    pub mouse: &'a ButtonInput<MouseButton>,
    /// This frame's wheel (lines or pixels; only the sign counts).
    pub wheel: f32,
    pub pad: &'a Pad,
    pub controls: &'a Controls,
}

impl Reader<'_> {
    fn on(&self, b: Bind, just: bool) -> bool {
        match b {
            Bind::Key(k) if just => self.keys.just_pressed(k),
            Bind::Key(k) => self.keys.pressed(k),
            Bind::Mouse(b) if just => self.mouse.just_pressed(b),
            Bind::Mouse(b) => self.mouse.pressed(b),
            Bind::WheelUp => self.wheel > 0.0,
            Bind::WheelDown => self.wheel < 0.0,
        }
    }

    fn any(&self, a: Action, just: bool) -> bool {
        self.controls.binds[a as usize]
            .iter()
            .flatten()
            .any(|&b| self.on(b, just))
    }

    pub fn pressed(&self, a: Action) -> bool {
        self.any(a, false) || self.pad.now[a as usize]
    }

    /// Pressed this frame (a wheel binding: turned this frame).
    pub fn just(&self, a: Action) -> bool {
        let i = a as usize;
        self.any(a, true) || (self.pad.now[i] && !self.pad.before[i])
    }
}

/// [`Reader`] with the profile's controls, for systems that only read.
#[derive(SystemParam)]
pub struct Input<'w> {
    keys: Res<'w, ButtonInput<KeyCode>>,
    mouse: Res<'w, ButtonInput<MouseButton>>,
    scroll: Res<'w, AccumulatedMouseScroll>,
    pad: Res<'w, Pad>,
    profile: Option<Res<'w, Profile>>,
}

impl Input<'_> {
    pub fn pressed(&self, a: Action) -> bool {
        Reader {
            keys: &self.keys,
            mouse: &self.mouse,
            wheel: self.scroll.delta.y,
            pad: &self.pad,
            controls: self.profile.as_deref().map_or(&*DEFAULTS, |p| &p.controls),
        }
        .pressed(a)
    }
}

pub struct ControlsPlugin;

impl Plugin for ControlsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Pad>()
            .init_resource::<Panel>()
            .add_systems(
                PreUpdate,
                capture.after(InputSystems).before(UiSystems::Focus),
            )
            .add_systems(Update, (overlay, clicks, drag, refresh, persist).chain());
    }
}

// ---- the CONTROLS panel ----

/// Present while the pause menu's CONTROLS overlay is open.
#[derive(Resource)]
pub struct Overlay;

const HINT: &str =
    "Click a binding, then press a key, mouse button or wheel. Esc cancels, Delete clears.";
const GOLD: Color = Color::srgb(1.0, 0.8, 0.3);
const DIM: Color = Color::srgb(0.72, 0.72, 0.72);
/// Speed-curve bars, one per count/ms.
const BARS: usize = 40;

#[derive(Resource)]
struct Panel {
    /// The binding waiting for an input.
    capture: Option<(Action, usize)>,
    /// The slider being dragged.
    drag: Option<Entity>,
    status: String,
    /// Recent mouse speed (counts/ms) for the curve.
    speed: f32,
}

impl Default for Panel {
    fn default() -> Self {
        Self {
            capture: None,
            drag: None,
            status: HINT.into(),
            speed: 0.0,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Knob {
    Sens,
    Vertical,
    Accel,
    Cap,
}

impl Knob {
    fn range(self) -> (f32, f32) {
        match self {
            Knob::Sens => SENS,
            Knob::Vertical => VERTICAL,
            Knob::Accel => ACCEL,
            Knob::Cap => CAP,
        }
    }

    fn step(self) -> f32 {
        if self == Knob::Cap { 0.1 } else { 0.05 }
    }

    fn get(self, c: &Controls) -> f32 {
        match self {
            Knob::Sens => c.sens,
            Knob::Vertical => c.vertical,
            Knob::Accel => c.accel,
            Knob::Cap => c.accel_cap,
        }
    }

    fn set(self, c: &mut Controls, v: f32) {
        let (lo, hi) = self.range();
        let v = round2(v).clamp(lo, hi);
        *match self {
            Knob::Sens => &mut c.sens,
            Knob::Vertical => &mut c.vertical,
            Knob::Accel => &mut c.accel,
            Knob::Cap => &mut c.accel_cap,
        } = v;
    }

    /// Slider position 0..=1 of a value; sensitivity and vertical ratio on a log scale.
    fn pos(self, v: f32) -> f32 {
        let (lo, hi) = self.range();
        match self {
            Knob::Sens | Knob::Vertical => (v / lo).ln() / (hi / lo).ln(),
            _ => (v - lo) / (hi - lo),
        }
    }

    fn at(self, t: f32) -> f32 {
        let (lo, hi) = self.range();
        match self {
            Knob::Sens | Knob::Vertical => lo * (hi / lo).powf(t),
            _ => lo + (hi - lo) * t,
        }
    }

    fn text(self, v: f32) -> String {
        match self {
            Knob::Accel if v == 0.0 => "OFF".into(),
            Knob::Accel => format!("{:.0}%", v * 100.0),
            Knob::Cap => format!("x{v:.1}"),
            _ => format!("x{v:.2}"),
        }
    }
}

#[derive(Clone, Copy)]
enum Flag {
    Invert,
    Raw,
}

#[derive(Component, Clone, Copy)]
enum Ctl {
    Bind(Action, usize),
    Step(Knob, i8),
    Toggle(Flag),
    Reset,
    Done,
}

#[derive(Component)]
struct Slider(Knob);

#[derive(Component)]
struct Fill(Knob);

#[derive(Component, Clone, Copy)]
enum Show {
    Knob(Knob),
    Turn,
    Status,
}

#[derive(Component)]
struct Bar(usize);

#[derive(Component)]
struct OverlayRoot;

fn text(s: &str, size: f32, color: Color) -> impl Bundle {
    (
        Text::new(s),
        TextFont::from_font_size(size),
        TextColor(color),
    )
}

/// A text in a box of fixed width (a fixed-width text node gets the height of its narrowest
/// wrapping, so the width goes on a parent).
fn boxed(s: &str, size: f32, color: Color, width: f32) -> impl Bundle {
    (
        Node {
            width: px(width),
            ..default()
        },
        children![text(s, size, color)],
    )
}

fn row() -> Node {
    Node {
        align_items: AlignItems::Center,
        column_gap: px(6),
        ..default()
    }
}

/// The panel's contents: mouse settings, then the bindings with the status line, RESET (and
/// DONE for the pause overlay) under them.
pub fn fill(p: &mut ChildSpawnerCommands, done: bool) {
    p.spawn(Node {
        column_gap: px(16),
        align_items: AlignItems::Stretch,
        ..default()
    })
    .with_children(|r| {
        r.spawn(panel(410.0, AlignItems::FlexStart))
            .with_children(mouse);
        r.spawn(panel(800.0, AlignItems::FlexStart))
            .with_children(|k| {
                binds(k);
                k.spawn(Node {
                    margin: UiRect::top(Val::Auto),
                    width: percent(100),
                    column_gap: px(12),
                    align_items: AlignItems::Center,
                    ..default()
                })
                .with_children(|f| {
                    f.spawn((
                        Node {
                            flex_grow: 1.0,
                            flex_basis: px(0),
                            ..default()
                        },
                        children![(Show::Status, text(HINT, 14.0, DIM))],
                    ));
                    f.spawn(button(170.0, 34.0, "RESET DEFAULTS", 15.0, Ctl::Reset));
                    if done {
                        f.spawn(primary(130.0, 34.0, "DONE", 18.0, Ctl::Done));
                    }
                });
            });
    });
}

fn mouse(m: &mut ChildSpawnerCommands) {
    m.spawn(heading("MOUSE"));
    let slider = |m: &mut ChildSpawnerCommands, name: &str, k: Knob| {
        m.spawn(row()).with_children(|r| {
            r.spawn(boxed(name, 15.0, Color::WHITE, 112.0));
            r.spawn((
                Button,
                Slider(k),
                RelativeCursorPosition::default(),
                Node {
                    width: px(120),
                    height: px(24),
                    flex_direction: FlexDirection::Column,
                    justify_content: JustifyContent::Center,
                    ..default()
                },
            ))
            .with_children(|t| {
                t.spawn((
                    Node {
                        height: px(6),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.15)),
                    children![(
                        Fill(k),
                        Node {
                            height: percent(100),
                            ..default()
                        },
                        BackgroundColor(GOLD),
                    )],
                ));
            });
            r.spawn(button(26.0, 24.0, "-", 16.0, Ctl::Step(k, -1)));
            r.spawn(button(26.0, 24.0, "+", 16.0, Ctl::Step(k, 1)));
            r.spawn((Show::Knob(k), text("", 15.0, GOLD)));
        });
    };
    let toggle = |m: &mut ChildSpawnerCommands, name: &str, f: Flag| {
        m.spawn(row()).with_children(|r| {
            r.spawn(boxed(name, 15.0, Color::WHITE, 296.0));
            r.spawn(button(64.0, 24.0, "", 14.0, Ctl::Toggle(f)));
        });
    };
    slider(m, "Sensitivity", Knob::Sens);
    m.spawn((
        Node {
            margin: UiRect::left(px(118)),
            ..default()
        },
        children![(Show::Turn, text("", 13.0, DIM))],
    ));
    slider(m, "Vertical", Knob::Vertical);
    toggle(m, "Invert vertical look", Flag::Invert);
    slider(m, "Acceleration", Knob::Accel);
    slider(m, "Accel. limit", Knob::Cap);
    if cfg!(target_arch = "wasm32") {
        toggle(m, "Raw input (no OS accel.)", Flag::Raw);
    } else {
        m.spawn(row()).with_children(|r| {
            r.spawn(boxed("Raw input", 15.0, Color::WHITE, 296.0));
            r.spawn(text("ALWAYS", 14.0, GOLD));
        });
    }
    m.spawn(text("SPEED CURVE", 14.0, GOLD));
    m.spawn((
        Node {
            height: px(60),
            align_items: AlignItems::FlexEnd,
            column_gap: px(2),
            border: UiRect::bottom(px(1)),
            ..default()
        },
        BorderColor::all(Color::srgba(1.0, 1.0, 1.0, 0.3)),
    ))
    .with_children(|g| {
        for i in 0..BARS {
            g.spawn((
                Bar(i),
                Node {
                    width: px(7),
                    height: px(6),
                    ..default()
                },
                BackgroundColor(DIM),
            ));
        }
    });
    m.spawn(boxed(
        "Gain by mouse speed, slow to fast; flat = no acceleration. Move the mouse to see your speed light up.",
        13.0,
        DIM,
        370.0,
    ));
}

fn binds(k: &mut ChildSpawnerCommands) {
    k.spawn(heading("KEYS AND BUTTONS"));
    k.spawn(Node {
        column_gap: px(18),
        ..default()
    })
    .with_children(|cols| {
        for (title, actions) in GROUPS {
            cols.spawn(Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(4),
                ..default()
            })
            .with_children(|col| {
                col.spawn(text(title, 14.0, GOLD));
                for &a in actions {
                    col.spawn(row()).with_children(|r| {
                        r.spawn(boxed(a.label(), 13.0, Color::WHITE, 100.0));
                        for slot in 0..2 {
                            r.spawn((
                                Button,
                                Ctl::Bind(a, slot),
                                Node {
                                    width: px(66),
                                    height: px(24),
                                    justify_content: JustifyContent::Center,
                                    align_items: AlignItems::Center,
                                    border: UiRect::all(px(1)),
                                    ..default()
                                },
                                BackgroundColor(Color::NONE),
                                BorderColor::all(Color::NONE),
                                children![text("", 12.0, Color::WHITE)],
                            ));
                        }
                    });
                }
            });
        }
    });
}

/// Takes the input a binding waits for (before the UI sees a click, so the click that binds
/// mouse 1 presses nothing), closes the overlay on Esc, and measures the mouse speed.
#[allow(clippy::too_many_arguments)]
fn capture(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    scroll: Res<AccumulatedMouseScroll>,
    motion: Res<AccumulatedMouseMotion>,
    real: Res<Time<Real>>,
    overlay: Option<Res<Overlay>>,
    mut panel: ResMut<Panel>,
    profile: Option<ResMut<Profile>>,
    mut commands: Commands,
) {
    let dt = real.delta_secs();
    let speed = motion.delta.length() / (dt * 1000.0).max(1.0);
    let p = panel.bypass_change_detection();
    p.speed = speed.max(p.speed - 30.0 * dt).max(0.0);
    let esc = keys.just_pressed(KeyCode::Escape);
    if esc && (panel.capture.is_some() || overlay.is_some()) {
        keys.clear_just_pressed(KeyCode::Escape);
    }
    let Some((a, slot)) = panel.capture else {
        if esc && overlay.is_some() {
            commands.remove_resource::<Overlay>();
        }
        return;
    };
    let (Some(mut profile), false) = (profile, esc) else {
        panel.capture = None;
        panel.status = HINT.into();
        return;
    };
    let got = if keys.any_just_pressed([KeyCode::Delete, KeyCode::Backspace]) {
        Some(None)
    } else if let Some(&k) = KEYS.iter().find(|k| keys.just_pressed(**k)) {
        Some(Some(Bind::Key(k)))
    } else if let Some(&(b, _)) = BUTTONS.iter().find(|b| mouse.just_pressed(b.0)) {
        mouse.clear_just_pressed(b);
        Some(Some(Bind::Mouse(b)))
    } else if scroll.delta.y != 0.0 {
        Some(Some(if scroll.delta.y > 0.0 {
            Bind::WheelUp
        } else {
            Bind::WheelDown
        }))
    } else {
        None
    };
    let Some(b) = got else { return };
    panel.capture = None;
    let from = profile.controls.bind(a, slot, b);
    panel.status = match (b, from) {
        (None, _) => format!("{}: binding cleared.", a.label()),
        (Some(b), Some(f)) => format!("{} moved from {} to {}.", label(b), f.label(), a.label()),
        (Some(b), None) => format!("{}: {}.", a.label(), label(b)),
    };
}

/// Spawns the pause menu's overlay while [`Overlay`] is present and the game is paused.
fn overlay(
    mut commands: Commands,
    open: Option<Res<Overlay>>,
    frozen: Option<Res<crate::game::Frozen>>,
    camera: Query<Entity, With<Camera3d>>,
    roots: Query<Entity, With<OverlayRoot>>,
    mut panel: ResMut<Panel>,
) {
    if open.is_some() && frozen.is_none() {
        commands.remove_resource::<Overlay>();
    }
    match (open.is_some() && frozen.is_some(), roots.single()) {
        (false, Ok(e)) => {
            commands.entity(e).despawn();
            *panel = Panel::default();
        }
        (true, Err(_)) => {
            let Ok(camera) = camera.single() else {
                return;
            };
            *panel = Panel::default();
            commands
                .spawn((
                    OverlayRoot,
                    UiTargetCamera(camera),
                    GlobalZIndex(20),
                    Node {
                        position_type: PositionType::Absolute,
                        width: percent(100),
                        height: percent(100),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    // opaque enough that the HUD under it (death text, notices) does not read through
                    BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.85)),
                ))
                .with_children(|r| fill(r, true));
        }
        _ => {}
    }
}

fn clicks(
    q: Query<(&Interaction, &Ctl), Changed<Interaction>>,
    mut panel: ResMut<Panel>,
    profile: Option<ResMut<Profile>>,
    mut commands: Commands,
) {
    let Some(mut profile) = profile else { return };
    for (i, c) in &q {
        if *i != Interaction::Pressed {
            continue;
        }
        let c2 = &mut profile.controls;
        match *c {
            Ctl::Bind(a, s) => {
                let again = panel.capture == Some((a, s));
                panel.capture = (!again).then_some((a, s));
                panel.status = if again {
                    HINT.into()
                } else {
                    format!(
                        "Press a key, mouse button or wheel for {}. Esc cancels, Delete clears.",
                        a.label()
                    )
                };
            }
            Ctl::Step(k, d) => k.set(c2, k.get(c2) + d as f32 * k.step()),
            Ctl::Toggle(Flag::Invert) => c2.invert = !c2.invert,
            Ctl::Toggle(Flag::Raw) => {
                c2.raw = !c2.raw;
                panel.status = "Raw input applies the next time the mouse is captured.".into();
            }
            Ctl::Reset => {
                *c2 = Controls::default();
                panel.status = "Default controls restored.".into();
            }
            Ctl::Done => commands.remove_resource::<Overlay>(),
        }
    }
}

fn drag(
    mouse: Res<ButtonInput<MouseButton>>,
    sliders: Query<(Entity, &Interaction, &Slider, &RelativeCursorPosition)>,
    mut panel: ResMut<Panel>,
    profile: Option<ResMut<Profile>>,
) {
    if !mouse.pressed(MouseButton::Left) {
        if panel.drag.is_some() {
            panel.drag = None;
        }
        return;
    }
    if panel.drag.is_none() {
        panel.drag = sliders
            .iter()
            .find(|s| *s.1 == Interaction::Pressed)
            .map(|s| s.0);
    }
    if let (Some(e), Some(mut profile)) = (panel.drag, profile)
        && let Ok((_, _, s, rel)) = sliders.get(e)
        && let Some(n) = rel.normalized
    {
        let v = round2(s.0.at((n.x + 0.5).clamp(0.0, 1.0)));
        if v != s.0.get(&profile.controls) {
            s.0.set(&mut profile.controls, v);
        }
    }
}

/// Keeps the panel's texts, slider fills, binding buttons and speed curve in line with the
/// profile (only writes what changed).
#[allow(clippy::type_complexity)]
fn refresh(
    profile: Option<Res<Profile>>,
    panel: Res<Panel>,
    mut shows: Query<(&Show, &mut Text)>,
    mut fills: Query<(&Fill, &mut Node), Without<Bar>>,
    mut bars: Query<(&Bar, &mut Node, &mut BackgroundColor), Without<Ctl>>,
    mut buttons: Query<(
        &Ctl,
        &Interaction,
        &Children,
        Option<&mut BackgroundColor>,
        Option<&mut BorderColor>,
    )>,
    mut texts: Query<(&mut Text, &mut TextColor), Without<Show>>,
) {
    if bars.is_empty() {
        return;
    }
    let c = profile.as_deref().map_or(&*DEFAULTS, |p| &p.controls);
    for (s, mut t) in &mut shows {
        let want = match *s {
            Show::Knob(k) => k.text(k.get(c)),
            Show::Turn => {
                let cm = |dpi: f32| TAU / (LOOK * c.sens) / dpi * 2.54;
                format!(
                    "{:.1} cm/360 @ 800 DPI, {:.1} @ 1600",
                    cm(800.0),
                    cm(1600.0)
                )
            }
            Show::Status => panel.status.clone(),
        };
        if t.0 != want {
            t.0 = want;
        }
    }
    for (f, mut n) in &mut fills {
        let w = percent(f.0.pos(f.0.get(c)).clamp(0.0, 1.0) * 100.0);
        if n.width != w {
            n.width = w;
        }
    }
    let lit = (panel.speed.round() as usize).min(BARS - 1);
    for (b, mut n, mut bg) in &mut bars {
        let gain = c.gain(b.0 as f32);
        let h = px(6.0 + (gain - 1.0) / (CAP.1 - 1.0) * 53.0);
        if n.height != h {
            n.height = h;
        }
        let col = if b.0 == lit && panel.speed > 0.05 {
            GOLD
        } else {
            Color::srgba(1.0, 1.0, 1.0, 0.35)
        };
        if bg.0 != col {
            bg.0 = col;
        }
    }
    for (ctl, i, kids, bg, border) in &mut buttons {
        let Some(&kid) = kids.first() else { continue };
        let Ok((mut t, mut tc)) = texts.get_mut(kid) else {
            continue;
        };
        let (want, color) = match *ctl {
            Ctl::Bind(a, s) if panel.capture == Some((a, s)) => ("PRESS...".to_owned(), GOLD),
            Ctl::Bind(a, s) => match c.binds[a as usize][s] {
                Some(b) => (label(b), Color::WHITE),
                None => ("-".to_owned(), Color::srgba(1.0, 1.0, 1.0, 0.35)),
            },
            Ctl::Toggle(f) => {
                let on = match f {
                    Flag::Invert => c.invert,
                    Flag::Raw => c.raw,
                };
                ((if on { "ON" } else { "OFF" }).to_owned(), Color::WHITE)
            }
            _ => continue,
        };
        if t.0 != want {
            t.0 = want;
        }
        if tc.0 != color {
            tc.0 = color;
        }
        // binding buttons draw themselves (the retail-textured ones follow `menu::hover`)
        if let (Ctl::Bind(a, s), Some(mut bg), Some(mut border)) = (*ctl, bg, border) {
            let capturing = panel.capture == Some((a, s));
            let fill = match (capturing, *i) {
                (true, _) => Color::srgba(1.0, 0.8, 0.3, 0.18),
                (false, Interaction::None) => Color::srgba(1.0, 1.0, 1.0, 0.06),
                _ => Color::srgba(1.0, 1.0, 1.0, 0.16),
            };
            let edge = BorderColor::all(if capturing {
                GOLD
            } else {
                Color::srgba(1.0, 1.0, 1.0, 0.25)
            });
            if bg.0 != fill {
                bg.0 = fill;
            }
            if *border != edge {
                *border = edge;
            }
        }
    }
}

/// Saves the profile once a controls change settles (the main menu does not save it on every
/// change the way a match does; a slider being dragged waits for the release).
fn persist(
    mouse: Res<ButtonInput<MouseButton>>,
    profile: Option<Res<Profile>>,
    mut saved: Local<Option<Controls>>,
) {
    let Some(p) = profile else { return };
    match &*saved {
        None => *saved = Some(p.controls.clone()),
        Some(c) if *c != p.controls && !mouse.pressed(MouseButton::Left) => {
            p.save();
            *saved = Some(p.controls.clone());
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acceleration_grows_with_speed_up_to_the_cap() {
        let mut c = Controls::default();
        assert_eq!(c.gain(0.0), 1.0);
        assert_eq!(c.gain(30.0), 1.0, "no acceleration by default");
        c.accel = 1.0;
        c.accel_cap = 2.0;
        assert!(c.gain(5.0) > c.gain(1.0));
        assert_eq!(c.gain(1000.0), 2.0);
        // the same 10 counts turn further when moved in 1 ms than in 20 ms
        let fast = c.look(Vec2::new(10.0, 0.0), 0.001).x;
        let slow = c.look(Vec2::new(10.0, 0.0), 0.02).x;
        assert!(fast > slow && slow > 0.0);
        c.invert = true;
        c.vertical = 0.5;
        let v = c.look(Vec2::new(0.0, 10.0), 0.02);
        assert!(v.y < 0.0 && (v.y + 10.0 * LOOK * 0.5 * c.gain(0.5)).abs() < 1e-6);
    }

    #[test]
    fn rebinding_moves_a_key_and_survives_the_profile_text() {
        let mut c = Controls::default();
        // W goes to Jump: Forward loses it
        assert_eq!(
            c.bind(Action::Jump, 1, Some(Bind::Key(KeyCode::KeyW))),
            Some(Action::Forward)
        );
        assert_eq!(c.binds[Action::Forward as usize], [None, None]);
        c.bind(Action::Fire, 0, Some(Bind::Mouse(MouseButton::Back)));
        c.bind(Action::Reload, 0, Some(Bind::WheelDown));
        c.bind(Action::Taunt, 1, None);
        c.sens = 1.35;
        c.invert = true;
        let mut back = Controls::default();
        for line in c.to_text().lines() {
            let (k, v) = line.split_once('=').unwrap();
            assert_eq!(back.set(k, v), Ok(true), "{line}");
        }
        assert_eq!(back, c);
        assert_eq!(back.binds[Action::NextWeapon as usize][1], None);
        assert_eq!(Controls::default().set("name", "x"), Ok(false));
        assert!(Controls::default().set("bind_fire", "Nope,-").is_err());
        assert!(Controls::default().set("mouse_sens", "NaN").is_err());
        // every bindable key round-trips and has a label
        for k in KEYS {
            assert_eq!(
                parse_token(&token(Some(Bind::Key(k)))),
                Some(Some(Bind::Key(k)))
            );
            assert!(!label(Bind::Key(k)).is_empty());
        }
    }
}
