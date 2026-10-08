//! Frame-time logger, active only when the `GUNZ_FRAMETIMES` environment variable is set
//! (any value). Every frame longer than [`HITCH_MS`] is printed to stderr with the main
//! schedules that took long, how many assets/entities appeared during it, and a summary
//! (p50/p99/max, frames over 20/33 ms) is printed at exit.
//!
//! A timestamp schedule is slotted before and after every main schedule, so the log names
//! the schedule that stalled. For system-level causes build with `--features
//! bevy/trace_chrome` (writes `trace-*.json`).
//!
//! In headless `--shot` runs the app loop waits to 1/60 s per frame, so a frame time is
//! `max(16.7 ms, real work)`; hitches are the frames where the work exceeded that.

use bevy::{app::MainScheduleOrder, audio::AudioSource, ecs::schedule::ScheduleLabel, prelude::*};
use std::time::Instant;

pub const HITCH_MS: f32 = 20.0;
/// Frames counted as loading (shader compiles, first-use asset reads) in the summary: 1.5 s
/// at 60 fps, the settling time of headless runs.
const WARM: usize = 90;

#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
struct Stamp(usize);

pub struct PerfPlugin;

impl Plugin for PerfPlugin {
    fn build(&self, app: &mut App) {
        if std::env::var_os("GUNZ_FRAMETIMES").is_none() {
            return;
        }
        let labels = app.world().resource::<MainScheduleOrder>().labels.clone();
        let names: Vec<String> = labels.iter().map(|l| format!("{l:?}")).collect();
        // Stamp(0) runs before the first schedule, Stamp(i + 1) after schedule i.
        let mut order = app.world_mut().resource_mut::<MainScheduleOrder>();
        order.labels.clear();
        order.labels.push(Stamp(0).intern());
        for (i, l) in labels.into_iter().enumerate() {
            order.labels.push(l);
            order.labels.push(Stamp(i + 1).intern());
        }
        let last = names.len();
        for i in 0..=last {
            app.add_systems(Stamp(i), move |world: &mut World| {
                world.resource_scope(|world, mut p: Mut<Perf>| p.stamp(world, i));
            });
        }
        app.insert_resource(Perf {
            names,
            at: vec![Instant::now(); last + 1],
            prev_start: None,
            dts: Vec::new(),
            counts: [0; 4],
        })
        .add_systems(Last, summary);
    }
}

#[derive(Resource)]
struct Perf {
    names: Vec<String>,
    at: Vec<Instant>,
    prev_start: Option<Instant>,
    /// Frame periods in ms.
    dts: Vec<f32>,
    /// Image, mesh, material, audio asset and entity counts at the previous frame's end.
    counts: [usize; 4],
}

fn count<A: Asset>(world: &World) -> usize {
    world.get_resource::<Assets<A>>().map_or(0, Assets::len)
}

impl Perf {
    /// At a frame start, closes the previous frame: its period and, for a hitch, the
    /// schedule times and the assets/entities that appeared since the frame before.
    fn stamp(&mut self, world: &World, i: usize) {
        let now = Instant::now();
        let ms = |a: Instant, b: Instant| b.duration_since(a).as_secs_f32() * 1e3;
        if i == 0
            && let Some(prev) = self.prev_start
        {
            let dt = ms(prev, now);
            self.dts.push(dt);
            let counts = [
                count::<Image>(world),
                count::<Mesh>(world),
                count::<StandardMaterial>(world),
                count::<AudioSource>(world),
            ];
            if dt > HITCH_MS {
                let slow: Vec<String> = (0..self.names.len())
                    .filter_map(|k| {
                        let t = ms(self.at[k], self.at[k + 1]);
                        (t >= 1.0).then(|| format!("{}={t:.1}", self.names[k]))
                    })
                    .collect();
                let d = |k: usize| counts[k] as isize - self.counts[k] as isize;
                eprintln!(
                    "[perf] frame {} period={dt:.1}ms main={:.1}ms [{}] +img{} +mesh{} +mat{} +snd{}",
                    self.dts.len(),
                    ms(self.at[0], self.at[self.names.len()]),
                    slow.join(" "),
                    d(0),
                    d(1),
                    d(2),
                    d(3),
                );
            }
            self.counts = counts;
        }
        if i == 0 {
            self.prev_start = Some(now);
        }
        self.at[i] = now;
    }
}

fn percentile(sorted: &[f32], p: f32) -> f32 {
    sorted[((sorted.len() - 1) as f32 * p).round() as usize]
}

fn stats(label: &str, dts: &[f32]) {
    if dts.is_empty() {
        return;
    }
    let mut s = dts.to_vec();
    s.sort_by(f32::total_cmp);
    eprintln!(
        "[perf] {label}: frames={} p50={:.1} p99={:.1} max={:.1} >20ms={} >33ms={}",
        s.len(),
        percentile(&s, 0.5),
        percentile(&s, 0.99),
        s[s.len() - 1],
        s.iter().filter(|&&d| d > 20.0).count(),
        s.iter().filter(|&&d| d > 33.0).count(),
    );
}

fn summary(mut exit: MessageReader<AppExit>, perf: Res<Perf>) {
    if exit.read().next().is_none() {
        return;
    }
    let warm = WARM.min(perf.dts.len());
    stats("load", &perf.dts[..warm]);
    stats("play", &perf.dts[warm..]);
}
