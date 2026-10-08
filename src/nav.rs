//! Walkable-floor graph for the bots, built from the map collision alone (the retail `.nav`
//! files exist only for quest maps, `hall` and `blitzkrieg`; docs/formats.md "Bots").
//!
//! Nodes are floor points sampled on a [`CELL`] grid (every walkable surface with room for a
//! character, so stacked floors and stairs get their own nodes). Links are found lazily, the first
//! time a search reaches a node, by *simulating the actor controller* with
//! [`MapCollision::slide_move`]: walk one cell in each axis direction (stairs, ramps and steps
//! pass, walls do not), fall off a ledge ([`Kind::Drop`]) or jump to a ledge or across a gap
//! ([`Kind::Jump`]). Searches are A*; a link a bot proves wrong is [`Nav::mark_broken`].

use crate::{
    actor::{
        GRAVITY, HEIGHT, JUMP, RADIUS, RUN, RUN_GRAVITY, SLIDE_GRAVITY, WALL_MIN_HEIGHT,
        WALL_RUN_SIDE, WALL_RUN_UP, climb,
    },
    col::{MapCollision, STEP, WALKABLE},
};
use bevy::prelude::*;
use std::{
    cmp::Ordering, collections::BinaryHeap, collections::HashMap, collections::HashSet,
    f32::consts::FRAC_PI_2,
};

/// Grid spacing of the floor samples (metres).
const CELL: f32 = 0.5;
/// Simulation step (seconds).
const DT: f32 = 1.0 / 30.0;
/// Longest horizontal hop and deepest fall a link may span (metres). *Inferred*: the port has no
/// fall damage (`combat.rs` never reads the fall), so bots may jump off any upper floor; 20 m
/// covers the deepest map (Stairway: 8 more routes than 8 m).
const REACH: f32 = 6.5;
const DROP: f32 = 20.0;
/// A wall climb jumps when the wall is this close (metres): the jump peaks at the wall.
const CLIMB_LEAD: f32 = 2.0;
/// A side wall run needs a wall within this distance (metres) at the side of the run-up, the jump
/// turns into it by one of these angles (radians; glancing, so the controller's wall run is a
/// side one, `fwd.dot(n) >= -0.7`) and may carry this far (`WALL_RUN_SIDE` x `RUN` = 12.6 m).
/// *Inferred* values.
const SIDE_WALL: f32 = 4.0;
const SIDE_ANGLES: [f32; 2] = [0.35, 0.6];
const SIDE_REACH: f32 = 14.0;
/// A node is taken for a landing spot within this horizontal distance and height difference.
const SNAP_XZ: f32 = 0.8;
const SNAP_Y: f32 = 0.6;
/// A route search in progress ([`Nav::search`], [`Nav::advance`]).
pub struct Search {
    s: u32,
    g: u32,
    goal: Vec3,
    /// Cost so far and the link (node, index) that reached each node.
    cost: Vec<f32>,
    prev: Vec<(u32, usize)>,
    done: Vec<bool>,
    open: BinaryHeap<Open>,
    /// The expanded node closest to the goal and its distance.
    best: (u32, f32),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Walk (stairs and ramps included).
    Walk,
    /// Jump at the node, then keep running towards the next one.
    Jump,
    /// Run off a ledge and fall.
    Drop,
}

#[derive(Clone, Copy, Debug)]
struct Link {
    to: u32,
    kind: Kind,
    cost: f32,
    /// Where a [`Kind::Jump`] leaves the ground (the ledge or wall the run-up reaches).
    takeoff: Vec3,
    /// Heading to face after the jump for `hold` seconds (a side wall run; zero: steer to the
    /// next node).
    turn: Vec3,
    hold: f32,
}

/// One node of a route; `kind` is how it is reached from the previous node.
#[derive(Clone, Copy, Debug)]
pub struct Step {
    pub node: u32,
    pub kind: Kind,
    /// For [`Kind::Jump`]: the feet position to jump from (on the previous node's floor).
    pub takeoff: Vec3,
    /// For a side wall run: the heading to hold for `hold` seconds after the jump.
    pub turn: Vec3,
    pub hold: f32,
}

#[derive(Resource)]
pub struct Nav {
    /// Feet positions.
    pub nodes: Vec<Vec3>,
    cells: HashMap<(i32, i32), Vec<u32>>,
    min: Vec2,
    links: Vec<Vec<Link>>,
    broken: HashSet<(u32, u32)>,
}

/// Longest straight walk [`walkable`] accepts (metres).
const WALK_MAX: f32 = 30.0;

/// Whether a character can walk the straight line `a` -> `b` (feet positions): nothing blocks
/// the body at chest height and the floor under every metre of it changes by < 0.7 m (stairs
/// and ramps pass, drops and cliffs do not).
pub fn walkable(col: &MapCollision, a: Vec3, b: Vec3) -> bool {
    let (d, up) = (b - a, Vec3::Y * 0.9);
    let len = d.length();
    if len > WALK_MAX || len < 1e-3 {
        return len < 1e-3;
    }
    if col.raycast(a + up, d / len, len).is_some() {
        return false;
    }
    let steps = len.ceil() as u32;
    let mut floor = a.y;
    for i in 1..=steps {
        let p = a + d * (i as f32 / steps as f32);
        let Some(h) = col.raycast(p + Vec3::Y * 0.6, Vec3::NEG_Y, 2.0) else {
            return false;
        };
        let y = h.point.y;
        if (y - floor).abs() > 0.7 {
            return false;
        }
        floor = y;
    }
    true
}

/// A controller run from a node: where it came down and what it did on the way.
struct Sim {
    land: Vec3,
    /// It left the ground other than by the jump (a ledge).
    air: bool,
    takeoff: Vec3,
    /// A wall run along a side wall carried it.
    side: bool,
    /// Seconds from takeoff to landing.
    secs: f32,
}

/// Runs the controller from `from` along the unit horizontal `dir` at full speed until it stands
/// on a floor `want` metres away. With `jump` it first runs up to the ledge or wall (at most 1 m)
/// and jumps from there; with a `lead` it instead runs until a wall stands within `lead` metres
/// and jumps then, so that the jump peaks at the wall and the wall run (`actor.rs`) climbs on from
/// there. A non-zero `turn` is the heading the actor faces after the jump (it keeps its speed and
/// steers with the air control): into a side wall at a glancing angle it wall-runs along it for
/// `WALL_RUN_SIDE` seconds. `None` when it is blocked, falls too far or goes farther than `reach`.
#[allow(clippy::too_many_arguments)]
fn simulate(
    col: &MapCollision,
    from: Vec3,
    dir: Vec3,
    jump: bool,
    lead: f32,
    want: f32,
    turn: Vec3,
    reach: f32,
) -> Option<Sim> {
    let mut takeoff = from;
    if jump {
        let (steps, chest) = if lead > 0.0 {
            (16, Vec3::Y)
        } else {
            (5, Vec3::ZERO)
        };
        for _ in 0..steps {
            if lead > 0.0 && col.raycast(takeoff + chest, dir, lead).is_some() {
                break;
            }
            let m = col.slide_move(
                takeoff,
                Vec3::new(dir.x * RUN * DT, -0.05, dir.z * RUN * DT),
                RADIUS,
                HEIGHT,
            );
            let moved = Vec2::new(m.pos.x - takeoff.x, m.pos.z - takeoff.z).length();
            if !m.grounded || moved < RUN * DT * 0.3 {
                if lead > 0.0 {
                    return None;
                }
                break;
            }
            takeoff = m.pos;
        }
    }
    let face = if turn == Vec3::ZERO || !jump {
        dir
    } else {
        turn
    };
    let wish = face * RUN;
    let mut hv = dir * RUN;
    let (mut pos, mut vy, mut grounded, mut air) =
        (takeoff, if jump { JUMP } else { 0.0 }, !jump, false);
    // Wall contact (normal, time) and the wall run in progress (normal, seconds left, along the
    // wall), as the controller keeps them.
    let (mut wall, mut run, mut spent) = (None::<(Vec3, f32)>, None::<(Vec3, f32, bool)>, false);
    let mut side = false;
    for step in 0..90 {
        let t = step as f32 * DT;
        let mut gravity = 1.0;
        let mut fall = 40.0;
        if let Some((n, left, along)) = run {
            let speed = if left > 0.0 { RUN } else { RUN * 0.5 };
            hv = if along {
                (face - n * face.dot(n)).normalize_or_zero() * speed
            } else {
                Vec3::ZERO
            } - n * 1.5;
            if left <= 0.0 {
                (gravity, fall) = (SLIDE_GRAVITY, 6.0);
            } else if along {
                gravity = RUN_GRAVITY;
            } else {
                vy = climb(WALL_RUN_UP - left);
                gravity = 0.0;
            }
            run = Some((n, left - DT, along));
        } else if grounded {
            hv = wish;
        } else {
            // air control
            hv += (wish - hv).clamp_length_max(10.0 * DT);
        }
        vy = (vy - GRAVITY * gravity * DT).max(-fall);
        let mut d = Vec3::new(hv.x, vy, hv.z) * DT;
        if grounded && vy <= 0.0 {
            d.y = d.y.min(-0.05);
        }
        let m = col.slide_move(pos, d, RADIUS, HEIGHT);
        let moved = Vec2::new(m.pos.x - pos.x, m.pos.z - pos.z).length();
        if m.grounded {
            vy = vy.max(0.0);
        } else if vy > 0.0 && m.pos.y - pos.y < d.y * 0.5 {
            vy = 0.0;
        }
        // A ledge higher than the controller's step is a jump, not a walk.
        if grounded && m.grounded && m.pos.y - pos.y > STEP + 0.02 {
            return None;
        }
        (pos, grounded) = (m.pos, m.grounded);
        air |= !grounded && !jump;
        if grounded {
            (run, wall, spent) = (None, None, false);
        } else if let Some(w) = m.wall {
            let n = Vec3::new(w.x, 0.0, w.z).normalize_or_zero();
            hv -= n * hv.dot(n).min(0.0);
            wall = Some((n, t));
        }
        if jump && !grounded {
            // A wall run ends when the wall is lost; one starts facing a wall (up) or at a
            // glancing angle (along) with air below.
            if wall.is_none_or(|w| t - w.1 > 0.15) {
                run = None;
            } else if run.is_none()
                && !spent
                && let Some((n, _)) = wall
                && face.dot(n) < 0.3
                && col
                    .raycast(pos + Vec3::Y * 0.05, Vec3::NEG_Y, WALL_MIN_HEIGHT)
                    .is_none()
            {
                let along = face.dot(n) >= -0.7;
                (run, spent, side) = (
                    Some((n, if along { WALL_RUN_SIDE } else { WALL_RUN_UP }, along)),
                    true,
                    side | along,
                );
                vy = if along {
                    vy.min(2.0)
                } else {
                    vy.max(climb(0.0))
                };
            }
        }
        let far = Vec2::new(pos.x - from.x, pos.z - from.z).length();
        // Pressing against a ledge while rising is fine; only a grounded stall means blocked.
        let blocked = grounded && m.wall.is_some() && moved < RUN * DT * 0.3;
        if far > reach || pos.y < from.y - DROP || blocked {
            return None;
        }
        if grounded && far >= want {
            return Some(Sim {
                land: pos,
                air,
                takeoff,
                side,
                secs: t,
            });
        }
    }
    None
}

impl Nav {
    /// Samples the floors of the box `min..max` (metres), then finds every node's links on all
    /// cores (the simulations cost ~0.1 ms each, so a map takes seconds of CPU).
    pub fn new(col: &MapCollision, min: Vec3, max: Vec3) -> Self {
        let mut nodes = Vec::new();
        let mut cells: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        let min2 = Vec2::new(min.x, min.z);
        let n = ((max.x - min.x) / CELL).ceil() as i32;
        let m = ((max.z - min.z) / CELL).ceil() as i32;
        for ix in 0..n {
            for iz in 0..m {
                let (x, z) = (
                    min.x + (ix as f32 + 0.5) * CELL,
                    min.z + (iz as f32 + 0.5) * CELL,
                );
                let mut y = max.y + 1.0;
                while let Some(h) = col.raycast(Vec3::new(x, y, z), Vec3::NEG_Y, y - min.y + 1.0) {
                    y = h.point.y - 0.05;
                    // A real standing place: a floor with headroom that the capsule lands on.
                    let stand =
                        col.slide_move(h.point + Vec3::Y * 0.03, Vec3::NEG_Y * 0.1, RADIUS, HEIGHT);
                    if h.normal.y >= WALKABLE
                        && stand.grounded
                        && (stand.pos.y - h.point.y).abs() < 0.08
                        && col
                            .raycast(h.point + Vec3::Y * 0.02, Vec3::Y, HEIGHT)
                            .is_none()
                    {
                        cells.entry((ix, iz)).or_default().push(nodes.len() as u32);
                        nodes.push(stand.pos);
                    }
                }
            }
        }
        let mut nav = Nav {
            links: Vec::new(),
            nodes,
            cells,
            min: min2,
            broken: HashSet::new(),
        };
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        let chunk = nav.nodes.len().div_ceil(threads).max(1);
        let ids: Vec<u32> = (0..nav.nodes.len() as u32).collect();
        nav.links = std::thread::scope(|s| {
            let workers: Vec<_> = ids
                .chunks(chunk)
                .map(|c| {
                    let nav = &nav;
                    s.spawn(move || c.iter().map(|&i| nav.links_of(col, i)).collect::<Vec<_>>())
                })
                .collect();
            workers
                .into_iter()
                .flat_map(|w| w.join().expect("nav worker"))
                .collect()
        });
        nav
    }

    fn cell(&self, p: Vec3) -> (i32, i32) {
        (
            ((p.x - self.min.x) / CELL).floor() as i32,
            ((p.z - self.min.y) / CELL).floor() as i32,
        )
    }

    /// The node under or beside `p` (a floor within `up` above and 2 m below it), preferring
    /// the closest.
    pub fn nearest(&self, p: Vec3, up: f32) -> Option<u32> {
        let (cx, cz) = self.cell(p);
        for r in 1..=2 {
            let best = (cx - r..=cx + r)
                .flat_map(|x| (cz - r..=cz + r).map(move |z| (x, z)))
                .filter_map(|c| self.cells.get(&c))
                .flatten()
                .copied()
                .filter(|&i| {
                    let dy = p.y - self.nodes[i as usize].y;
                    (-up..=2.0).contains(&dy)
                })
                .min_by(|&a, &b| {
                    let s = |i: u32| {
                        let n = self.nodes[i as usize];
                        Vec2::new(n.x - p.x, n.z - p.z).length() + 2.0 * (n.y - p.y).abs()
                    };
                    s(a).total_cmp(&s(b))
                });
            if best.is_some() {
                return best;
            }
        }
        None
    }

    /// The node a controller landing at `p` stands on.
    fn snap(&self, p: Vec3) -> Option<u32> {
        let (cx, cz) = self.cell(p);
        (cx - 1..=cx + 1)
            .flat_map(|x| (cz - 1..=cz + 1).map(move |z| (x, z)))
            .filter_map(|c| self.cells.get(&c))
            .flatten()
            .copied()
            .filter(|&i| {
                let n = self.nodes[i as usize];
                (n.y - p.y).abs() < SNAP_Y && Vec2::new(n.x - p.x, n.z - p.z).length() < SNAP_XZ
            })
            .min_by(|&a, &b| {
                let s = |i: u32| self.nodes[i as usize].distance_squared(p);
                s(a).total_cmp(&s(b))
            })
    }

    /// The links of node `i`: what the controller does when it walks, drops or jumps from it.
    fn links_of(&self, col: &MapCollision, i: u32) -> Vec<Link> {
        let p = self.nodes[i as usize];
        let mut out: Vec<Link> = Vec::new();
        for k in 0..8 {
            let a = k as f32 * std::f32::consts::FRAC_PI_4;
            let dir = Vec3::new(a.cos(), 0.0, a.sin());
            let want = CELL
                * 0.9
                * if k % 2 == 1 {
                    std::f32::consts::SQRT_2
                } else {
                    1.0
                };
            let mut walked = false;
            if let Some(sim) = simulate(col, p, dir, false, 0.0, want, Vec3::ZERO, REACH)
                && let Some(j) = self.snap(sim.land).filter(|&j| j != i)
            {
                let d = p.distance(self.nodes[j as usize]);
                let (kind, cost) = if sim.air {
                    (
                        Kind::Drop,
                        d * 1.1 + 1.0 + 0.5 * (p.y - self.nodes[j as usize].y).max(0.0),
                    )
                } else {
                    (Kind::Walk, d)
                };
                out.push(Link {
                    to: j,
                    kind,
                    cost,
                    takeoff: p,
                    turn: Vec3::ZERO,
                    hold: 0.0,
                });
                walked = !sim.air;
            }
            // A ledge or gap: jump unless something is in the way at head height; failing that,
            // a wall on an axis direction: run at it, jump and wall-run up.
            if walked {
                continue;
            }
            let lands = |s: &Sim| self.snap(s.land).filter(|&j| j != i);
            let hop = if col.raycast(p + Vec3::Y * 1.4, dir, 1.0).is_none() {
                simulate(col, p, dir, true, 0.0, want, Vec3::ZERO, REACH)
                    .filter(|s| lands(s).is_some())
            } else {
                None
            }
            .map(|s| (s, 1.3, 1.5));
            let climb = || {
                if k % 2 == 0 && col.raycast(p + Vec3::Y, dir, CLIMB_LEAD + 1.0).is_some() {
                    simulate(col, p, dir, true, CLIMB_LEAD, want, Vec3::ZERO, REACH)
                        .filter(|s| lands(s).is_some())
                        .map(|s| (s, 1.5, 4.0))
                } else {
                    None
                }
            };
            // Failing that, a pit or gap between side walls: jump off the ledge, turn into the
            // wall at a glancing angle and wall-run along it (the sim sets the landing).
            let side_run = || {
                if col
                    .raycast(p + dir * 0.9 + Vec3::Y * 0.5, Vec3::NEG_Y, 1.2)
                    .is_some()
                {
                    return None;
                }
                [1.0f32, -1.0].into_iter().find_map(|s| {
                    let wall = Quat::from_rotation_y(s * FRAC_PI_2) * dir;
                    col.raycast(p + Vec3::Y, wall, SIDE_WALL)?;
                    SIDE_ANGLES.into_iter().find_map(|a| {
                        let turn = Quat::from_rotation_y(s * a) * dir;
                        simulate(col, p, dir, true, 0.0, want, turn, SIDE_REACH)
                            .filter(|s| s.side && lands(s).is_some())
                            .map(|s| (s, turn))
                    })
                })
            };
            if let Some((sim, per_m, fixed)) = hop.or_else(climb)
                && let Some(j) = lands(&sim)
            {
                let d = p.distance(self.nodes[j as usize]);
                out.push(Link {
                    to: j,
                    kind: Kind::Jump,
                    cost: d * per_m + fixed,
                    takeoff: sim.takeoff,
                    turn: Vec3::ZERO,
                    hold: 0.0,
                });
            } else if let Some((sim, turn)) = side_run()
                && let Some(j) = lands(&sim)
            {
                let d = p.distance(self.nodes[j as usize]);
                out.push(Link {
                    to: j,
                    kind: Kind::Jump,
                    cost: d * 1.5 + 6.0,
                    takeoff: sim.takeoff,
                    turn,
                    hold: sim.secs + 0.3,
                });
            }
        }
        out
    }

    /// Declares the link `a` -> `b` unusable (a bot got stuck on it).
    pub fn mark_broken(&mut self, a: u32, b: u32) {
        self.broken.insert((a, b));
    }

    /// Starts an A* from the floor under `from` to the floor under `to`; `None` when either has no
    /// node. Run it with [`Nav::advance`], a few thousand expansions per frame.
    pub fn search(&self, from: Vec3, to: Vec3) -> Option<Search> {
        let start = self.nearest(from, 0.35).or_else(|| self.nearest(from, 1.5));
        let (s, g) = (start?, self.nearest(to, 1.5)?);
        let n = self.nodes.len();
        let goal = self.nodes[g as usize];
        let mut cost = vec![f32::INFINITY; n];
        cost[s as usize] = 0.0;
        let h = self.nodes[s as usize].distance(goal);
        Some(Search {
            s,
            g,
            goal,
            cost,
            prev: vec![(s, 0); n],
            done: vec![false; n],
            open: BinaryHeap::from([Open(h, s)]),
            best: (s, h),
        })
    }

    /// Expands nodes of `q` while `budget` lasts (each expansion costs 1). Returns the route once
    /// the goal is reached, or, when it cannot be, the one that ends at the reachable node
    /// closest to it.
    pub fn advance(&self, q: &mut Search, budget: &mut usize) -> Option<Vec<Step>> {
        let h = |i: u32| self.nodes[i as usize].distance(q.goal);
        while let Some(&Open(_, i)) = q.open.peek() {
            if i == q.g {
                q.best.0 = q.g;
                break;
            }
            if *budget == 0 {
                return None;
            }
            q.open.pop();
            if std::mem::replace(&mut q.done[i as usize], true) {
                continue;
            }
            *budget -= 1;
            if h(i) < q.best.1 {
                q.best = (i, h(i));
            }
            for (k, l) in self.links[i as usize].iter().enumerate() {
                if self.broken.contains(&(i, l.to)) {
                    continue;
                }
                let c = q.cost[i as usize] + l.cost;
                if c < q.cost[l.to as usize] {
                    q.cost[l.to as usize] = c;
                    q.prev[l.to as usize] = (i, k);
                    q.open.push(Open(c + h(l.to), l.to));
                }
            }
        }
        let mut route = Vec::new();
        let mut i = q.best.0;
        loop {
            let (from, k) = q.prev[i as usize];
            let l = self.links[from as usize].get(k).filter(|_| i != q.s);
            route.push(Step {
                node: i,
                kind: l.map_or(Kind::Walk, |l| l.kind),
                takeoff: l.map_or(Vec3::ZERO, |l| l.takeoff),
                turn: l.map_or(Vec3::ZERO, |l| l.turn),
                hold: l.map_or(0.0, |l| l.hold),
            });
            if i == q.s {
                break;
            }
            i = from;
        }
        route.reverse();
        Some(route)
    }

    /// A whole search at once (diagnostics and tests; the bots slice theirs).
    pub fn route(&self, from: Vec3, to: Vec3) -> Vec<Step> {
        let mut budget = usize::MAX;
        self.search(from, to)
            .and_then(|mut q| self.advance(&mut q, &mut budget))
            .unwrap_or_default()
    }

    /// The nodes reachable from `i` in one move, for diagnostics.
    pub fn neighbors(&self, i: u32) -> impl Iterator<Item = (u32, Kind)> + '_ {
        self.links[i as usize].iter().map(|l| (l.to, l.kind))
    }
}

/// Min-heap entry: estimated total cost, node.
struct Open(f32, u32);

impl PartialEq for Open {
    fn eq(&self, o: &Self) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}
impl Eq for Open {}
impl PartialOrd for Open {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Open {
    fn cmp(&self, o: &Self) -> Ordering {
        o.0.total_cmp(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        level::Level,
        map,
        mrs::Vfs,
        view::{SCALE, to_bevy},
    };

    /// The routing check (docs/formats.md "Bots"): per spawn point, one route to the next spawn
    /// and one to the spawn half the list away; a route counts when it ends within 2 m of the
    /// goal. Needs the retail install (`GUNZ_GAME=<dir>`, optional `GUNZ_MAPS=a,b`), so it is
    /// ignored by default: `cargo test --release routing_pairs -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn routing_pairs() {
        let Ok(game) = std::env::var("GUNZ_GAME") else {
            return;
        };
        let vfs = Vfs::mount(&game).unwrap();
        let first = map::find_rs(&vfs, "mansion").unwrap();
        let mut level = Level {
            map: map::load(&vfs, &first).unwrap(),
            vfs,
        };
        let maps = std::env::var("GUNZ_MAPS").unwrap_or_else(|_| {
            "battle arena,blitzkrieg,castle,catacomb,citadel,classic town,dungeon,factory,garden,hall,halloween town,high_haven,island,lost shrine,port,prison,prison ii,ruin,shower room,skirmishhall,snow_town,stairway,station,test_a,test_b,town,weaponshop,jail,mansion".into()
        });
        let (mut ok_all, mut all) = (0, 0);
        for name in maps.split(',') {
            let rs = map::find_rs(&level.vfs, name).unwrap();
            let col = MapCollision::load(&level.vfs, &rs).unwrap();
            level.map = map::load(&level.vfs, &rs).unwrap();
            let (mut min, mut max) = (Vec3::MAX, Vec3::MIN);
            for v in &level.map.vertices {
                let p = Vec3::from(to_bevy(v.pos)) * SCALE;
                (min, max) = (min.min(p), max.max(p));
            }
            let t = std::time::Instant::now();
            let nav = Nav::new(&col, min, max);
            let built = t.elapsed().as_secs_f32();
            let spawns: Vec<Vec3> = level.spawn_points().iter().map(|s| s.0).collect();
            let n = spawns.len();
            let (mut ok, mut total, mut worst) = (0, 0, 0.0f32);
            for (i, &a) in spawns.iter().enumerate() {
                for j in [(i + 1) % n, (i + n / 2) % n] {
                    let t = std::time::Instant::now();
                    let route = nav.route(a, spawns[j]);
                    worst = worst.max(t.elapsed().as_secs_f32());
                    total += 1;
                    let hit = route
                        .last()
                        .is_some_and(|s| nav.nodes[s.node as usize].distance(spawns[j]) < 2.0);
                    if !hit && std::env::var("GUNZ_BAD").is_ok() {
                        let end = route
                            .last()
                            .map_or(Vec3::ZERO, |s| nav.nodes[s.node as usize]);
                        println!("  BAD {a:.1} -> {:.1} ended {end:.1}", spawns[j]);
                    }
                    ok += hit as u32;
                }
            }
            println!(
                "{name:16} {ok:4}/{total:<4} nodes {:6} build {built:5.1}s worst route {:.1} ms",
                nav.nodes.len(),
                worst * 1000.0
            );
            (ok_all, all) = (ok_all + ok, all + total);
        }
        println!("TOTAL {ok_all}/{all}");
    }
}
