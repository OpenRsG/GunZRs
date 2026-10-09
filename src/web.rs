//! Browser glue of the WebGPU build (`web/`): the page's `index.html` downloads the game data
//! packs (`gunz-pack`) and the chosen match, then starts the wasm module. Everything else runs
//! as on the desktop.

use crate::{game::Frozen, mrs::Vfs};
use bevy::{
    input::{
        InputSystems,
        mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll},
    },
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

/// Tells the page when the match is on screen (`globalThis.gunzReady()`, it hides its loading
/// screen) and hands the end of a match to it (`globalThis.gunzExit(code)`): it starts the same
/// match again or goes back to its menu, as `relaunch` does on the desktop. Also asks for the
/// pointer lock again on a click while playing: browsers grant it only right after a user
/// gesture, and Esc releases it without telling the game. On a touch screen it feeds the page's
/// on-screen controls into the game instead (`touch`).
pub struct WebPlugin;

impl Plugin for WebPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, touch.after(InputSystems))
            .add_systems(Update, (ready, relock))
            .add_systems(Last, exit);
    }
}

#[derive(Clone, Copy)]
enum Ctl {
    Key(KeyCode),
    Mouse(MouseButton),
}

/// Held controls at `gunzTouch[2..12]`, in the page's order: stick W A S D, then jump, fire,
/// guard, reload, scores, pause. The page writes 3 on a press and 2 on a release this side has
/// not seen yet, so a tap shorter than a frame still counts.
const HELD: [Ctl; 10] = [
    Ctl::Key(KeyCode::KeyW),
    Ctl::Key(KeyCode::KeyA),
    Ctl::Key(KeyCode::KeyS),
    Ctl::Key(KeyCode::KeyD),
    Ctl::Key(KeyCode::Space),
    Ctl::Mouse(MouseButton::Left),
    Ctl::Mouse(MouseButton::Right),
    Ctl::Key(KeyCode::KeyR),
    Ctl::Key(KeyCode::Tab),
    Ctl::Key(KeyCode::Escape),
];
/// Taps after the held controls, cleared once read.
const NEXT_WEAPON: usize = 12;
const DASH: usize = 13;

/// The page's touch controls (`globalThis.gunzTouch`, a `Float32Array(14)`; absent without a
/// touch screen): `[0..2]` is the look drag in pixels since the last frame, added to the mouse
/// motion, then [`HELD`], next weapon (a scroll step) and dash. Dash taps the stick's direction
/// (forward if none) twice on consecutive frames, which `drive` takes as a tumble. Runs right
/// after Bevy's input systems, so a touch reaches the game in the same frame as a key would.
/// Also tells the page when the game pauses or resumes (`globalThis.gunzPaused(bool)`): it
/// hides the controls so taps reach the pause and end menus.
#[allow(clippy::too_many_arguments)]
fn touch(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut motion: ResMut<AccumulatedMouseMotion>,
    mut scroll: ResMut<AccumulatedMouseScroll>,
    frozen: Option<Res<Frozen>>,
    mut down: Local<[bool; HELD.len()]>,
    mut dash: Local<(u8, usize)>,
    mut paused: Local<bool>,
) {
    let Ok(t) = global("gunzTouch").dyn_into::<Float32Array>() else {
        return;
    };
    let v = t.to_vec();
    if v.len() <= DASH {
        return;
    }
    for i in [0, 1, NEXT_WEAPON, DASH] {
        t.set_index(i as u32, 0.0);
    }
    for i in 2..2 + HELD.len() {
        match v[i] {
            3.0 => t.set_index(i as u32, 1.0),
            2.0 => t.set_index(i as u32, 0.0),
            _ => {}
        }
    }
    motion.delta += Vec2::new(v[0], v[1]);
    if v[NEXT_WEAPON] != 0.0 {
        scroll.delta.y -= 1.0;
    }
    let mut want: [bool; HELD.len()] = std::array::from_fn(|i| v[2 + i] != 0.0);
    if v[DASH] != 0.0 && dash.0 == 0 {
        *dash = (4, (0..4).find(|&d| want[d]).unwrap_or(0));
    }
    // off, on, off, on: two fresh presses of one direction
    if dash.0 > 0 {
        want[dash.1] = dash.0 % 2 == 1;
        dash.0 -= 1;
    }
    for (i, ctl) in HELD.iter().enumerate() {
        if want[i] == down[i] {
            continue;
        }
        down[i] = want[i];
        match (*ctl, want[i]) {
            (Ctl::Key(k), true) => keys.press(k),
            (Ctl::Key(k), false) => keys.release(k),
            (Ctl::Mouse(b), true) => mouse.press(b),
            (Ctl::Mouse(b), false) => mouse.release(b),
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
    mut windows: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    // no pointer lock on a touch screen: the fire button is a left click there
    if frozen.is_none() && mouse.just_pressed(MouseButton::Left) && !global("gunzTouch").is_object()
    {
        for mut c in &mut windows {
            // assigning marks it changed, so the lock is requested again inside the gesture
            (c.grab_mode, c.visible) = (CursorGrabMode::Locked, false);
        }
    }
}

fn exit(mut exits: MessageReader<AppExit>) {
    if let Some(e) = exits.read().last() {
        let code = match e {
            AppExit::Success => 0,
            AppExit::Error(c) => c.get(),
        };
        if let Ok(f) = global("gunzExit").dyn_into::<Function>() {
            let _ = f.call1(&JsValue::NULL, &code.into());
        }
    }
}
