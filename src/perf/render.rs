//! Render-thread schedule timings and scene counts for `GUNZ_FRAMETIMES` (see the parent module).
//!
//! A stamp system runs between every two [`RenderSystems`] sets of the render sub-app's
//! `Render` schedule; the p50/p99 of each stretch and of the whole schedule are printed at
//! exit, with entity, visible-mesh and light counts.

use super::{WARM, percentile};
use bevy::{
    camera::visibility::ViewVisibility,
    prelude::*,
    render::{Render, RenderApp, RenderSystems as S},
    ui::Node,
};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::Instant,
};

const SETS: [(S, &str); 12] = [
    (S::ExtractCommands, "extract_cmds"),
    (S::PrepareAssets, "prep_assets"),
    (S::PrepareMeshes, "prep_meshes"),
    (S::CreateViews, "create_views"),
    (S::Specialize, "specialize"),
    (S::PrepareViews, "prep_views"),
    (S::Queue, "queue"),
    (S::PhaseSort, "phase_sort"),
    (S::Prepare, "prepare"),
    (S::Render, "render"),
    (S::Cleanup, "cleanup"),
    (S::PostCleanup, "post_cleanup"),
];

#[derive(Default)]
struct Times {
    at: Option<Instant>,
    /// Per frame: ms spent in each set (index i = set i), then the whole schedule.
    frames: Vec<[f32; SETS.len() + 1]>,
    cur: [f32; SETS.len() + 1],
}

#[derive(Resource, Clone, Default)]
struct Stamps(Arc<Mutex<Times>>);

impl Stamps {
    /// Stamp `k` runs just after set `k - 1` (stamp 0 before the first set).
    fn stamp(&self, k: usize) {
        let now = Instant::now();
        let mut t = self.0.lock().unwrap();
        if k == 0 {
            t.cur = Default::default();
        } else if let Some(at) = t.at {
            t.cur[k - 1] = now.duration_since(at).as_secs_f32() * 1e3;
        }
        t.at = Some(now);
        if k == SETS.len() {
            let mut cur = t.cur;
            cur[SETS.len()] = cur[..SETS.len()].iter().sum();
            t.frames.push(cur);
        }
    }
}

pub fn build(app: &mut App) {
    let stamps = Stamps::default();
    let Some(render) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render.insert_resource(stamps.clone());
    for k in 0..=SETS.len() {
        let stamp = move |s: Res<Stamps>| s.stamp(k);
        let mut cfg = stamp.into_configs();
        if k > 0 {
            cfg = cfg.after(SETS[k - 1].0.clone());
        }
        if k < SETS.len() {
            cfg = cfg.before(SETS[k].0.clone());
        }
        render.add_systems(Render, cfg);
    }
    // GPU pass timestamps add queries to every frame, so they are opt-in.
    if std::env::var_os("GUNZ_GPUTIME").is_some() {
        app.add_plugins(bevy::render::diagnostic::RenderDiagnosticsPlugin);
    }
    app.insert_resource(stamps)
        .add_systems(Last, (summary.after(super::summary), scene, churn));
}

fn summary(
    mut exit: MessageReader<AppExit>,
    mut mat_events: MessageReader<AssetEvent<StandardMaterial>>,
    mut mesh_events: MessageReader<AssetEvent<Mesh>>,
    mut map_events: MessageReader<AssetEvent<crate::level::MapMaterial>>,
    mut modified: Local<[usize; 4]>,
    mut by_asset: Local<std::collections::HashMap<AssetId<StandardMaterial>, usize>>,
    names: Query<(Entity, &MeshMaterial3d<StandardMaterial>)>,
    name_of: Query<&Name>,
    diagnostics: Res<bevy::diagnostic::DiagnosticsStore>,
    parents: Query<&ChildOf>,
    stamps: Res<Stamps>,
) {
    let mut by_id = |id: AssetId<StandardMaterial>| *by_asset.entry(id).or_insert(0usize) += 1;
    for e in mat_events.read() {
        if let AssetEvent::Modified { id } = e {
            by_id(*id);
            modified[0] += 1;
        }
    }
    modified[3] += map_events
        .read()
        .filter(|e| matches!(e, AssetEvent::Modified { .. }))
        .count();
    modified[1] += mesh_events
        .read()
        .filter(|e| matches!(e, AssetEvent::Modified { .. }))
        .count();
    modified[2] += 1;
    if exit.read().next().is_none() {
        return;
    }
    eprintln!(
        "[perf] asset Modified events per frame: StandardMaterial={:.2} Mesh={:.2} MapMaterial={:.2}",
        modified[0] as f32 / modified[2] as f32,
        modified[1] as f32 / modified[2] as f32,
        modified[3] as f32 / modified[2] as f32
    );
    let mut gpu: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.path().as_str().ends_with("elapsed_gpu"))
        .filter_map(|d| Some((d.path().as_str().to_string(), d.average()?)))
        .collect();
    gpu.sort_by(|a, b| b.1.total_cmp(&a.1));
    let gpu_sum: f64 = gpu
        .iter()
        .filter(|(p, _)| p.matches('/').count() == 2)
        .map(|g| g.1)
        .sum();
    if !gpu.is_empty() {
        eprintln!("[perf] gpu passes (avg ms, top-level sum {gpu_sum:.3}):");
        for (p, ms) in gpu.iter().take(8) {
            eprintln!("[perf]   {ms:.3} {p}");
        }
    }
    let owners: std::collections::HashMap<_, _> =
        names.iter().map(|(e, m, ..)| (m.0.id(), e)).collect();
    let mut groups: std::collections::HashMap<String, (usize, usize)> = Default::default();
    for (id, n) in by_asset.iter() {
        let mut chain = Vec::new();
        let mut cur = owners.get(id).copied();
        while let Some(e) = cur.filter(|_| chain.len() < 6) {
            chain.push(name_of.get(e).map_or("-", Name::as_str).to_string());
            cur = parents.get(e).ok().map(ChildOf::parent);
        }
        let g = groups.entry(chain.join(" < ")).or_default();
        *g = (g.0 + n, g.1 + 1);
    }
    let mut top: Vec<_> = groups.into_iter().collect();
    top.sort_by_key(|(_, (n, _))| std::cmp::Reverse(*n));
    for (label, (n, k)) in top.into_iter().take(12) {
        eprintln!("[perf]   {k} materials modified {n}x total: {label}");
    }
    let t = stamps.0.lock().unwrap();
    let warm = WARM.min(t.frames.len());
    if t.frames.len() > warm {
        let play = &t.frames[warm..];
        let col = |i: usize| {
            let mut v: Vec<f32> = play.iter().map(|f| f[i]).collect();
            v.sort_by(f32::total_cmp);
            format!("{:.2}/{:.2}", percentile(&v, 0.5), percentile(&v, 0.99))
        };
        let sets: Vec<String> = SETS
            .iter()
            .enumerate()
            .map(|(i, (_, n))| format!("{n}={}", col(i)))
            .collect();
        eprintln!(
            "[perf] render schedule p50/p99 ms: total={} {}",
            col(SETS.len()),
            sets.join(" ")
        );
    }
}

/// Components written per frame (change detection): every one costs transform propagation,
/// visibility, UI layout or render extraction downstream, even when the value is unchanged.
fn churn(
    mut exit: MessageReader<AppExit>,
    mut sums: Local<[usize; 6]>,
    transform: Query<(), Changed<Transform>>,
    visibility: Query<(), Changed<Visibility>>,
    ui: Query<(), Changed<Node>>,
    text: Query<(), Changed<Text>>,
    colour: Query<(), Changed<BackgroundColor>>,
    moved: Query<Entity, Changed<Transform>>,
    name_of: Query<&Name>,
    parents: Query<&ChildOf>,
) {
    sums[0] += transform.iter().count();
    sums[1] += visibility.iter().count();
    sums[2] += ui.iter().count();
    sums[3] += text.iter().count();
    sums[4] += colour.iter().count();
    sums[5] += 1;
    if exit.read().next().is_some() {
        let n = sums[5] as f32;
        eprintln!(
            "[perf] changed per frame: Transform={:.1} Visibility={:.1} ui Node={:.1} Text={:.1} BackgroundColor={:.1}",
            sums[0] as f32 / n,
            sums[1] as f32 / n,
            sums[2] as f32 / n,
            sums[3] as f32 / n,
            sums[4] as f32 / n
        );
        let mut by_root: std::collections::HashMap<String, usize> = Default::default();
        for e in &moved {
            let mut top = e;
            while let Ok(p) = parents.get(top) {
                top = p.parent();
            }
            let key = name_of.get(top).map_or("-".to_string(), |n| {
                n.as_str()
                    .trim_end_matches(|c: char| c.is_ascii_digit() || c == '_')
                    .to_string()
            });
            *by_root.entry(key).or_default() += 1;
        }
        let mut roots: Vec<_> = by_root.into_iter().collect();
        roots.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        let roots: Vec<String> = roots
            .iter()
            .take(10)
            .map(|(k, n)| format!("{k}={n}"))
            .collect();
        eprintln!(
            "[perf] Transform changed in the last frame by root: {}",
            roots.join(" ")
        );
    }
}

/// Entity, mesh and material counts of the scene at exit.
fn scene(
    mut exit: MessageReader<AppExit>,
    mesh_owner: Query<(Entity, &Mesh3d)>,
    level: Query<&ViewVisibility, (With<crate::game::MapEntity>, With<Mesh3d>)>,
    name_of: Query<&Name>,
    parents: Query<&ChildOf>,
    all: Query<&ViewVisibility, With<Mesh3d>>,
    nodes: Query<(), With<Node>>,
    standard: Query<&MeshMaterial3d<StandardMaterial>>,
    entities: Query<Entity>,
) {
    if exit.read().next().is_none() {
        return;
    }
    let visible = all.iter().filter(|v| v.get()).count();
    let uniq_mesh = mesh_owner
        .iter()
        .map(|m| m.1.0.id())
        .collect::<HashSet<_>>()
        .len();
    let uniq_mat = standard
        .iter()
        .map(|m| m.0.id())
        .collect::<HashSet<_>>()
        .len();
    eprintln!(
        "[perf] scene: entities={} mesh3d={} visible={visible} level_meshes={} (visible {}) unique_meshes={uniq_mesh} std_materials_used={uniq_mat} ui_nodes={}",
        entities.iter().count(),
        all.iter().count(),
        level.iter().count(),
        level.iter().filter(|v| v.get()).count(),
        nodes.iter().count(),
    );
    let mut by_root: std::collections::HashMap<String, usize> = Default::default();
    for (e, _) in &mesh_owner {
        let mut top = e;
        while let Ok(p) = parents.get(top) {
            top = p.parent();
        }
        let key = name_of.get(top).map_or("-".to_string(), |n| {
            n.as_str()
                .trim_end_matches(|c: char| c.is_ascii_digit() || c == '_')
                .to_string()
        });
        *by_root.entry(key).or_default() += 1;
    }
    let mut roots: Vec<_> = by_root.into_iter().collect();
    roots.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let roots: Vec<String> = roots
        .iter()
        .take(14)
        .map(|(k, n)| format!("{k}={n}"))
        .collect();
    eprintln!("[perf] mesh entities by root: {}", roots.join(" "));
}
