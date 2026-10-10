//! Browser glue of the WebGPU build (`web/`): the page's `index.html` downloads the game data
//! packs (`gunz-pack`) and the chosen match, then starts the wasm module. Everything else runs
//! as on the desktop.

use crate::{
    controls::{Action, Pad},
    game::Frozen,
    mrs::Vfs,
    profile::Profile,
};
use bevy::{
    input::{InputSystems, mouse::AccumulatedMouseMotion},
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use js_sys::{Array, Float32Array, Function, Reflect, Uint8Array};
use std::io;
use wasm_bindgen::{JsCast, JsValue};

fn global(name: &str) -> JsValue {
    Reflect::get(&js_sys::global(), &name.into()).unwrap_or(JsValue::UNDEFINED)
}

/// The `gunz-play` arguments the page chose (`globalThis.gunzArgs`, an array of strings).
pub fn args() -> Vec<String> {
    global("gunzArgs")
        .dyn_into::<Array>()
        .map(|a| a.iter().filter_map(|v| v.as_string()).collect())
        .unwrap_or_default()
}

/// The game files of the packs the page downloaded (`globalThis.gunzPacks`, `Uint8Array`s).
pub fn vfs() -> io::Result<Vfs> {
    let packs: Vec<Vec<u8>> = global("gunzPacks")
        .dyn_into::<Array>()
        .map_err(|_| io::Error::other("the page passed no game packs"))?
        .iter()
        .filter_map(|p| p.dyn_into::<Uint8Array>().ok())
        .map(|p| p.to_vec())
        .collect();
    // the page's copies are no longer needed
    let _ = Reflect::set(&js_sys::global(), &"gunzPacks".into(), &JsValue::UNDEFINED);
    Vfs::from_packs(packs.iter().map(Vec::as_slice))
}

/// A file served next to the packs (`mrs::Packed::Fetch`), downloaded now: the page's
/// `globalThis.gunzFetch(path)` returns its bytes (a synchronous request; the clothes are a few
/// hundred kB each) or `null`.
pub fn fetch(path: &str) -> io::Result<Vec<u8>> {
    global("gunzFetch")
        .dyn_into::<Function>()
        .ok()
        .and_then(|f| f.call1(&JsValue::NULL, &path.into()).ok())
        .and_then(|b| b.dyn_into::<Uint8Array>().ok())
        .map(|b| b.to_vec())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{path}: download failed")))
}

const PROFILE_KEY: &str = "gunzrs.profile";

fn storage() -> Option<JsValue> {
    Some(global("localStorage")).filter(JsValue::is_object)
}

fn call(on: &JsValue, method: &str, args: &[JsValue]) -> Option<JsValue> {
    let f = Reflect::get(on, &method.into())
        .ok()?
        .dyn_into::<Function>()
        .ok()?;
    f.apply(on, &args.iter().collect::<Array>()).ok()
}

/// The profile text the browser keeps (`localStorage`).
pub fn load_profile() -> Option<String> {
    call(&storage()?, "getItem", &[PROFILE_KEY.into()])?.as_string()
}

pub fn store_profile(text: &str) {
    if let Some(s) = storage() {
        call(&s, "setItem", &[PROFILE_KEY.into(), text.into()]);
    }
}

/// The menu's START: the page loads the map and starts the match
/// (`globalThis.gunzPlay(map, flags)`).
pub fn play(cfg: &crate::menu::Config) {
    let flags: Array = cfg.flags().iter().map(|s| JsValue::from_str(s)).collect();
    let map = cfg.map.clone().unwrap_or_default();
    call(
        &js_sys::global().into(),
        "gunzPlay",
        &[map.into(), flags.into()],
    );
}

/// Tells the page when the match is on screen (`globalThis.gunzReady()`, it hides its loading
/// screen) and hands the end of a match to it (`globalThis.gunzExit(code)`): it starts the same
/// match again or goes back to its menu, as `relaunch` does on the desktop. Also asks for the
/// pointer lock again on a click while playing: browsers grant it only right after a user
/// gesture, and Esc releases it without telling the game. On a touch screen it feeds the page's
/// on-screen controls into the game instead (`touch`).
pub struct WebPlugin;

impl Plugin for WebPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, (touch.after(InputSystems), gpu_errors))
            .add_systems(Update, (ready, relock, raw_input))
            .add_systems(Last, exit);
    }
}

/// GPU errors the page saw (`globalThis.gunzGpuErrors`, a count, and `gunzGpuError`, the last
/// message): Safari hands them only to `uncapturederror` listeners, never to wgpu's handler,
/// so bevy's error handling never hears of them there.
fn gpu_errors(world: &mut World, mut seen: Local<f64>) {
    let n = global("gunzGpuErrors").as_f64().unwrap_or(0.0);
    if n > *seen {
        *seen = n;
        let what = global("gunzGpuError").as_string().unwrap_or_default();
        crate::gfx::gpu_failed(world, &what);
    }
}

/// Held controls at `gunzTouch[2..11]`, in the page's order: stick forward, left, back, right,
/// then jump, fire, guard, reload, scores; `[11]` is pause (Esc). The page writes 3 on a press
/// and 2 on a release this side has not seen yet, so a tap shorter than a frame still counts.
const HELD: [Action; 9] = [
    Action::Forward,
    Action::Left,
    Action::Back,
    Action::Right,
    Action::Jump,
    Action::Fire,
    Action::Guard,
    Action::Reload,
    Action::Score,
];
const PAUSE: usize = 11;
/// Taps after the held controls, cleared once read; then the aim assist strength (0..=1).
const NEXT_WEAPON: usize = 12;
const DASH: usize = 13;
const AIM_ASSIST: usize = 14;

/// The page's touch controls (`globalThis.gunzTouch`, a `Float32Array(15)`; absent without a
/// touch screen, or while the player uses a mouse and keyboard on one): `[0..2]` is the look drag in pixels since the last frame, then [`HELD`],
/// pause, next weapon, dash and the aim assist strength (`Settings::aim_assist`). They hold
/// actions in the [`Pad`], so they work whatever the keys are bound to. The look drag replaces
/// the mouse motion: the browser also reports every finger's movement (the stick's too) as
/// mouse motion. Runs right after Bevy's input systems, so a touch reaches the game in the
/// same frame as a key would. Also tells the page when the game pauses or resumes
/// (`globalThis.gunzPaused(bool)`): it hides the controls so taps reach the pause and end menus.
#[allow(clippy::too_many_arguments)]
fn touch(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut motion: ResMut<AccumulatedMouseMotion>,
    mut pad: ResMut<Pad>,
    frozen: Option<Res<Frozen>>,
    settings: Option<ResMut<crate::game::Settings>>,
    mut esc: Local<bool>,
    mut paused: Local<bool>,
    touch_screen: Option<Res<crate::game::TouchScreen>>,
    mut commands: Commands,
) {
    let Ok(t) = global("gunzTouch").dyn_into::<Float32Array>() else {
        if touch_screen.is_some() {
            commands.remove_resource::<crate::game::TouchScreen>();
            *pad = Pad::default();
        }
        return;
    };
    if touch_screen.is_none() {
        commands.insert_resource(crate::game::TouchScreen);
    }
    let v = t.to_vec();
    if v.len() <= DASH {
        return;
    }
    for i in [0, 1, NEXT_WEAPON, DASH] {
        t.set_index(i as u32, 0.0);
    }
    for i in 2..=PAUSE {
        match v[i] {
            3.0 => t.set_index(i as u32, 1.0),
            2.0 => t.set_index(i as u32, 0.0),
            _ => {}
        }
    }
    motion.delta = Vec2::new(v[0], v[1]);
    let aim = v.get(AIM_ASSIST).copied().unwrap_or(0.0).clamp(0.0, 1.0);
    if let Some(mut s) = settings
        && s.aim_assist != aim
    {
        s.aim_assist = aim;
    }
    pad.before = pad.now;
    for (i, a) in HELD.iter().enumerate() {
        pad.now[*a as usize] = v[2 + i] != 0.0;
    }
    pad.now[Action::NextWeapon as usize] = v[NEXT_WEAPON] != 0.0;
    pad.now[Action::Dash as usize] = v[DASH] != 0.0;
    let pause = v[PAUSE] != 0.0;
    if pause != *esc {
        *esc = pause;
        if pause {
            keys.press(KeyCode::Escape);
        } else {
            keys.release(KeyCode::Escape);
        }
    }
    if frozen.is_some() != *paused {
        *paused = !*paused;
        if let Ok(f) = global("gunzPaused").dyn_into::<Function>() {
            let _ = f.call1(&JsValue::NULL, &(*paused).into());
        }
    }
}

/// Frames rendered before the page is told; the first ones still compile pipelines.
const READY_FRAMES: u32 = 3;

fn ready(mut frames: Local<u32>) {
    *frames += 1;
    if *frames == READY_FRAMES
        && let Ok(f) = global("gunzReady").dyn_into::<Function>()
    {
        let _ = f.call0(&JsValue::NULL);
    }
}

fn relock(
    mouse: Res<ButtonInput<MouseButton>>,
    frozen: Option<Res<Frozen>>,
    clock: Option<Res<crate::session::Clock>>,
    mut windows: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    // only in a match (the menu keeps its cursor), and never on a touch screen: the fire
    // button is a left click there
    if clock.is_some()
        && frozen.is_none()
        && mouse.get_just_pressed().next().is_some()
        && !global("gunzTouch").is_object()
    {
        for mut c in &mut windows {
            // assigning marks it changed, so the lock is requested again inside the gesture
            (c.grab_mode, c.visible) = (CursorGrabMode::Locked, false);
        }
    }
}

/// Hands the profile's "Raw input" choice to the page (`globalThis.gunzRaw`): its pointer-lock
/// wrapper asks for unaccelerated movement (`unadjustedMovement`) with it.
fn raw_input(profile: Option<Res<Profile>>, mut sent: Local<Option<bool>>) {
    let raw = profile.is_none_or(|p| p.controls.raw);
    if *sent != Some(raw) {
        *sent = Some(raw);
        let _ = Reflect::set(&js_sys::global(), &"gunzRaw".into(), &raw.into());
    }
}

fn exit(mut exits: MessageReader<AppExit>) {
    if let Some(e) = exits.read().last() {
        leave(match e {
            AppExit::Success => 0,
            AppExit::Error(c) => c.get().into(),
        });
    }
}

/// Hands the end of the game to the page (`globalThis.gunzExit(code)`).
pub fn leave(code: i32) {
    if let Ok(f) = global("gunzExit").dyn_into::<Function>() {
        let _ = f.call1(&JsValue::NULL, &code.into());
    }
}
