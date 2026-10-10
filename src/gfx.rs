//! Graphics settings saved in the profile ([`Graphics`], `gfx_*` keys, `docs/formats.md`
//! "Graphics"): presets (ORIGINAL renders as the game always did: no tonemapping, no effects),
//! HDR with tonemapping, colour grading, bloom, anti-aliasing, sharpening, distance fog,
//! anisotropic filtering, field of view, vsync and a frame limiter, and screen effects (vignette,
//! film grain, chromatic aberration, camera-turn motion blur, speed effects, low-health and
//! hit feedback). Changes apply to the camera at once; the GRAPHICS panel the main menu (a page)
//! and the pause menu (an overlay) share is at the end of this file.

use crate::{
    game::{Damage, Dead, Motor, Player, Settings, Vitals},
    level::MapMaterial,
    menu::{ACCENT, Chosen, DIM, button, heading, panel, primary},
    profile::Profile,
};
use bevy::{
    anti_alias::{
        contrast_adaptive_sharpening::ContrastAdaptiveSharpening, fxaa::Fxaa, smaa::Smaa,
    },
    asset::embedded_asset,
    camera::Hdr,
    core_pipeline::{
        Core3dSystems,
        fullscreen_material::{FullscreenMaterial, FullscreenMaterialPlugin},
        tonemapping::Tonemapping,
    },
    ecs::{schedule::ScheduleConfigs, system::BoxedSystem},
    image::ImageSampler,
    input::InputSystems,
    pbr::{DistanceFog, FogFalloff},
    post_process::{
        bloom::{Bloom, BloomCompositeMode, BloomPrefilter, bloom},
        effect_stack::{ChromaticAberration, Vignette},
    },
    prelude::*,
    render::{
        extract_component::ExtractComponent,
        render_resource::{ShaderType, WgpuFeatures},
        renderer::RenderDevice,
        view::{ColorGrading, ColorGradingGlobal, ColorGradingSection, Msaa},
    },
    shader::ShaderRef,
    ui::{RelativeCursorPosition, UiSystems},
    window::{PresentMode, PrimaryWindow},
};
use std::{collections::HashSet, f32::consts::TAU};

// ---- the settings ----

/// A slider setting. The order is that of [`KNOBS`] and `Graphics::knobs`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Knob {
    Exposure,
    Contrast,
    Saturation,
    Bloom,
    Sharpen,
    Fog,
    Fov,
    Vignette,
    Grain,
    Aberration,
    MotionBlur,
    SpeedFx,
    LowHealth,
    HitFlash,
    ActorLight,
    DynLight,
    Shadow,
}

pub const N: usize = 17;

impl Knob {
    pub const ALL: [Knob; N] = [
        Knob::Exposure,
        Knob::Contrast,
        Knob::Saturation,
        Knob::Bloom,
        Knob::Sharpen,
        Knob::Fog,
        Knob::Fov,
        Knob::Vignette,
        Knob::Grain,
        Knob::Aberration,
        Knob::MotionBlur,
        Knob::SpeedFx,
        Knob::LowHealth,
        Knob::HitFlash,
        Knob::ActorLight,
        Knob::DynLight,
        Knob::Shadow,
    ];

    fn row(self) -> &'static Row {
        &KNOBS[self as usize]
    }

    pub fn label(self) -> &'static str {
        self.row().1
    }

    pub fn range(self) -> (f32, f32) {
        (self.row().2, self.row().3)
    }

    pub fn step(self) -> f32 {
        self.row().4
    }

    /// Slider position 0..=1 of a value.
    pub fn pos(self, v: f32) -> f32 {
        let (lo, hi) = self.range();
        (v - lo) / (hi - lo)
    }

    pub fn at(self, t: f32) -> f32 {
        let (lo, hi) = self.range();
        lo + (hi - lo) * t
    }

    /// On-screen value: effect strengths read OFF at 0.
    pub fn text(self, v: f32) -> String {
        match self {
            Knob::Exposure => format!("{v:+.1} EV"),
            Knob::Fov => format!("{v:.0} deg"),
            Knob::Contrast | Knob::Saturation => format!("{:.0}%", v * 100.0),
            _ if v == 0.0 => "OFF".into(),
            _ => format!("{:.0}%", v * 100.0),
        }
    }
}

/// Profile key (after `gfx_`), label, range, step, and the value in ORIGINAL, ENHANCED and ULTRA.
type Row = (&'static str, &'static str, f32, f32, f32, [f32; 3]);

const KNOBS: [Row; N] = [
    ("exposure", "Exposure", -2.0, 2.0, 0.1, [0.0, 0.2, 0.3]),
    ("contrast", "Contrast", 0.7, 1.5, 0.05, [1.0, 1.05, 1.1]),
    ("saturation", "Saturation", 0.0, 2.0, 0.05, [1.0, 1.1, 1.1]),
    ("bloom", "Bloom", 0.0, 1.0, 0.05, [0.0, 0.3, 0.5]),
    ("sharpen", "Sharpening", 0.0, 1.0, 0.05, [0.0, 0.3, 0.5]),
    ("fog", "Distance fog", 0.0, 1.0, 0.05, [0.0, 0.0, 0.25]),
    ("fov", "Field of view", 70.0, 120.0, 1.0, [90.0, 90.0, 90.0]),
    ("vignette", "Vignette", 0.0, 1.0, 0.05, [0.0, 0.3, 0.4]),
    ("grain", "Film grain", 0.0, 1.0, 0.05, [0.0, 0.0, 0.2]),
    ("aberration", "Aberration", 0.0, 1.0, 0.05, [0.0, 0.0, 0.2]),
    (
        "motion_blur",
        "Motion blur",
        0.0,
        1.0,
        0.05,
        [0.0, 0.0, 0.4],
    ),
    ("speed_fx", "Speed effects", 0.0, 1.0, 0.05, [0.0, 0.6, 0.8]),
    ("low_health", "Low health", 0.0, 1.0, 0.05, [0.0, 0.8, 1.0]),
    ("hit_flash", "Hit flash", 0.0, 1.0, 0.05, [0.0, 0.6, 0.8]),
    (
        "light_actors",
        "Character lighting",
        0.0,
        1.0,
        0.05,
        [0.0, 0.8, 1.0],
    ),
    (
        "light_dynamic",
        "Dynamic lights",
        0.0,
        1.0,
        0.05,
        [0.0, 0.8, 1.0],
    ),
    (
        "light_shadow",
        "Contact shadows",
        0.0,
        1.0,
        0.05,
        [0.0, 0.6, 0.8],
    ),
];

/// Value, profile text and label of each option of a choice setting.
pub type Opts<T> = [(T, &'static str, &'static str)];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Off,
    Reinhard,
    Aces,
    Agx,
    Tony,
    Filmic,
    Neutral,
}

pub const TONES: [(Tone, &str, &str); 7] = [
    (Tone::Off, "none", "NONE"),
    (Tone::Reinhard, "reinhard", "REINHARD"),
    (Tone::Aces, "aces", "ACES"),
    (Tone::Agx, "agx", "AGX"),
    (Tone::Tony, "tony", "TONY"),
    (Tone::Filmic, "filmic", "FILMIC"),
    (Tone::Neutral, "neutral", "NEUTRAL"),
];

impl Tone {
    fn component(self) -> Tonemapping {
        match self {
            Tone::Off => Tonemapping::None,
            Tone::Reinhard => Tonemapping::ReinhardLuminance,
            Tone::Aces => Tonemapping::AcesFitted,
            Tone::Agx => Tonemapping::AgX,
            Tone::Tony => Tonemapping::TonyMcMapface,
            Tone::Filmic => Tonemapping::BlenderFilmic,
            Tone::Neutral => Tonemapping::KhronosPbrNeutral,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Aa {
    Off,
    Fxaa,
    Smaa,
    Msaa2,
    Msaa4,
}

pub const AAS: [(Aa, &str, &str); 5] = [
    (Aa::Off, "off", "OFF"),
    (Aa::Fxaa, "fxaa", "FXAA"),
    (Aa::Smaa, "smaa", "SMAA"),
    (Aa::Msaa2, "msaa2", "MSAA 2X"),
    (Aa::Msaa4, "msaa4", "MSAA 4X"),
];

/// Anisotropic filtering levels and frame limits (0 = none); the text of both is the number.
pub const ANISOS: [(u16, &str, &str); 5] = [
    (1, "1", "OFF"),
    (2, "2", "2X"),
    (4, "4", "4X"),
    (8, "8", "8X"),
    (16, "16", "16X"),
];
pub const FPS: [(u16, &str, &str); 7] = [
    (0, "0", "NONE"),
    (30, "30", "30"),
    (60, "60", "60"),
    (90, "90", "90"),
    (120, "120", "120"),
    (144, "144", "144"),
    (240, "240", "240"),
];

impl Aa {
    fn msaa(self) -> Msaa {
        match self {
            Aa::Msaa2 => Msaa::Sample2,
            Aa::Msaa4 => Msaa::Sample4,
            _ => Msaa::Off,
        }
    }
}

fn key<T: Copy + PartialEq>(t: &Opts<T>, v: T) -> &'static str {
    t.iter().find(|o| o.0 == v).map_or("", |o| o.1)
}

fn find<T: Copy>(t: &Opts<T>, k: &str) -> Option<T> {
    t.iter().find(|o| o.1 == k).map(|o| o.0)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Preset {
    Original,
    Enhanced,
    Ultra,
}

impl Preset {
    pub const ALL: [Preset; 3] = [Preset::Original, Preset::Enhanced, Preset::Ultra];

    pub fn label(self) -> &'static str {
        ["ORIGINAL", "ENHANCED", "ULTRA"][self as usize]
    }

    fn tone(self) -> Tone {
        [Tone::Off, Tone::Tony, Tone::Tony][self as usize]
    }

    fn aa(self) -> Aa {
        [Aa::Msaa4, Aa::Msaa4, Aa::Smaa][self as usize]
    }

    fn aniso(self) -> u16 {
        [1, 8, 16][self as usize]
    }
}

impl std::str::FromStr for Preset {
    type Err = ();

    /// For the `--gfx` flag.
    fn from_str(s: &str) -> Result<Self, ()> {
        Self::ALL
            .into_iter()
            .find(|p| p.label().eq_ignore_ascii_case(s))
            .ok_or(())
    }
}

/// The player's graphics settings (`Profile::graphics`).
#[derive(Clone, Debug, PartialEq)]
pub struct Graphics {
    pub tone: Tone,
    pub aa: Aa,
    pub aniso: u16,
    /// Native build: present in step with the display (the browser always does).
    pub vsync: bool,
    /// Native build: frames per second to stay under, 0 = no limit.
    pub fps: u16,
    pub knobs: [f32; N],
}

impl Default for Graphics {
    /// ENHANCED, the preset new profiles start with.
    fn default() -> Self {
        let mut g = Self {
            tone: Tone::Off,
            aa: Aa::Msaa4,
            aniso: 1,
            vsync: true,
            fps: 0,
            knobs: KNOBS.map(|k| k.5[0]),
        };
        g.preset(Preset::Enhanced);
        g
    }
}

fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

impl Graphics {
    pub fn get(&self, k: Knob) -> f32 {
        self.knobs[k as usize]
    }

    pub fn set(&mut self, k: Knob, v: f32) {
        let (lo, hi) = k.range();
        let s = k.step();
        self.knobs[k as usize] = round2((v / s).round() * s).clamp(lo, hi);
    }

    /// Takes a preset's settings; vsync, the frame limit and the field of view stay.
    pub fn preset(&mut self, p: Preset) {
        let fov = self.get(Knob::Fov);
        self.tone = p.tone();
        self.aa = p.aa();
        self.aniso = p.aniso();
        self.knobs = KNOBS.map(|k| k.5[p as usize]);
        self.knobs[Knob::Fov as usize] = fov;
    }

    /// The preset these settings are (ignoring vsync, frame limit and field of view).
    pub fn which(&self) -> Option<Preset> {
        Preset::ALL.into_iter().find(|&p| {
            let mut q = self.clone();
            q.preset(p);
            q == *self
        })
    }

    /// Whether the camera renders in HDR: bloom and anything the tonemapping pass does need it.
    fn hdr(&self) -> bool {
        self.tone != Tone::Off
            || self.get(Knob::Bloom) > 0.0
            || self.get(Knob::Exposure) != 0.0
            || self.get(Knob::Saturation) != 1.0
    }

    /// Profile lines (`gfx_*`).
    pub fn to_text(&self) -> String {
        let mut t = format!(
            "gfx_tonemap={}\ngfx_aa={}\ngfx_aniso={}\ngfx_vsync={}\ngfx_fps={}\n",
            key(&TONES, self.tone),
            key(&AAS, self.aa),
            self.aniso,
            self.vsync as u8,
            self.fps
        );
        for (k, v) in KNOBS.iter().zip(self.knobs) {
            t += &format!("gfx_{}={}\n", k.0, v);
        }
        t
    }

    /// Takes one profile line; `Ok(false)`: not a graphics key.
    pub fn set_text(&mut self, k: &str, v: &str) -> Result<bool, String> {
        let Some(name) = k.strip_prefix("gfx_") else {
            return Ok(false);
        };
        let bad = || format!("{k}: bad value {v:?}");
        let v = v.trim();
        match name {
            "tonemap" => self.tone = find(&TONES, v).ok_or_else(bad)?,
            "aa" => self.aa = find(&AAS, v).ok_or_else(bad)?,
            "aniso" => self.aniso = find(&ANISOS, v).ok_or_else(bad)?,
            "fps" => self.fps = find(&FPS, v).ok_or_else(bad)?,
            "vsync" => {
                self.vsync = match v {
                    "0" => false,
                    "1" => true,
                    _ => return Err(bad()),
                }
            }
            _ => {
                let i = KNOBS.iter().position(|r| r.0 == name).ok_or_else(bad)?;
                let n = v
                    .parse::<f32>()
                    .ok()
                    .filter(|x| x.is_finite())
                    .ok_or_else(bad)?;
                self.set(Knob::ALL[i], n);
            }
        }
        Ok(true)
    }
}

// ---- applying them ----

/// Parameters of the custom screen effects (`gfx.wgsl`), on the camera while any is active.
#[derive(Component, ExtractComponent, Clone, Copy, ShaderType, Default)]
struct Fx {
    time: f32,
    grain: f32,
    hurt: f32,
    low: f32,
    speed: f32,
    aspect: f32,
    pulse: f32,
    contrast: f32,
    /// Motion blur smear in screen units.
    blur: Vec2,
    pad2: Vec2,
}

impl FullscreenMaterial for Fx {
    fn fragment_shader() -> ShaderRef {
        "embedded://gunz/gfx.wgsl".into()
    }

    /// Before bloom: the passes of this stage order themselves (bevy's own are chained) and
    /// two that do not both swap the main texture in an order the GPU may not follow.
    fn schedule_configs(system: ScheduleConfigs<BoxedSystem>) -> ScheduleConfigs<BoxedSystem> {
        system.in_set(Core3dSystems::PostProcess).before(bloom)
    }
}

pub struct GfxPlugin;

impl Plugin for GfxPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "gfx.wgsl");
        app.add_message::<Damage>()
            .add_plugins(FullscreenMaterialPlugin::<Fx>::default())
            .init_resource::<Panel>()
            .add_systems(
                PreUpdate,
                close.after(InputSystems).before(UiSystems::Focus),
            )
            .add_systems(
                Update,
                (
                    apply.run_if(resource_exists::<Settings>),
                    filtering.run_if(resource_exists::<Settings>),
                    drive.run_if(resource_exists::<Settings>),
                    overlay,
                    clicks,
                    drag,
                    refresh,
                    persist,
                )
                    .chain(),
            );
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(Last, limit);
    }
}

fn put<C: Component>(c: &mut EntityCommands, v: Option<C>) {
    match v {
        Some(v) => c.insert(v),
        None => c.remove::<C>(),
    };
}

/// Puts the settings on the camera: render components, window present mode and field of view.
fn apply(
    profile: Option<Res<Profile>>,
    device: Option<Res<RenderDevice>>,
    mut commands: Commands,
    mut camera: Query<(Entity, &mut Projection), With<Camera3d>>,
    mut window: Query<&mut Window, With<PrimaryWindow>>,
    mut done: Local<Option<(Graphics, Entity)>>,
) {
    let Ok((e, mut projection)) = camera.single_mut() else {
        return;
    };
    let g = profile.map_or_else(Graphics::default, |p| p.graphics.clone());
    if done.as_ref().is_some_and(|d| d.0 == g && d.1 == e) {
        return;
    }
    let k = |k| g.get(k);
    // the field of view is horizontal at 16:9, the game's way of saying it
    if let Projection::Perspective(p) = &mut *projection {
        let want = 2.0 * ((k(Knob::Fov).to_radians() / 2.0).tan() * 9.0 / 16.0).atan();
        // (the camera spawns at 90, which then stays bit for bit what it was)
        if (p.fov - want).abs() > 1e-4 {
            p.fov = want;
        }
    }
    if std::env::var_os("GUNZ_NOVSYNC").is_none()
        && let Ok(mut w) = window.single_mut()
    {
        let want = if g.vsync {
            PresentMode::AutoVsync
        } else {
            PresentMode::AutoNoVsync
        };
        if w.present_mode != want {
            w.present_mode = want;
        }
    }
    let mut c = commands.entity(e);
    put(&mut c, g.hdr().then_some(Hdr));
    let neutral = ColorGradingSection::default();
    let section = ColorGradingSection {
        saturation: k(Knob::Saturation),
        ..neutral
    };
    c.insert((
        g.tone.component(),
        g.aa.msaa(),
        ColorGrading {
            global: ColorGradingGlobal {
                exposure: k(Knob::Exposure),
                ..default()
            },
            shadows: section,
            midtones: section,
            highlights: section,
        },
    ));
    put(&mut c, (g.aa == Aa::Fxaa).then(Fxaa::default));
    put(&mut c, (g.aa == Aa::Smaa).then(Smaa::default));
    put(
        &mut c,
        (k(Knob::Sharpen) > 0.0).then(|| ContrastAdaptiveSharpening {
            enabled: true,
            sharpening_strength: k(Knob::Sharpen),
            denoise: false,
        }),
    );
    // bloom renders to Rg11b10Ufloat, which WebGPU only renders with an optional feature
    // (without it every frame fails and the screen stays black)
    let bloom = device.is_none_or(|d| {
        d.features()
            .contains(WgpuFeatures::RG11B10UFLOAT_RENDERABLE)
    });
    put(
        &mut c,
        (bloom && k(Knob::Bloom) > 0.0).then(|| Bloom {
            intensity: 0.45 * k(Knob::Bloom),
            composite_mode: BloomCompositeMode::Additive,
            prefilter: BloomPrefilter {
                threshold: 0.4,
                threshold_softness: 0.5,
            },
            ..Bloom::NATURAL
        }),
    );
    put(
        &mut c,
        (k(Knob::Vignette) > 0.0).then(|| Vignette {
            intensity: 0.8 * k(Knob::Vignette),
            ..default()
        }),
    );
    // `drive` sets its strength: the setting plus a kick when hit
    put(
        &mut c,
        (k(Knob::Aberration) > 0.0 || k(Knob::HitFlash) > 0.0).then(|| ChromaticAberration {
            intensity: 0.0,
            ..default()
        }),
    );
    put(
        &mut c,
        (k(Knob::Fog) > 0.0).then(|| DistanceFog {
            color: Color::srgb(0.1, 0.11, 0.14),
            falloff: FogFalloff::Exponential {
                density: 0.035 * k(Knob::Fog),
            },
            ..default()
        }),
    );
    *done = Some((g, e));
}

/// Anisotropic filtering of the textures of the map and of the models: applied to the images
/// the materials hold, again whenever materials were added (idempotent, so only new images
/// are touched).
fn filtering(
    profile: Option<Res<Profile>>,
    map: Option<Res<Assets<MapMaterial>>>,
    standard: Option<Res<Assets<StandardMaterial>>>,
    mut commands: Commands,
    mut last: Local<(u16, usize, usize)>,
) {
    let level = profile.map_or(Graphics::default().aniso, |p| p.graphics.aniso);
    let now = (
        level,
        map.map_or(0, |a| a.len()),
        standard.map_or(0, |a| a.len()),
    );
    if *last == now || (last.0 == 0 && level == 1) {
        *last = now;
        return;
    }
    *last = now;
    commands.queue(move |world: &mut World| {
        let mut ids = HashSet::new();
        ids.extend(
            world
                .get_resource::<Assets<MapMaterial>>()
                .into_iter()
                .flat_map(|a| a.iter())
                .map(|m| m.1.diffuse.id()),
        );
        ids.extend(
            world
                .get_resource::<Assets<StandardMaterial>>()
                .into_iter()
                .flat_map(|a| a.iter())
                .filter_map(|m| m.1.base_color_texture.as_ref().map(|t| t.id())),
        );
        let mut images = world.resource_mut::<Assets<Image>>();
        for id in ids {
            let todo = images.get(id).is_some_and(|i| {
                matches!(&i.sampler, ImageSampler::Descriptor(d) if d.anisotropy_clamp != level
                    && d.min_filter == bevy::image::ImageFilterMode::Linear
                    && d.mag_filter == bevy::image::ImageFilterMode::Linear
                    && d.mipmap_filter == bevy::image::ImageFilterMode::Linear)
            });
            if todo
                && let Some(mut i) = images.get_mut(id)
                && let ImageSampler::Descriptor(d) = &mut i.sampler
            {
                d.anisotropy_clamp = level;
            }
        }
    });
}

/// Sleeps out the rest of the frame under the frame limit (windowed native runs).
#[cfg(not(target_arch = "wasm32"))]
fn limit(
    profile: Option<Res<Profile>>,
    shot: Option<Res<crate::view::Shot>>,
    mut next: Local<Option<std::time::Instant>>,
) {
    let fps = profile.map_or(0, |p| p.graphics.fps);
    if fps == 0 || shot.is_some() {
        *next = None;
        return;
    }
    let now = std::time::Instant::now();
    let due = next.map_or(now, |t| {
        t + std::time::Duration::from_secs_f64(1.0 / fps as f64)
    });
    if due > now {
        std::thread::sleep(due - now);
        *next = Some(due);
    } else {
        *next = Some(now);
    }
}

/// What the player's state asks of the effects.
#[derive(Default)]
struct Live {
    hurt: f32,
    low: f32,
    speed: f32,
    turn: Option<Quat>,
}

/// Runs the custom effects: hit flash on damage, low-health drain and heartbeat, speed effects
/// while tumbling or falling fast, motion blur from the camera's turn; the pass stays off the
/// camera while all of them are idle.
#[allow(clippy::too_many_arguments)]
fn drive(
    profile: Option<Res<Profile>>,
    time: Res<Time>,
    real: Res<Time<Real>>,
    mut hits: MessageReader<Damage>,
    player: Query<(Entity, &Vitals, Option<&Motor>, Has<Dead>), With<Player>>,
    mut camera: Query<
        (
            Entity,
            &Transform,
            &Projection,
            Option<&mut Fx>,
            Option<&mut ChromaticAberration>,
        ),
        With<Camera3d>,
    >,
    mut commands: Commands,
    mut live: Local<Live>,
) {
    let Ok((e, transform, projection, fx, aberration)) = camera.single_mut() else {
        return;
    };
    let g = profile.map_or_else(Graphics::default, |p| p.graphics.clone());
    let k = |k| g.get(k);
    let dt = time.delta_secs();
    let ease = |from: f32, to: f32, rate: f32| from + (to - from) * (1.0 - (-rate * dt).exp());
    let me = player.single().ok();
    for h in hits.read() {
        if me.is_some_and(|m| m.0 == h.target) && h.amount > 0.0 {
            live.hurt = live.hurt.max((h.amount / 50.0).clamp(0.4, 1.0));
        }
    }
    live.hurt = (live.hurt - dt * 2.5).max(0.0);
    let (mut low, mut speed) = (0.0, 0.0);
    if let Some((_, v, motor, dead)) = me {
        let hp = v.hp / v.max_hp.max(1.0);
        low = if dead {
            1.0
        } else {
            1.0 - ((hp - 0.08) / 0.27).clamp(0.0, 1.0)
        };
        if let Some(m) = motor {
            let fall = ((-m.vel.y - 14.0) / 14.0).clamp(0.0, 1.0);
            speed = if m.tumble.is_some() {
                0.8f32.max(fall)
            } else {
                fall
            };
        }
    }
    live.low = ease(live.low, low, 5.0);
    live.speed = ease(live.speed, speed, 8.0);
    // the screen smear of the turn since last frame: where the old view centre now appears
    let mut blur = Vec2::ZERO;
    let turned = transform.rotation;
    if let (Some(before), Projection::Perspective(p), true) =
        (live.turn, projection, k(Knob::MotionBlur) > 0.0 && dt > 0.0)
    {
        let seen = turned.inverse() * (before * Vec3::NEG_Z);
        if seen.z < -0.2 {
            let tan_v = (p.fov / 2.0).tan();
            let shift = Vec2::new(
                seen.x / (-seen.z) / (tan_v * p.aspect_ratio),
                -seen.y / (-seen.z) / tan_v,
            ) * 0.5;
            // a shutter open for a quarter of a 60 Hz frame at full strength, whatever the frame rate
            let shutter = k(Knob::MotionBlur) * (1.0 / 240.0);
            blur = (shift / dt * shutter).clamp_length_max(0.03);
        }
    }
    live.turn = Some(turned);
    let heartbeat = (real.elapsed_secs() * TAU * 1.3).sin().max(0.0).powi(3);
    let want = Fx {
        time: real.elapsed_secs() % 1000.0,
        grain: k(Knob::Grain),
        contrast: k(Knob::Contrast),
        hurt: live.hurt * k(Knob::HitFlash),
        low: live.low * k(Knob::LowHealth),
        speed: live.speed * k(Knob::SpeedFx),
        aspect: match projection {
            Projection::Perspective(p) => p.aspect_ratio,
            _ => 16.0 / 9.0,
        },
        pulse: heartbeat
            * live.low
            * k(Knob::LowHealth)
            * if me.is_some_and(|m| m.3) { 0.0 } else { 1.0 },
        blur,
        ..default()
    };
    let active = want.grain > 0.0
        || want.contrast != 1.0
        || want.hurt > 0.005
        || want.low > 0.005
        || want.speed > 0.005
        || blur.length() > 0.0004;
    match (active, fx) {
        (true, Some(mut f)) => *f = want,
        (true, None) => {
            commands.entity(e).insert(want);
        }
        (false, Some(_)) => {
            commands.entity(e).remove::<Fx>();
        }
        _ => {}
    }
    if let Some(mut a) = aberration {
        a.intensity = 0.012 * k(Knob::Aberration) + 0.012 * want.hurt;
    }
}

// ---- the GRAPHICS panel ----

/// Present while the pause menu's GRAPHICS overlay is open.
#[derive(Resource)]
pub struct Overlay;

#[derive(Resource, Default)]
struct Panel {
    /// The slider being dragged.
    drag: Option<Entity>,
}

#[derive(Component, Clone, Copy, PartialEq)]
enum Ctl {
    Preset(Preset),
    Step(Knob, i8),
    Tone(Tone),
    Aa(Aa),
    Aniso(u16),
    Fps(u16),
    Vsync,
    Done,
}

#[derive(Component)]
struct Slider(Knob);

#[derive(Component)]
struct Fill(Knob);

#[derive(Component, Clone, Copy)]
enum Show {
    Knob(Knob),
    Preset,
}

#[derive(Component)]
struct OverlayRoot;

fn text(s: &str, size: f32, color: Color) -> impl Bundle {
    (
        Text::new(s),
        TextFont::from_font_size(size),
        TextColor(color),
    )
}

fn row() -> Node {
    Node {
        align_items: AlignItems::Center,
        column_gap: px(6),
        ..default()
    }
}

/// The panel's three cards: the image, the screen effects, the preset and display.
pub fn fill(p: &mut ChildSpawnerCommands, done: bool) {
    p.spawn(Node {
        column_gap: px(16),
        align_items: AlignItems::Stretch,
        ..default()
    })
    .with_children(|r| {
        r.spawn(panel(398.0, AlignItems::FlexStart))
            .with_children(image);
        r.spawn(panel(398.0, AlignItems::FlexStart))
            .with_children(effects);
        r.spawn(panel(398.0, AlignItems::FlexStart))
            .with_children(|d| display(d, done));
    });
}

fn slider(m: &mut ChildSpawnerCommands, k: Knob) {
    m.spawn(row()).with_children(|r| {
        r.spawn((
            Node {
                width: px(138),
                ..default()
            },
            children![text(k.label(), 14.0, Color::WHITE)],
        ));
        r.spawn((
            Button,
            Slider(k),
            RelativeCursorPosition::default(),
            Node {
                width: px(96),
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
                    border_radius: BorderRadius::all(px(3)),
                    ..default()
                },
                BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.15)),
                children![(
                    Fill(k),
                    Node {
                        height: percent(100),
                        border_radius: BorderRadius::all(px(3)),
                        ..default()
                    },
                    BackgroundColor(ACCENT),
                )],
            ));
        });
        r.spawn(button(22.0, 24.0, "-", 15.0, Ctl::Step(k, -1)));
        r.spawn(button(22.0, 24.0, "+", 15.0, Ctl::Step(k, 1)));
        r.spawn((Show::Knob(k), text("", 14.0, ACCENT)));
    });
}

/// A titled group of buttons, one per option, the current one marked.
fn choices<T: Copy>(
    m: &mut ChildSpawnerCommands,
    title: &str,
    opts: &Opts<T>,
    w: f32,
    ctl: fn(T) -> Ctl,
) {
    m.spawn(text(title, 14.0, DIM));
    m.spawn(Node {
        flex_wrap: FlexWrap::Wrap,
        width: percent(100),
        column_gap: px(6),
        row_gap: px(6),
        margin: UiRect::bottom(px(4)),
        ..default()
    })
    .with_children(|c| {
        for &(v, _, label) in opts {
            c.spawn(button(w, 24.0, label, 12.0, ctl(v)));
        }
    });
}

fn image(m: &mut ChildSpawnerCommands) {
    m.spawn(heading("IMAGE"));
    choices(m, "Tonemapping", &TONES, 86.0, Ctl::Tone);
    for k in [
        Knob::Exposure,
        Knob::Contrast,
        Knob::Saturation,
        Knob::Bloom,
    ] {
        slider(m, k);
    }
    choices(m, "Anti-aliasing", &AAS, 86.0, Ctl::Aa);
    slider(m, Knob::Sharpen);
    choices(m, "Anisotropic filtering", &ANISOS, 66.0, Ctl::Aniso);
    slider(m, Knob::Fog);
}

fn effects(m: &mut ChildSpawnerCommands) {
    m.spawn(heading("SCREEN EFFECTS"));
    for k in [
        Knob::Vignette,
        Knob::Grain,
        Knob::Aberration,
        Knob::MotionBlur,
        Knob::SpeedFx,
        Knob::LowHealth,
        Knob::HitFlash,
    ] {
        slider(m, k);
    }
    m.spawn((
        Node {
            width: px(366),
            margin: UiRect::top(px(8)),
            ..default()
        },
        children![text(
            "An effect set to OFF costs nothing. Motion blur follows the camera's turn; speed effects (radial blur, streaks) a tumble or a fast fall; low health drains the colour and throbs; a hit flashes red. The HUD is never touched.",
            13.0,
            DIM
        )],
    ));
}

fn display(m: &mut ChildSpawnerCommands, done: bool) {
    m.spawn(heading("PRESET"));
    m.spawn(row()).with_children(|r| {
        for p in Preset::ALL {
            r.spawn(button(112.0, 34.0, p.label(), 15.0, Ctl::Preset(p)));
        }
    });
    m.spawn((
        Node {
            width: px(366),
            min_height: px(54),
            ..default()
        },
        children![(Show::Preset, text("", 13.0, DIM))],
    ));
    m.spawn(heading("LIGHTING"));
    for k in [Knob::ActorLight, Knob::DynLight, Knob::Shadow] {
        slider(m, k);
    }
    m.spawn(heading("DISPLAY"));
    slider(m, Knob::Fov);
    // the browser always presents in step with the display and cannot be limited
    if !cfg!(target_arch = "wasm32") {
        m.spawn(row()).with_children(|r| {
            r.spawn((
                Node {
                    width: px(138),
                    ..default()
                },
                children![text("Vertical sync", 14.0, Color::WHITE)],
            ));
            r.spawn(button(64.0, 24.0, "", 13.0, Ctl::Vsync));
        });
        choices(m, "Frame limit", &FPS, 44.0, Ctl::Fps);
    }
    if done {
        m.spawn(Node {
            margin: UiRect::top(Val::Auto),
            align_self: AlignSelf::FlexEnd,
            ..default()
        })
        .with_children(|f| {
            f.spawn(primary(130.0, 34.0, "DONE", 18.0, Ctl::Done));
        });
    }
}

/// Closes the overlay on Esc (before the pause menu sees the key, which stays open).
fn close(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    open: Option<Res<Overlay>>,
    mut commands: Commands,
) {
    if open.is_some() && keys.just_pressed(KeyCode::Escape) {
        keys.clear_just_pressed(KeyCode::Escape);
        commands.remove_resource::<Overlay>();
    }
}

/// Spawns the pause menu's overlay while [`Overlay`] is present and the game is paused.
fn overlay(
    mut commands: Commands,
    open: Option<Res<Overlay>>,
    frozen: Option<Res<crate::game::Frozen>>,
    camera: Query<Entity, With<Camera3d>>,
    roots: Query<Entity, With<OverlayRoot>>,
) {
    if open.is_some() && frozen.is_none() {
        commands.remove_resource::<Overlay>();
    }
    match (open.is_some() && frozen.is_some(), roots.single()) {
        (false, Ok(e)) => commands.entity(e).despawn(),
        (true, Err(_)) => {
            let Ok(camera) = camera.single() else {
                return;
            };
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
                    // opaque enough that the HUD under it does not read through
                    BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.85)),
                ))
                .with_children(|r| fill(r, true));
        }
        _ => {}
    }
}

fn clicks(
    q: Query<(&Interaction, &Ctl), Changed<Interaction>>,
    profile: Option<ResMut<Profile>>,
    mut commands: Commands,
) {
    let Some(mut profile) = profile else { return };
    for (i, c) in &q {
        if *i != Interaction::Pressed {
            continue;
        }
        let g = &mut profile.graphics;
        match *c {
            Ctl::Preset(p) => g.preset(p),
            Ctl::Step(k, d) => g.set(k, g.get(k) + d as f32 * k.step()),
            Ctl::Tone(t) => g.tone = t,
            Ctl::Aa(a) => g.aa = a,
            Ctl::Aniso(n) => g.aniso = n,
            Ctl::Fps(n) => g.fps = n,
            Ctl::Vsync => g.vsync = !g.vsync,
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
        let before = profile.graphics.get(s.0);
        let mut g = profile.graphics.clone();
        g.set(s.0, s.0.at((n.x + 0.5).clamp(0.0, 1.0)));
        if g.get(s.0) != before {
            profile.graphics = g;
        }
    }
}

/// Keeps the panel's values, slider fills and marked options in line with the profile (only
/// writes what changed).
#[allow(clippy::type_complexity)]
fn refresh(
    profile: Option<Res<Profile>>,
    mut shows: Query<(&Show, &mut Text)>,
    mut fills: Query<(&Fill, &mut Node)>,
    buttons: Query<(Entity, &Ctl, &Children, Has<Chosen>)>,
    mut texts: Query<&mut Text, Without<Show>>,
    mut commands: Commands,
) {
    if fills.is_empty() {
        return;
    }
    let default = Graphics::default();
    let g = profile.as_deref().map_or(&default, |p| &p.graphics);
    for (s, mut t) in &mut shows {
        let want = match *s {
            Show::Knob(k) => k.text(g.get(k)),
            Show::Preset => match g.which() {
                Some(Preset::Original) => "ORIGINAL: no tonemapping, no effects, no lighting, 4x MSAA; the image as the game always drew it.".into(),
                Some(Preset::Enhanced) => "ENHANCED: HDR with tonemapping, bloom, sharpening, vignette, lit characters, muzzle-flash and explosion lights, contact shadows.".into(),
                Some(Preset::Ultra) => "ULTRA: all of that, stronger, plus SMAA, 16x filtering, fog, film grain, chromatic aberration and motion blur.".into(),
                None => "CUSTOM: your own mix. A preset keeps the field of view, vsync and frame limit.".to_owned(),
            },
        };
        if t.0 != want {
            t.0 = want;
        }
    }
    for (f, mut n) in &mut fills {
        let w = percent(f.0.pos(g.get(f.0)).clamp(0.0, 1.0) * 100.0);
        if n.width != w {
            n.width = w;
        }
    }
    for (e, ctl, kids, chosen) in &buttons {
        let on = match *ctl {
            Ctl::Preset(p) => g.which() == Some(p),
            Ctl::Tone(t) => g.tone == t,
            Ctl::Aa(a) => g.aa == a,
            Ctl::Aniso(n) => g.aniso == n,
            Ctl::Fps(n) => g.fps == n,
            Ctl::Vsync => {
                if let Some(&kid) = kids.first()
                    && let Ok(mut t) = texts.get_mut(kid)
                {
                    let want = if g.vsync { "ON" } else { "OFF" };
                    if t.0 != want {
                        t.0 = want.into();
                    }
                }
                g.vsync
            }
            _ => continue,
        };
        if on != chosen {
            match on {
                true => commands.entity(e).insert(Chosen),
                false => commands.entity(e).remove::<Chosen>(),
            };
        }
    }
}

/// Saves the profile once a graphics change settles (the main menu does not save it on every
/// change the way a match does; a slider being dragged waits for the release).
fn persist(
    mouse: Res<ButtonInput<MouseButton>>,
    profile: Option<Res<Profile>>,
    mut saved: Local<Option<Graphics>>,
) {
    let Some(p) = profile else { return };
    match &*saved {
        None => *saved = Some(p.graphics.clone()),
        Some(g) if *g != p.graphics && !mouse.pressed(MouseButton::Left) => {
            p.save();
            *saved = Some(p.graphics.clone());
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_recognised_and_survive_the_profile_text() {
        let mut g = Graphics::default();
        assert_eq!(g.which(), Some(Preset::Enhanced));
        g.preset(Preset::Original);
        assert_eq!(g.which(), Some(Preset::Original));
        assert!(
            !g.hdr(),
            "ORIGINAL renders in plain 8-bit like the game always did"
        );
        g.preset(Preset::Ultra);
        g.set(Knob::Fov, 103.4);
        g.vsync = false;
        g.fps = 144;
        assert_eq!(
            g.which(),
            Some(Preset::Ultra),
            "fov, vsync and fps are no part of a preset"
        );
        g.set(Knob::Exposure, -0.5);
        assert_eq!(g.which(), None);
        let mut back = Graphics::default();
        for line in g.to_text().lines() {
            let (k, v) = line.split_once('=').unwrap();
            assert_eq!(back.set_text(k, v), Ok(true), "{line}");
        }
        assert_eq!(back, g);
        assert_eq!(back.get(Knob::Fov), 103.0);
        assert_eq!(Graphics::default().set_text("mouse_sens", "1"), Ok(false));
        assert!(Graphics::default().set_text("gfx_aa", "taa").is_err());
        assert!(Graphics::default().set_text("gfx_bloom", "NaN").is_err());
        assert!(Graphics::default().set_text("gfx_nope", "1").is_err());
        assert_eq!("ultra".parse(), Ok(Preset::Ultra));
    }
}
