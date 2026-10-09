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
        FALL, GRAVITY, HEIGHT, JUMP, RADIUS, RUN, RUN_GRAVITY, SLIDE_GRAVITY, WALL_GRACE,
        WALL_LOSE, WALL_MIN_HEIGHT, WALL_OUT, WALL_RUN_SIDE, WALL_RUN_UP, WALL_UP, climb,
    },
    col::{MapCollision, STEP, WALKABLE},
    combat::yaw_of,
};
use bevy::prelude::*;
use std::{
    cmp::Ordering,
    collections::BinaryHeap,
    collections::HashMap,
    collections::HashSet,
    f32::consts::{FRAC_PI_2, TAU},
    sync::atomic::{AtomicBool, AtomicU32, Ordering as AtomicOrdering},
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
/// The controller's own step (seconds): the actor moves by the frame time, which is 1/60 s in
/// headless runs and on a 60 Hz display. A [`Kick`] is counted in these ticks.
pub const PDT: f32 = 1.0 / 60.0;
/// A wall kick climb ([`Kind::Kick`]) lands on an island floor `KICK_RISE` to `KICK_MAX` metres
/// above its start and within `KICK_REACH` metres of it; it looks for a wall within `KICK_WALL`
/// metres of the start (16 headings), jumps in the first `KICK_JUMP` ticks and may keep the
/// controller's wall run for `KICK_AIR` ticks. `KICK_TOL` is the start offset (metres, every
/// compass point) the script must survive, `KICK_COST` is added to the path cost. *Inferred*
/// values.
const KICK_RISE: f32 = 3.0;
const KICK_WALL: f32 = 6.0;
const KICK_JUMP: u32 = 30;
const KICK_AIR: u32 = 70;
const KICK_TOL: f32 = 0.025;
const KICK_COST: f32 = 10.0;
/// The floors a run at a wall may start from are `KICK_NEAR` to `KICK_RUN` metres in front of
/// it; at most `KICK_STARTS` are tried per wall and an island gets `KICK_TRIES` tries.
const KICK_NEAR: f32 = 1.0;
const KICK_RUN: f32 = 6.0;
const KICK_STARTS: usize = 4;
const KICK_TRIES: u32 = 60;
/// Fewest floors of an island.
const ISLAND_MIN: u32 = 40;
/// Ticks a kicked pawn may fall before it counts as lost.
const KICK_FLIGHT: u32 = 180;
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
    /// Wall kick climbs may be used.
    kicks: bool,
}

/// A wall kick climb script ([`Kind::Kick`]): from a standstill at the takeoff with forward held,
/// face `run` until tick `jump` (the jump press), face `wall` from then on and press jump again
/// at tick `kick` (the wall kick). The nav search finds it by simulating the controller
/// ([`Pawn`]); the bot replays it tick by tick ([`Kick::input`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Kick {
    pub run: f32,
    pub wall: f32,
    pub jump: u32,
    pub kick: u32,
}

/// Where a wall kick leaves a wall ([`Nav::perches`]): the wall's outward normal and the
/// pawn's position at the kick.
struct Perch {
    n: Vec3,
    at: Vec3,
}

impl Kick {
    /// Facing (yaw) and jump press of tick `n`.
    pub fn input(&self, n: u32) -> (f32, bool) {
        (
            if n <= self.jump { self.run } else { self.wall },
            n == self.jump || n == self.kick,
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Walk (stairs and ramps included).
    Walk,
    /// Jump at the node, then keep running towards the next one.
    Jump,
    /// Run off a ledge and fall.
    Drop,
    /// A wall run up a wall and a wall kick off it ([`Kick`]): from a standstill at `takeoff`.
    Kick,
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
    kick: Option<Kick>,
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
    /// For [`Kind::Kick`]: the script to run from a standstill at `takeoff`.
    pub kick: Option<Kick>,
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
        let mut fall = FALL;
        if let Some((n, left, along)) = run {
            let speed = if left > 0.0 { RUN } else { RUN * 0.5 };
            hv = if along {
                (face - n * face.dot(n)).normalize_or_zero() * speed
            } else {
                Vec3::ZERO
            } - n * 1.5;
            if left <= 0.0 {
                (gravity, fall) = (SLIDE_GRAVITY, FALL);
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

/// What the controller is doing in the air besides falling (`actor.rs` `State`).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Air {
    Free,
    /// Wall run: wall normal, along the wall (side run) or up it, seconds left.
    Run(Vec3, bool, f32),
    /// Wall kick animation (no control, no new kick): seconds left.
    Kick(f32),
}

/// The actor controller's movement (`actor.rs`: jump, wall kick, wall run, air control, gravity)
/// with forward held, as a plain value so a search can try inputs on it. One [`Pawn::step`] is
/// one `dt` (a `PDT`).
#[derive(Clone, Copy, Debug)]
struct Pawn {
    pos: Vec3,
    hv: Vec3,
    vy: f32,
    grounded: bool,
    /// Last wall touched in the air: outward normal and time.
    wall: Option<(Vec3, f32)>,
    spent: bool,
    air: Air,
    t: f32,
    /// Seconds per step ([`PDT`] unless a test tries another frame time).
    dt: f32,
}

impl Pawn {
    /// Standing still at `pos`.
    fn new(pos: Vec3) -> Self {
        Self {
            pos,
            hv: Vec3::ZERO,
            vy: 0.0,
            grounded: true,
            wall: None,
            spent: false,
            air: Air::Free,
            t: 0.0,
            dt: PDT,
        }
    }

    /// One tick facing `yaw` with forward held and a jump press or not.
    fn step(&mut self, col: &MapCollision, yaw: f32, jump: bool) {
        let face = Quat::from_rotation_y(yaw) * Vec3::NEG_Z;
        let now = self.t;
        let lost = now - self.wall.map_or(f32::MIN, |w| w.1) > WALL_LOSE;
        match &mut self.air {
            Air::Kick(left) => {
                *left -= self.dt;
                if *left <= 0.0 || (self.grounded && now > self.wall.map_or(0.0, |w| w.1) + 0.1) {
                    self.air = Air::Free;
                }
            }
            Air::Run(_, _, left) => {
                *left -= self.dt;
                if self.grounded || lost {
                    self.air = Air::Free;
                }
            }
            Air::Free => {}
        }
        let free = self.air == Air::Free;
        if jump && free && self.grounded {
            self.vy = JUMP;
            self.grounded = false;
        } else if jump
            && (free || matches!(self.air, Air::Run(..)))
            && !self.grounded
            && let Some((n, t)) = self.wall
            && now - t < WALL_GRACE
        {
            // `jump_wallF` (facing the wall) is 30 frames long, the others 40, at 30 fps.
            self.air = Air::Kick(if face.dot(n) < -0.6 { 1.0 } else { 4.0 / 3.0 });
            self.hv = n * WALL_OUT;
            self.vy = WALL_UP;
            self.wall = None;
        }
        if self.air == Air::Free
            && !self.grounded
            && !self.spent
            && !jump
            && let Some((n, t)) = self.wall
            && now - t < 0.1
            && face.dot(n) < 0.3
            && col
                .raycast(self.pos + Vec3::Y * 0.05, Vec3::NEG_Y, WALL_MIN_HEIGHT)
                .is_none()
        {
            let along = face.dot(n) >= -0.7;
            self.air = Air::Run(n, along, if along { WALL_RUN_SIDE } else { WALL_RUN_UP });
            self.spent = true;
            self.vy = if along {
                self.vy.min(2.0)
            } else {
                self.vy.max(climb(0.0))
            };
        }
        let wish = face * RUN;
        let (mut gravity, mut fall) = (1.0, FALL);
        match self.air {
            Air::Run(n, along, left) => {
                let speed = if left > 0.0 { RUN } else { RUN * 0.5 };
                self.hv = if along {
                    (face - n * face.dot(n)).normalize_or_zero() * speed
                } else {
                    Vec3::ZERO
                } - n * 1.5;
                if left <= 0.0 {
                    (gravity, fall) = (SLIDE_GRAVITY, FALL);
                } else if along {
                    gravity = RUN_GRAVITY;
                } else {
                    self.vy = climb(WALL_RUN_UP - left);
                    gravity = 0.0;
                }
            }
            Air::Kick(_) => {}
            Air::Free if self.grounded => {
                self.hv += (wish - self.hv).clamp_length_max(60.0 * self.dt);
            }
            Air::Free => {
                self.hv += (wish - self.hv).clamp_length_max(10.0 * self.dt);
            }
        }
        self.vy = (self.vy - GRAVITY * gravity * self.dt).max(-fall);
        let mut d = Vec3::new(self.hv.x, self.vy, self.hv.z) * self.dt;
        if self.grounded && self.vy <= 0.0 {
            d.y = d.y.min(-0.05);
        }
        let m = col.slide_move(self.pos, d, RADIUS, HEIGHT);
        let moved_up = m.pos.y - self.pos.y;
        self.pos = m.pos;
        if let Some(w) = m.wall {
            let n = Vec3::new(w.x, 0.0, w.z).normalize_or_zero();
            self.hv -= n * self.hv.dot(n).min(0.0);
            if !m.grounded {
                self.wall = Some((n, now));
            }
        }
        if m.grounded {
            self.vy = self.vy.max(0.0);
            self.spent = false;
        } else if self.vy > 0.0 && moved_up < d.y * 0.5 {
            self.vy = 0.0;
        }
        self.grounded = m.grounded;
        self.t += self.dt;
    }
}

impl Nav {
    /// Samples the floors of the box `min..max` (metres), then finds every node's links on all
    /// cores (the simulations cost ~0.1 ms each, so a map takes seconds of CPU).
    pub fn new(col: &MapCollision, min: Vec3, max: Vec3) -> Self {
        let mut nav = Self::floors(col, min, max);
        nav.add_kicks(col);
        nav
    }

    /// [`Nav::new`] without the wall kick climbs.
    fn floors(col: &MapCollision, min: Vec3, max: Vec3) -> Self {
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
        let links = par(nav.nodes.len(), |i| nav.links_of(col, i));
        nav.links = links;
        nav
    }

    /// Adds the wall kick climbs ([`Kind::Kick`]) from the main body of the map to its islands.
    /// Per island (a group of linked floors) the search stops at its first climb or after
    /// `KICK_TRIES` tries.
    fn add_kicks(&mut self, col: &MapCollision) {
        let island = self.islands();
        let comp = self.components(&island);
        let groups = comp
            .iter()
            .filter(|&&c| c != u32::MAX)
            .max()
            .map_or(0, |c| c + 1) as usize;
        let state: Vec<(AtomicBool, AtomicU32)> = (0..groups)
            .map(|_| (AtomicBool::new(false), AtomicU32::new(0)))
            .collect();
        let extra = par(self.nodes.len(), |j| {
            let c = comp[j as usize];
            (c != u32::MAX)
                .then(|| self.kick_into(col, &island, j, &state[c as usize]))
                .flatten()
        });
        for (i, link) in extra.into_iter().flatten() {
            self.links[i as usize].push(link);
        }
    }

    /// For each node: it is on an island, a floor that the floors of the largest group of
    /// mutually reachable floors (the main body of the map) do not reach along the links.
    fn islands(&self) -> Vec<bool> {
        // Tarjan's strongly connected components, without recursion.
        let n = self.nodes.len();
        let (mut index, mut low, mut comp) = (vec![u32::MAX; n], vec![0u32; n], vec![u32::MAX; n]);
        let (mut next, mut groups) = (0u32, 0u32);
        let (mut stack, mut calls): (Vec<u32>, Vec<(u32, usize)>) = (Vec::new(), Vec::new());
        for root in 0..n as u32 {
            if index[root as usize] != u32::MAX {
                continue;
            }
            (index[root as usize], low[root as usize]) = (next, next);
            next += 1;
            stack.push(root);
            calls.push((root, 0));
            while let Some(&(v, k)) = calls.last() {
                if let Some(l) = self.links[v as usize].get(k) {
                    calls.last_mut().expect("call").1 += 1;
                    let w = l.to as usize;
                    if index[w] == u32::MAX {
                        (index[w], low[w]) = (next, next);
                        next += 1;
                        stack.push(l.to);
                        calls.push((l.to, 0));
                    } else if comp[w] == u32::MAX {
                        low[v as usize] = low[v as usize].min(index[w]);
                    }
                } else {
                    calls.pop();
                    if low[v as usize] == index[v as usize] {
                        while let Some(w) = stack.pop() {
                            comp[w as usize] = groups;
                            if w == v {
                                break;
                            }
                        }
                        groups += 1;
                    }
                    if let Some(&(u, _)) = calls.last() {
                        low[u as usize] = low[u as usize].min(low[v as usize]);
                    }
                }
            }
        }
        let mut size = vec![0u32; groups as usize];
        for &c in &comp {
            size[c as usize] += 1;
        }
        let main = (0..groups as usize).max_by_key(|&c| size[c]).unwrap_or(0) as u32;
        let mut reach: Vec<bool> = comp.iter().map(|&c| c == main).collect();
        let mut open: Vec<u32> = (0..n as u32).filter(|&i| reach[i as usize]).collect();
        while let Some(u) = open.pop() {
            for l in &self.links[u as usize] {
                if !std::mem::replace(&mut reach[l.to as usize], true) {
                    open.push(l.to);
                }
            }
        }
        reach.iter().map(|r| !r).collect()
    }

    /// The island of each island floor (`u32::MAX` for the others): floors that the links join,
    /// in either direction, `ISLAND_MIN` or more of them.
    fn components(&self, island: &[bool]) -> Vec<u32> {
        fn find(root: &mut [u32], mut i: u32) -> u32 {
            while root[i as usize] != i {
                root[i as usize] = root[root[i as usize] as usize];
                i = root[i as usize];
            }
            i
        }
        let n = self.nodes.len();
        let mut root: Vec<u32> = (0..n as u32).collect();
        for (a, links) in self.links.iter().enumerate() {
            for l in links.iter().filter(|l| island[a] && island[l.to as usize]) {
                let (x, y) = (find(&mut root, a as u32), find(&mut root, l.to));
                root[x as usize] = y;
            }
        }
        let group: Vec<u32> = (0..n as u32).map(|i| find(&mut root, i)).collect();
        let mut size: HashMap<u32, u32> = HashMap::new();
        for (i, &g) in group.iter().enumerate() {
            if island[i] {
                *size.entry(g).or_default() += 1;
            }
        }
        let mut id: HashMap<u32, u32> = HashMap::new();
        (0..n)
            .map(|i| {
                if !island[i] || size[&group[i]] < ISLAND_MIN {
                    return u32::MAX;
                }
                let next = id.len() as u32;
                *id.entry(group[i]).or_insert(next)
            })
            .collect()
    }

    /// Tries to climb to island floor `j` with a wall kick: for each wall near it that a kick
    /// coming down on `j` leaves ([`Nav::perches`]) a few main floors in front of the wall
    /// ([`Nav::perch_starts`]) are tried ([`Nav::kick_run`]). Returns the start node and link.
    fn kick_into(
        &self,
        col: &MapCollision,
        island: &[bool],
        j: u32,
        (done, tries): &(AtomicBool, AtomicU32),
    ) -> Option<(u32, Link)> {
        for perch in self.perches(col, j) {
            for i in self.perch_starts(&perch, island) {
                if done.load(AtomicOrdering::Relaxed)
                    || tries.fetch_add(1, AtomicOrdering::Relaxed) >= KICK_TRIES
                {
                    return None;
                }
                let p = self.nodes[i as usize];
                if let Some((kick, to)) = self.kick_run(col, island, p, &perch, j) {
                    done.store(true, AtomicOrdering::Relaxed);
                    return Some((
                        i,
                        Link {
                            to,
                            kind: Kind::Kick,
                            cost: p.distance(self.nodes[to as usize]) + KICK_COST,
                            takeoff: p,
                            turn: Vec3::ZERO,
                            hold: 0.0,
                            kick: Some(kick),
                        },
                    ));
                }
            }
        }
        None
    }

    /// The places a wall kick leaves a wall to come down on `j`: for each of 16 headings with a
    /// wall near it, the wall's outward normal and where the kicked pawn must be (`at`, with
    /// `at.y` the height of the kick). A kick throws it out at `WALL_OUT` and up at `WALL_UP`,
    /// so it comes down on `j` (falling, so after the top at `WALL_UP / GRAVITY`, and within the
    /// second the kick animation takes away the control) `0.56..1` s later.
    fn perches(&self, col: &MapCollision, j: u32) -> Vec<Perch> {
        let g = self.nodes[j as usize];
        (0..16)
            .filter_map(|a| {
                let d = Quat::from_rotation_y(a as f32 * TAU / 16.0) * Vec3::NEG_Z;
                let h = col.raycast(g + Vec3::Y, d, KICK_WALL)?;
                let n = Vec3::new(h.normal.x, 0.0, h.normal.z).normalize_or_zero();
                let out = (g - h.point).dot(n) - RADIUS;
                let (t0, t1) = (WALL_UP / GRAVITY, 1.0);
                if h.normal.y.abs() > 0.2
                    || n.dot(-d) < 0.8
                    || !(WALL_OUT * t0..=WALL_OUT * t1).contains(&out)
                {
                    return None;
                }
                let t = out / WALL_OUT;
                let y = g.y - (WALL_UP * t - 0.5 * GRAVITY * t * t);
                let at = Vec3::new(g.x - n.x * out, y, g.z - n.z * out);
                // The flight must be clear (a floor above the wall run stops the kick).
                let arc = |s: u32| {
                    let t = t * s as f32 / 12.0;
                    at + n * WALL_OUT * t + Vec3::Y * (0.9 + WALL_UP * t - 0.5 * GRAVITY * t * t)
                };
                (1..=12)
                    .all(|s| {
                        let (a, b) = (arc(s - 1), arc(s));
                        col.raycast(a, b - a, a.distance(b)).is_none()
                    })
                    .then_some(Perch { n, at })
            })
            .collect()
    }

    /// Up to `KICK_STARTS` main floors to run at `perch` from: `KICK_NEAR` to `KICK_RUN` metres
    /// in front of it (within 37 degrees of its normal) and 0.8..4.8 m below its height (a jump
    /// and a wall run climb that much).
    fn perch_starts(&self, perch: &Perch, island: &[bool]) -> Vec<u32> {
        let (cx, cz) = self.cell(perch.at);
        let r = (KICK_RUN / CELL) as i32;
        let mut v: Vec<(f32, u32)> = (cx - r..=cx + r)
            .flat_map(|x| (cz - r..=cz + r).map(move |z| (x, z)))
            .filter_map(|c| self.cells.get(&c))
            .flatten()
            .filter_map(|&i| {
                let p = self.nodes[i as usize];
                let (to, rise) = (perch.at - p, perch.at.y - p.y);
                let to = Vec3::new(to.x, 0.0, to.z);
                let along = -to.dot(perch.n);
                (!island[i as usize]
                    && (0.8..=4.8).contains(&rise)
                    && (KICK_NEAR..=KICK_RUN).contains(&along)
                    && along >= 0.8 * to.length())
                .then_some(((along - 3.0).abs(), i))
            })
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        v.into_iter().take(KICK_STARTS).map(|(_, i)| i).collect()
    }

    /// A [`Kick`] from a standstill at `p`, running at the perch: jump at the ticks that leave
    /// 0.5..2.2 m to the wall, face the run heading or 0.4 rad to either side in the air, wall-run
    /// up the wall and kick off it ([`Nav::kick_wall`]). The first script that lands on an island
    /// floor and keeps doing so from every start `KICK_TOL` metres off is taken.
    fn kick_run(
        &self,
        col: &MapCollision,
        island: &[bool],
        p: Vec3,
        perch: &Perch,
        j: u32,
    ) -> Option<(Kick, u32)> {
        let run = yaw_of(perch.at - p);
        let mut pw = Pawn::new(p);
        for jump in 0..KICK_JUMP {
            let rest = (pw.pos - perch.at).dot(perch.n);
            if (0.5..=2.2).contains(&rest) && jump % 2 == 0 {
                let mut q = pw;
                q.step(col, run, true);
                for b in [0.0, 0.4, -0.4] {
                    let k = Kick {
                        run,
                        wall: run + b,
                        jump,
                        kick: 0,
                    };
                    if let Some(found) = self.kick_wall(col, island, &[j], p, q, k) {
                        return Some(found);
                    }
                }
            }
            pw.step(col, run, false);
            if !pw.grounded || rest < 0.0 {
                break;
            }
        }
        None
    }

    /// `q` is the pawn just after the jump of `k`: it flies facing `k.wall` until a wall run up
    /// a wall begins, then each following tick is tried as the kick tick.
    fn kick_wall(
        &self,
        col: &MapCollision,
        island: &[bool],
        goals: &[u32],
        from: Vec3,
        mut q: Pawn,
        mut k: Kick,
    ) -> Option<(Kick, u32)> {
        let mut n = k.jump;
        while !matches!(q.air, Air::Run(_, false, _)) {
            if q.grounded || matches!(q.air, Air::Run(..)) || n >= k.jump + KICK_AIR {
                return None;
            }
            n += 1;
            q.step(col, k.wall, false);
        }
        loop {
            n += 1;
            if q.grounded || n >= k.jump + KICK_AIR {
                return None;
            }
            let mut r = q;
            r.step(col, k.wall, true);
            if matches!(r.air, Air::Kick(_)) && self.reaches(&r, goals) {
                k.kick = n;
                if let Some(to) = self.kick_replay(col, island, from, &k) {
                    return Some((k, to));
                }
            }
            q.step(col, k.wall, false);
        }
    }

    /// Whether the kicked pawn `r`, flying free (walls and ceilings ignored: [`Nav::lands`]
    /// checks those), comes down within a metre of one of the `goals`.
    fn reaches(&self, r: &Pawn, goals: &[u32]) -> bool {
        goals.iter().any(|&j| {
            let g = self.nodes[j as usize];
            let disc = WALL_UP * WALL_UP - 2.0 * GRAVITY * (g.y - r.pos.y);
            let t = (WALL_UP + disc.max(0.0).sqrt()) / GRAVITY;
            disc >= 0.0
                && Vec2::new(r.pos.x + r.hv.x * t - g.x, r.pos.z + r.hv.z * t - g.z).length() < 1.0
        })
    }

    /// The island floor node a standstill at `from` and `k` lands on, also from every start
    /// `KICK_TOL` metres to either side.
    fn kick_replay(
        &self,
        col: &MapCollision,
        island: &[bool],
        from: Vec3,
        k: &Kick,
    ) -> Option<u32> {
        let go = |from: Vec3| {
            let mut q = Pawn::new(from);
            for n in 0..=k.kick {
                let (yaw, jump) = k.input(n);
                q.step(col, yaw, jump);
            }
            self.lands(col, island, from, q, k.wall)
        };
        let to = go(from)?;
        (0..8)
            .all(|c| {
                let a = c as f32 * TAU / 8.0;
                go(from + Vec3::new(a.cos(), 0.0, a.sin()) * KICK_TOL).is_some()
            })
            .then_some(to)
    }

    /// The pawn `q` (just kicked, facing `yaw`) falls to a floor; then it walks to the floor
    /// node it landed on as a bot does. That node when it is on an island, `KICK_RISE` above
    /// `from`.
    fn lands(
        &self,
        col: &MapCollision,
        island: &[bool],
        from: Vec3,
        mut q: Pawn,
        yaw: f32,
    ) -> Option<u32> {
        for _ in 0..KICK_FLIGHT {
            if q.grounded {
                break;
            }
            q.step(col, yaw, false);
            if q.pos.y < from.y {
                return None;
            }
        }
        let j = self.snap(q.pos).filter(|_| q.grounded)?;
        let goal = self.nodes[j as usize];
        if !island[j as usize] || goal.y < from.y + KICK_RISE {
            return None;
        }
        for _ in 0..60 {
            let d = Vec3::new(goal.x - q.pos.x, 0.0, goal.z - q.pos.z);
            q.step(col, yaw_of(d), false);
        }
        let d = Vec2::new(goal.x - q.pos.x, goal.z - q.pos.z).length();
        (q.grounded && d < SNAP_XZ && (q.pos.y - goal.y).abs() < SNAP_Y).then_some(j)
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
                    kick: None,
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
                    kick: None,
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
                    kick: None,
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
    /// node. Run it with [`Nav::advance`], a few thousand expansions per frame. `kicks`: wall kick
    /// climbs may be used (a bot can do them, a monster cannot).
    pub fn search(&self, from: Vec3, to: Vec3, kicks: bool) -> Option<Search> {
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
            kicks,
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
                if self.broken.contains(&(i, l.to)) || l.kind == Kind::Kick && !q.kicks {
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
                kick: l.and_then(|l| l.kick),
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
        self.search(from, to, false)
            .and_then(|mut q| self.advance(&mut q, &mut budget))
            .unwrap_or_default()
    }

    /// The nodes reachable from `i` in one move, for diagnostics.
    pub fn neighbors(&self, i: u32) -> impl Iterator<Item = (u32, Kind)> + '_ {
        self.links[i as usize].iter().map(|l| (l.to, l.kind))
    }
}

/// `f` of every node id `0..n`, on all cores, in order.
fn par<T: Send>(n: usize, f: impl Fn(u32) -> T + Sync) -> Vec<T> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let ids: Vec<u32> = (0..n as u32).collect();
    std::thread::scope(|s| {
        let workers: Vec<_> = ids
            .chunks(n.div_ceil(threads).max(1))
            .map(|c| {
                let f = &f;
                s.spawn(move || c.iter().map(|&i| f(i)).collect::<Vec<_>>())
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().expect("nav worker"))
            .collect()
    })
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
    /// ignored by default: `cargo test --release routing_pairs -- --ignored --nocapture`. Routes
    /// are planned as a bot plans them (wall kicks included); `GUNZ_NOKICK=1` leaves the wall
    /// kick climbs out of the graph.
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
            let nav = if std::env::var("GUNZ_NOKICK").is_ok() {
                Nav::floors(&col, min, max)
            } else {
                Nav::new(&col, min, max)
            };
            let built = t.elapsed().as_secs_f32();
            let spawns: Vec<Vec3> = level.spawn_points().iter().map(|s| s.0).collect();
            let n = spawns.len();
            let (mut ok, mut total, mut worst) = (0, 0, 0.0f32);
            for (i, &a) in spawns.iter().enumerate() {
                for j in [(i + 1) % n, (i + n / 2) % n] {
                    let t = std::time::Instant::now();
                    let mut budget = usize::MAX;
                    let route = nav
                        .search(a, spawns[j], std::env::var("GUNZ_NOKICK").is_err())
                        .and_then(|mut q| nav.advance(&mut q, &mut budget))
                        .unwrap_or_default();
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
            let kicks: Vec<_> = nav
                .links
                .iter()
                .enumerate()
                .flat_map(|(i, l)| l.iter().map(move |l| (i, l)))
                .filter(|(_, l)| l.kind == Kind::Kick)
                .collect();
            for (i, l) in kicks.iter().take(6) {
                println!(
                    "  kick {:.2} -> {:.2} {:?}",
                    nav.nodes[*i],
                    nav.nodes[l.to as usize],
                    l.kick.unwrap()
                );
            }
            println!(
                "{name:16} {ok:4}/{total:<4} nodes {:6} kicks {:3} build {built:5.1}s worst route {:.1} ms",
                nav.nodes.len(),
                kicks.len(),
                worst * 1000.0
            );
            (ok_all, all) = (ok_all + ok, all + total);
        }
        println!("TOTAL {ok_all}/{all}");
    }

    /// Mansion's wall kick links ([`Kind::Kick`]; needs the retail install, `GUNZ_GAME=<dir>`):
    /// there is at least one, and a bot that replays it on the clock ([`Kick::input`] by the sum
    /// of the frame times, as `bot.rs` does) lands on the island at 30..240 Hz and within +-1 ms
    /// of jitter at 60 Hz, though the search used 1/60 s steps only.
    /// `cargo test --release kick_links -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn kick_links() {
        let Ok(game) = std::env::var("GUNZ_GAME") else {
            return;
        };
        let vfs = Vfs::mount(&game).unwrap();
        let rs = map::find_rs(&vfs, "mansion").unwrap();
        let col = MapCollision::load(&vfs, &rs).unwrap();
        let map = map::load(&vfs, &rs).unwrap();
        let (mut min, mut max) = (Vec3::MAX, Vec3::MIN);
        for v in &map.vertices {
            let p = Vec3::from(to_bevy(v.pos)) * SCALE;
            (min, max) = (min.min(p), max.max(p));
        }
        let mut nav = Nav::floors(&col, min, max);
        let island = nav.islands();
        nav.add_kicks(&col);
        let mut rng = 12345u32;
        let mut jitter = move || {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            1.0 / 60.0 + ((rng >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.002
        };
        let mut links = 0;
        for (i, ls) in nav.links.iter().enumerate() {
            for (l, k) in ls.iter().filter_map(|l| Some((l, l.kick?))) {
                links += 1;
                let from = nav.nodes[i];
                let run = |dt: &mut dyn FnMut() -> f32| {
                    let mut q = Pawn::new(from);
                    let (mut t, mut next) = (0.0f32, 0u32);
                    loop {
                        let tick = (t / PDT + 1e-3) as u32;
                        let jump = (next..=tick).any(|n| k.input(n).1);
                        q.dt = dt();
                        q.step(&col, k.input(tick).0, jump);
                        (t, next) = (t + q.dt, tick + 1);
                        if tick >= k.kick {
                            break;
                        }
                    }
                    nav.lands(&col, &island, from, q, k.wall)
                };
                for hz in [30.0, 60.0, 144.0, 240.0] {
                    let to = run(&mut || 1.0 / hz);
                    println!(
                        "{from:.2} -> {:.2} at {hz} Hz: {to:?}",
                        nav.nodes[l.to as usize]
                    );
                    assert!(to.is_some(), "{from:?} at {hz} Hz");
                }
                let ok = (0..40).filter(|_| run(&mut jitter).is_some()).count();
                println!("{from:.2} jitter +-1 ms: {ok}/40");
                assert!(ok >= 36);
            }
        }
        assert!(links > 0, "Mansion has no wall kick link");
    }
}
