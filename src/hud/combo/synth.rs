//! The combo layer's sounds, synthesized in code (**not retail**: nothing is copied from the game
//! or elsewhere): sums of sines and additive saws, exponential envelopes, pitch sweeps and filtered
//! noise, mixed to 16-bit mono PCM WAV at [`RATE`] and built once at startup. The retail data has no
//! announcer or kill-streak clips (`docs/formats.md`, "Combo and killing spree").

use std::f32::consts::TAU;

pub const RATE: u32 = 44_100;
const SR: f32 = RATE as f32;

/// Clip indices of [`clips`]: the hit tick, the multiplier whoosh, the kill impact, the headshot
/// ping, then one spree stinger per tier from [`STING`].
pub const TICK: usize = 0;
pub const STEP: usize = 1;
pub const KILL: usize = 2;
pub const PING: usize = 3;
pub const STING: usize = 4;
/// Spree stinger tiers.
pub const TIERS: usize = 5;

pub const NAMES: [&str; STING + TIERS] = [
    "tick", "step", "kill", "ping", "sting0", "sting1", "sting2", "sting3", "sting4",
];

/// Major arpeggio over three octaves, in semitones.
const ARP: [f32; 8] = [0.0, 4.0, 7.0, 12.0, 16.0, 19.0, 24.0, 28.0];

/// White noise in -1..=1 (xorshift32).
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as f32 / u32::MAX as f32 * 2.0 - 1.0
    }
}

fn tone(f: f32, t: f32) -> f32 {
    (TAU * f * t).sin()
}

/// A bright saw: the first four harmonics.
fn saw(f: f32, t: f32) -> f32 {
    (1..=4)
        .map(|h| tone(f * h as f32, t) / h as f32)
        .sum::<f32>()
        * 0.6
}

fn decay(t: f32, tau: f32) -> f32 {
    (-t / tau).exp()
}

/// Click-free start: a linear ramp over `secs`.
fn attack(t: f32, secs: f32) -> f32 {
    (t / secs).min(1.0)
}

/// Phase (rad) of an exponential sweep from `f0` to `f1` Hz over `secs`, then held at `f1`.
fn sweep(t: f32, f0: f32, f1: f32, secs: f32) -> f32 {
    let k = (f1 / f0).ln() / secs;
    let ts = t.min(secs);
    TAU * (f0 * ((k * ts).exp() - 1.0) / k + f1 * (t - ts))
}

/// Mono samples being mixed.
struct Clip(Vec<f32>);

impl Clip {
    fn new(secs: f32) -> Self {
        Self(vec![0.0; (secs * SR) as usize])
    }

    /// Adds a voice `f(t)`, `t` seconds since `at`, for `secs` (or until the clip ends).
    fn add(&mut self, at: f32, secs: f32, mut f: impl FnMut(f32) -> f32) {
        let start = ((at * SR) as usize).min(self.0.len());
        let end = (start + (secs * SR) as usize).min(self.0.len());
        for (i, s) in self.0[start..end].iter_mut().enumerate() {
            *s += f(i as f32 / SR);
        }
    }

    /// 16-bit PCM WAV, scaled so the loudest sample is `peak` (0..=1) of full scale, with the
    /// last 5 ms faded out.
    fn wav(&self, peak: f32) -> Vec<u8> {
        let n = self.0.len();
        let top = self.0.iter().fold(1e-6f32, |m, s| m.max(s.abs()));
        let gain = peak / top * 32767.0;
        let fade = (0.005 * SR) as usize;
        let data = n as u32 * 2;
        let mut out = Vec::with_capacity(44 + n * 2);
        out.extend(b"RIFF");
        out.extend((36 + data).to_le_bytes());
        out.extend(b"WAVEfmt ");
        out.extend(16u32.to_le_bytes());
        out.extend(1u16.to_le_bytes()); // PCM
        out.extend(1u16.to_le_bytes()); // mono
        out.extend(RATE.to_le_bytes());
        out.extend((RATE * 2).to_le_bytes());
        out.extend(2u16.to_le_bytes());
        out.extend(16u16.to_le_bytes());
        out.extend(b"data");
        out.extend(data.to_le_bytes());
        for (i, s) in self.0.iter().enumerate() {
            let tail = (n - 1 - i).min(fade) as f32 / fade as f32;
            out.extend(((s * gain * tail).round() as i16).to_le_bytes());
        }
        out
    }
}

/// A short click-and-ping at 1 kHz; the player pitches it up with the combo (playback speed).
fn tick() -> Vec<u8> {
    let mut c = Clip::new(0.09);
    let mut n = Noise(0x1234_5678);
    c.add(0.0, 0.09, |t| {
        (0.7 * tone(1000.0, t) * decay(t, 0.02)
            + 0.3 * tone(2000.0, t) * decay(t, 0.01)
            + 0.25 * n.next() * decay(t, 0.002))
            * attack(t, 0.0008)
    });
    c.wav(0.45)
}

/// The multiplier stepped up: a rising sine sweep under a brightening noise whoosh, ending on a
/// high ding.
fn step() -> Vec<u8> {
    let mut c = Clip::new(0.5);
    let mut n = Noise(0x0BAD_F00D);
    let mut low = 0.0;
    c.add(0.0, 0.5, |t| {
        let rise = (t / 0.3).min(1.0).powi(2) * decay((t - 0.3).max(0.0), 0.07);
        low += (0.02 + 0.3 * (t / 0.32).min(1.0)) * (n.next() - low);
        (0.8 * sweep(t, 220.0, 1760.0, 0.32).sin() + 1.2 * low) * rise
    });
    c.add(0.28, 0.22, |t| {
        0.25 * (tone(2093.0, t) + 0.6 * tone(3136.0, t)) * decay(t, 0.07) * attack(t, 0.002)
    });
    c.wav(0.6)
}

/// A kill: a falling kick thump and a noise snap under a bright C major chord.
fn kill() -> Vec<u8> {
    let mut c = Clip::new(0.6);
    let mut n = Noise(0x2545_F491);
    c.add(0.0, 0.3, |t| {
        1.2 * sweep(t, 190.0, 45.0, 0.14).sin() * decay(t, 0.08) * attack(t, 0.001)
    });
    c.add(0.0, 0.08, |t| 0.6 * n.next() * decay(t, 0.012));
    for f in [523.25, 659.25, 783.99, 1046.5] {
        c.add(0.025, 0.55, move |t| {
            0.2 * (tone(f, t) + 0.3 * tone(2.0 * f, t) + 0.12 * tone(3.0 * f, t))
                * decay(t, 0.17)
                * attack(t, 0.002)
        });
    }
    c.wav(0.85)
}

/// A glassy two-partial ping on top of a headshot kill.
fn ping() -> Vec<u8> {
    let mut c = Clip::new(0.4);
    c.add(0.0, 0.4, |t| {
        (tone(2637.0, t) + 0.5 * tone(2650.0, t) + 0.8 * tone(3951.0, t))
            * decay(t, 0.1)
            * attack(t, 0.001)
    });
    c.wav(0.5)
}

/// Spree stinger `tier` (0..[`TIERS`]): a bass drop and noise impact, a rising arpeggio with
/// one more note per tier, and a shimmer of high partials. Higher tiers sit a whole tone higher
/// each, ring longer and play louder.
fn sting(tier: usize) -> Vec<u8> {
    let root = 110.0 * (tier as f32 * 2.0 / 12.0).exp2();
    let notes = 4 + tier;
    let gap = 0.075;
    let end = 0.1 + notes as f32 * gap;
    let mut c = Clip::new(end + 0.8 + 0.1 * tier as f32);
    c.add(0.0, 1.0, |t| {
        let p = sweep(t, root * 3.0, root * 0.5, 0.35);
        (p.sin() + 0.4 * (2.0 * p).sin()) * decay(t, 0.28) * attack(t, 0.002)
    });
    let mut n = Noise(0x9E37_79B9);
    let mut low = 0.0;
    c.add(0.0, 0.3, |t| {
        low += 0.25 * (n.next() - low);
        2.5 * low * decay(t, 0.06)
    });
    for (i, semi) in ARP.iter().take(notes).enumerate() {
        let f = root * 4.0 * (semi / 12.0).exp2();
        let ring = if i + 1 == notes { 0.35 } else { 0.12 };
        c.add(0.1 + i as f32 * gap, 1.0, move |t| {
            0.3 * saw(f, t) * decay(t, ring) * attack(t, 0.003)
        });
    }
    for m in [8.0, 12.0, 16.0, 24.0] {
        let f = root * m;
        c.add(end - gap, 1.5, move |t| {
            0.05 * (TAU * f * t + 0.6 * tone(5.5, t)).sin()
                * attack(t, 0.08)
                * decay(t, 0.45 + 0.1 * tier as f32)
        });
    }
    c.wav(0.6 + 0.075 * tier as f32)
}

/// Every clip as WAV bytes, in [`NAMES`] order.
pub fn clips() -> Vec<Vec<u8>> {
    let mut all = vec![tick(), step(), kill(), ping()];
    all.extend((0..TIERS).map(sting));
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clips_are_normalised_pcm_wavs() {
        let all = clips();
        assert_eq!(all.len(), NAMES.len());
        for (name, w) in NAMES.iter().zip(&all) {
            assert!(w.starts_with(b"RIFF") && &w[8..16] == b"WAVEfmt ", "{name}");
            let samples: Vec<i16> = w[44..]
                .chunks(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]))
                .collect();
            assert_eq!(
                u32::from_le_bytes(w[40..44].try_into().unwrap()) as usize,
                w.len() - 44
            );
            let secs = samples.len() as f32 / SR;
            let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap() as f32 / 32767.0;
            assert!((0.05..2.5).contains(&secs), "{name}: {secs} s");
            assert!((0.4..=0.91).contains(&peak), "{name}: peak {peak}");
            assert_eq!(samples.last(), Some(&0), "{name}: ends on silence");
        }
    }
}
