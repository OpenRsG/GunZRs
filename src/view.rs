//! Shared Bevy viewer plumbing for the `gunz*` binaries: coordinate conversion, texture lookup
//! and decoding, windowed fly camera or headless `--shot` capture.

use crate::mrs::Vfs;
use bevy::{
    app::ScheduleRunnerPlugin,
    asset::RenderAssetUsages,
    camera::{
        RenderTarget,
        visibility::{InheritedVisibility, ViewVisibility},
    },
    core_pipeline::tonemapping::Tonemapping,
    image::{CompressedImageFormats, ImageSampler, ImageType},
    input::mouse::AccumulatedMouseMotion,
    light::cluster::GlobalClusterSettings,
    prelude::*,
    render::{
        render_resource::TextureFormat,
        view::{
            NoIndirectDrawing,
            screenshot::{Screenshot, save_to_disk},
        },
    },
    window::{ExitCondition, PresentMode},
    winit::WinitPlugin,
};
use std::{collections::HashMap, time::Duration};

/// GunZ units are centimetres; Bevy scenes here use metres.
pub const SCALE: f32 = 0.01;

/// Left-handed Z-up -> right-handed Y-up (swapping Y/Z mirrors, so D3D winding stays front-facing).
pub fn to_bevy([x, y, z]: [f32; 3]) -> [f32; 3] {
    [x, z, y]
}

/// Removes `--shot OUT.png` from `args`. `Err` means the flag had no value.
pub fn take_shot_arg(args: &mut Vec<String>) -> Result<Option<String>, ()> {
    match args.iter().position(|a| a == "--shot") {
        Some(i) if i + 1 < args.len() => {
            let path = args.remove(i + 1);
            args.remove(i);
            Ok(Some(path))
        }
        Some(_) => Err(()),
        None => Ok(None),
    }
}

/// Windowed app with a fly camera, or (with `shot`) a headless app that renders the
/// `Camera3d` spawned by [`spawn_camera`] to `shot` at frame [`Shot::capture`] (default 120)
/// and exits 60 frames later.
pub fn app(title: &str, shot: Option<String>) -> App {
    let windowed = shot.is_none();
    let mut app = app_plain(title, shot);
    if windowed {
        app.add_systems(Update, fly_camera);
    }
    app
}

/// [`app`] without the fly camera, for binaries that drive the camera themselves.
pub fn app_plain(title: &str, shot: Option<String>) -> App {
    let mut app = App::new();
    // `GUNZ_NOVSYNC=1`: windowed runs present immediately; headless `--shot` runs drop the
    // 60 Hz loop wait, so frame times are the real work (simulation stays 1/60 s per frame).
    let novsync = std::env::var_os("GUNZ_NOVSYNC").is_some();
    let wait = if novsync {
        Duration::ZERO
    } else {
        Duration::from_secs_f64(1.0 / 60.0)
    };
    let present_mode = if novsync {
        PresentMode::AutoNoVsync
    } else {
        PresentMode::default()
    };
    let plugins = DefaultPlugins.set(WindowPlugin {
        primary_window: shot.is_none().then(|| Window {
            title: format!("Gunz2Rust - {title}"),
            present_mode,
            // the browser build draws into `<canvas id="gunz">` and follows its size
            canvas: Some("#gunz".into()),
            fit_canvas_to_parent: true,
            ..default()
        }),
        exit_condition: if shot.is_some() {
            ExitCondition::DontExit
        } else {
            ExitCondition::OnPrimaryClosed
        },
        ..default()
    });
    match shot {
        Some(path) => app
            .add_plugins((
                plugins.disable::<WinitPlugin>(),
                ScheduleRunnerPlugin::run_loop(wait),
            ))
            .insert_resource(Shot {
                path,
                frame: 0,
                capture: 120,
            })
            .add_systems(Update, take_shot),
        None => app.add_plugins(plugins),
    };
    app.insert_resource(ClearColor(Color::BLACK));
    // No lights anywhere: the GPU light-clustering passes only cost render-thread time
    // (Castle 1.26 -> 1.10 ms). `ClusterConfig::None` is not an option, Bevy 0.19 then
    // creates a zero-sized texture.
    app.add_systems(Startup, |mut s: ResMut<GlobalClusterSettings>| {
        s.gpu_clustering = None
    });
    app
}

/// A mesh that is shown but outside every view (frustum-culled). Animating its assets is
/// wasted work; hidden ones still update, so a fade-in never starts from a stale state.
pub fn culled(inherited: &InheritedVisibility, view: &ViewVisibility) -> bool {
    inherited.get() && !view.get()
}

/// Present in headless `--shot` runs.
#[derive(Resource)]
pub struct Shot {
    path: String,
    frame: u32,
    /// Frame at which the image is captured; later frames still render and the app exits
    /// 60 frames after it.
    pub capture: u32,
}

/// Waits for pipelines to compile, captures the camera target, then exits.
fn take_shot(
    mut commands: Commands,
    mut shot: ResMut<Shot>,
    camera: Single<&RenderTarget, With<Camera3d>>,
    mut exit: MessageWriter<AppExit>,
) {
    shot.frame += 1;
    let RenderTarget::Image(target) = *camera else {
        return;
    };
    // `GUNZ_SEQ=SECS` with a `%` in the path: also save every 3rd frame (20 fps) of the SECS
    // before the capture frame, `%` replaced by the frame number (for demo GIFs).
    let span = std::env::var("GUNZ_SEQ")
        .ok()
        .and_then(|s| s.parse::<f32>().ok())
        .filter(|_| shot.path.contains('%'))
        .map_or(0, |s| (s * 60.0) as u32);
    let lead = shot.capture.saturating_sub(shot.frame);
    if shot.frame <= shot.capture && lead <= span && lead % 3 == 0 {
        let path = shot.path.replace('%', &format!("{:05}", shot.frame));
        commands
            .spawn(Screenshot::image(target.handle.clone()))
            .observe(save_to_disk(path));
    } else if shot.frame == shot.capture + 60 {
        exit.write(AppExit::Success);
    }
}

/// Size of a headless `--shot` image: 1280x720, or `GUNZ_SHOT_SIZE=WxH` (to check small windows).
pub fn shot_size() -> (u32, u32) {
    std::env::var("GUNZ_SHOT_SIZE")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .filter(|&(w, h)| w > 0 && h > 0)
        .unwrap_or((1280, 720))
}

/// Spawns the viewer camera (GunZ's horizontal FOV 90 at 16:9). In `--shot` runs it renders
/// into a [`shot_size`] sRGB image instead of the (absent) window.
pub fn spawn_camera<'a>(
    commands: &'a mut Commands,
    images: &mut Assets<Image>,
    shot: bool,
    eye: Vec3,
    dir: Vec3,
) -> EntityCommands<'a> {
    let mut camera = commands.spawn((
        Camera3d::default(),
        Tonemapping::None,
        Projection::Perspective(PerspectiveProjection {
            fov: 2.0 * (9.0f32 / 16.0).atan(),
            near: 0.05,
            ..default()
        }),
        Transform::from_translation(eye).looking_to(dir, Vec3::Y),
    ));
    // Unlit scenes of a few hundred meshes: plain draws and CPU preprocessing beat the GPU
    // indirect-draw setup (render thread 1.03 -> 0.95 ms on Castle with 8 bots).
    camera.insert(NoIndirectDrawing);
    if shot {
        let (w, h) = shot_size();
        let target = Image::new_target_texture(w, h, TextureFormat::Rgba8UnormSrgb, None);
        camera.insert(RenderTarget::Image(images.add(target).into()));
    }
    camera
}

fn fly_camera(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    mut camera: Single<&mut Transform, With<Camera3d>>,
    mut exit: MessageWriter<AppExit>,
) {
    if keys.just_pressed(KeyCode::Escape) {
        exit.write(AppExit::Success);
    }
    if buttons.pressed(MouseButton::Right) {
        let (yaw, pitch, _) = camera.rotation.to_euler(EulerRot::YXZ);
        let yaw = yaw - motion.delta.x * 0.003;
        let pitch = (pitch - motion.delta.y * 0.003).clamp(-1.54, 1.54);
        camera.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
    }
    let axis = |pos: KeyCode, neg: KeyCode| {
        keys.pressed(pos) as i32 as f32 - keys.pressed(neg) as i32 as f32
    };
    let (forward, right) = (camera.forward(), camera.right());
    let wish = forward * axis(KeyCode::KeyW, KeyCode::KeyS)
        + right * axis(KeyCode::KeyD, KeyCode::KeyA)
        + Vec3::Y * axis(KeyCode::Space, KeyCode::KeyC);
    let speed = if keys.pressed(KeyCode::ShiftLeft) {
        30.0
    } else {
        6.0
    };
    camera.translation += wish.normalize_or_zero() * speed * time.delta_secs();
}

/// Decodes DDS/BMP/TGA/JPEG bytes. `srgb: false` keeps raw values (map.wgsl does gamma math);
/// use `true` for textures fed to Bevy's StandardMaterial.
pub fn decode(bytes: &[u8], ext: &str, srgb: bool, sampler: ImageSampler) -> Option<Image> {
    Image::from_buffer(
        bytes,
        ImageType::Extension(ext),
        CompressedImageFormats::BC,
        srgb,
        sampler,
        RenderAssetUsages::RENDER_WORLD,
    )
    .map_err(|e| warn!("decode .{ext}: {e}"))
    .ok()
}

/// File name -> VFS path for every archived file; names under `prefer` win over duplicates.
pub fn file_index(vfs: &Vfs, prefer: &str) -> HashMap<String, String> {
    let mut by_name = HashMap::new();
    for p in vfs.paths() {
        let file = p.rsplit('/').next().unwrap().to_string();
        if p.starts_with(prefer) || !by_name.contains_key(&file) {
            by_name.insert(file, p.to_string());
        }
    }
    by_name
}

/// Resolves a texture name (may be `../Other/x.dds`) against `dir` (VFS directory ending in
/// `/`), then any archive by file name; `.bmp`/`.tga` references fall back to a `.dds` twin.
pub fn texture_path(
    vfs: &Vfs,
    by_name: &HashMap<String, String>,
    dir: &str,
    name: &str,
) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    let joined = format!("{dir}{}", crate::mrs::normalize(name));
    for part in joined.split('/') {
        match part {
            ".." => drop(parts.pop()),
            "." | "" => {}
            p => parts.push(p),
        }
    }
    let path = parts.join("/");
    let dds = path.rsplit_once('.').map(|(stem, _)| format!("{stem}.dds"));
    [Some(path), dds].into_iter().flatten().find_map(|p| {
        if vfs.exists(&p) {
            return Some(p);
        }
        by_name.get(p.rsplit('/').next().unwrap()).cloned()
    })
}
