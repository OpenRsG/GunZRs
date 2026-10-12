//! Where the sound goes: the PulseAudio / PipeWire sink chosen on the GRAPHICS & SOUND page
//! (`Graphics::out`, empty = the system default; Linux only). bevy_audio opens its one output
//! stream on the default device at startup and has no device choice, so this moves the game's
//! stream with `pactl` instead, at startup and on every change. That also overrides a sink the
//! sound server remembered for the game (a stream once moved by hand keeps going there).

use crate::{profile::Profile, view::Shot};
use bevy::prelude::*;
use std::process::Command;

pub struct OutputPlugin;

impl Plugin for OutputPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Outputs::list()).add_systems(
            Update,
            route.run_if(resource_exists::<Profile>.and_then(not(resource_exists::<Shot>))),
        );
    }
}

/// The sound server's sinks: name and description.
#[derive(Resource, Default)]
pub struct Outputs(pub Vec<(String, String)>);

impl Outputs {
    pub fn list() -> Self {
        Self(pactl(&["list", "sinks"]).map_or_else(Vec::new, |t| sinks(&t)))
    }

    pub fn label(&self, name: &str) -> String {
        if name.is_empty() {
            return "System default".into();
        }
        self.0
            .iter()
            .find(|s| s.0 == name)
            .map_or_else(|| format!("{name} (not connected)"), |s| s.1.clone())
    }

    /// The sink `d` steps from `name` (the system default comes first).
    pub fn step(&self, name: &str, d: i8) -> String {
        let at = self.0.iter().position(|s| s.0 == name).map_or(0, |i| i + 1);
        let n = self.0.len() as i32 + 1;
        match (at as i32 + d as i32).rem_euclid(n) {
            0 => String::new(),
            i => self.0[i as usize - 1].0.clone(),
        }
    }
}

fn pactl(args: &[&str]) -> Option<String> {
    let out = Command::new("pactl")
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `pactl list sinks`: each sink's `Name:` and `Description:`.
fn sinks(text: &str) -> Vec<(String, String)> {
    let mut out = vec![];
    let mut name = None;
    for line in text.lines().map(str::trim) {
        if let Some(n) = line.strip_prefix("Name: ") {
            name = Some(n.to_owned());
        } else if let Some(d) = line.strip_prefix("Description: ")
            && let Some(n) = name.take()
        {
            out.push((n, d.to_owned()));
        }
    }
    out
}

/// The `#N` of each block of a `pactl list` output whose lines contain `has`.
fn blocks_with(text: &str, head: &str, has: &str) -> Vec<String> {
    text.split(head)
        .skip(1)
        .filter(|b| b.lines().any(|l| l.trim() == has))
        .filter_map(|b| b.lines().next().map(|n| n.trim().to_owned()))
        .collect()
}

/// This process's playback streams: sink inputs of the clients with its process id.
fn ours() -> Vec<String> {
    let pid = format!("application.process.id = \"{}\"", std::process::id());
    let clients =
        pactl(&["list", "clients"]).map_or_else(Vec::new, |t| blocks_with(&t, "Client #", &pid));
    pactl(&["list", "sink-inputs"]).map_or_else(Vec::new, |t| {
        clients
            .iter()
            .flat_map(|c| blocks_with(&t, "Sink Input #", &format!("Client: {c}")))
            .collect()
    })
}

/// Moves the game's stream when the chosen sink changes (off the main thread: `pactl` takes a
/// few milliseconds).
fn route(profile: Res<Profile>, mut done: Local<Option<String>>) {
    let want = &profile.graphics.out;
    if done.as_ref() == Some(want) {
        return;
    }
    *done = Some(want.clone());
    let target = if want.is_empty() {
        "@DEFAULT_SINK@".to_owned()
    } else {
        want.clone()
    };
    std::thread::spawn(move || {
        let streams = ours();
        if streams.is_empty() {
            info!("audio out: no stream of this process found (pactl missing or no sound server)");
        }
        for s in streams {
            match pactl(&["move-sink-input", &s, &target]) {
                Some(_) => info!("audio out: stream {s} -> {target}"),
                None => warn!("audio out: could not move stream {s} to {target}"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_sinks_and_our_streams() {
        let sinks_text = "Sink #63\n\tState: RUNNING\n\tName: a50\n\tDescription: A50 Analog Stereo\n\
            \tDriver: PipeWire\nSink #75\n\tName: hdmi\n\tDescription: HDMI\n";
        let o = Outputs(sinks(sinks_text));
        assert_eq!(
            o.0,
            [
                ("a50".into(), "A50 Analog Stereo".into()),
                ("hdmi".into(), "HDMI".into())
            ]
        );
        // default -> a50 -> hdmi -> default, and back
        assert_eq!(o.step("", 1), "a50");
        assert_eq!(o.step("hdmi", 1), "");
        assert_eq!(o.step("", -1), "hdmi");
        assert_eq!(o.step("gone", 1), "a50");
        assert_eq!(o.label("gone"), "gone (not connected)");
        let inputs = "Sink Input #1012\n\tDriver: PipeWire\n\tClient: 1021\n\tSink: 75\n\
            Sink Input #1013\n\tClient: 10210\n";
        assert_eq!(
            blocks_with(inputs, "Sink Input #", "Client: 1021"),
            ["1012"]
        );
    }
}
