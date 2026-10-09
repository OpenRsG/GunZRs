//! Browser glue of the WebGPU build (`web/`): the page's `index.html` downloads the game data
//! packs (`gunz-pack`) and the chosen match, then starts the wasm module. Everything else runs
//! as on the desktop.

use crate::{game::Frozen, mrs::Vfs};
use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use js_sys::{Array, Function, Reflect, Uint8Array};
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

/// Hands the end of a match to the page (`globalThis.gunzExit(code)`): it starts the same
/// match again or goes back to its menu, as `relaunch` does on the desktop. Also asks for the
/// pointer lock again on a click while playing: browsers grant it only right after a user
/// gesture, and Esc releases it without telling the game.
pub struct WebPlugin;

impl Plugin for WebPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, relock).add_systems(Last, exit);
    }
}

fn relock(
    mouse: Res<ButtonInput<MouseButton>>,
    frozen: Option<Res<Frozen>>,
    mut windows: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    if frozen.is_none() && mouse.just_pressed(MouseButton::Left) {
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
