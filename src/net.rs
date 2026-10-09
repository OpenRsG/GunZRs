//! LAN multiplayer (`gunz-play --host`, `gunz-play --join ADDR|lan`). The host runs the whole
//! match (bots, mode rules, damage, deaths, scores); every player who joins is a [`Remote`] actor
//! there. Our own protocol over TCP: frames of a little-endian `u32` length and a payload that
//! starts with a tag byte. A UDP probe on the same port finds the host on the LAN.
//!
//! A client moves its own player itself (its feet position is taken as is) and sends its
//! [`Intent`] every frame. The host answers every frame with a snapshot of every actor (intent,
//! feet, health, score, team, ammo, name) plus the kills and wounds. Between snapshots the client
//! feeds each actor's intent through the normal actor controller, so animations, shots, rockets
//! and effects play locally; only the host decides damage (`combat::apply_damage` does not run
//! on a client). Only the respawning deathmatch modes are played over the LAN ([`supported`]):
//! the round modes' rules would need their round state mirrored too.

use crate::{
    actor::{Actor, ActorData, ActorSpawner, ActorSpec, EMOTES, pick_spawn, yaw_of},
    combat::Wounded,
    game::{Bot, Dead, Intent, Killed, Loadout, Player, Remote, Score, Team, Vitals},
    menu::Mode,
    session::{Clock, Rules, freeze},
};
use bevy::prelude::*;
use std::{
    collections::{HashMap, HashSet},
    io::{self, ErrorKind, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket},
    time::{Duration, Instant},
};

/// TCP port of the host, and the UDP port it answers LAN probes on.
pub const PORT: u16 = 7790;
const MAGIC: &[u8; 4] = b"GZRS";
const VERSION: u16 = 1;
const PROBE: &[u8] = b"GZRS?";
const ANSWER: &[u8] = b"GZRS!";
/// A peer that lets this much unsent data pile up has stalled: it is dropped.
const BACKLOG: usize = 1 << 20;
/// Largest frame accepted (a snapshot of 32 actors is about 4 KB).
const MAX_FRAME: usize = 1 << 16;

// Frame tags. Client -> host:
const HELLO: u8 = 1;
const INPUT: u8 = 2;
// Host -> client:
const WELCOME: u8 = 10;
const REFUSE: u8 = 11;
const SPAWN: u8 = 12;
const GONE: u8 = 13;
const STATE: u8 = 14;
const KILL: u8 = 15;
const WOUND: u8 = 16;

/// What `--host` accepts.
pub const MODES: &str = "LAN matches play dm, tdm, gladiator or team-gladiator";

pub fn supported(mode: Mode) -> bool {
    matches!(
        mode,
        Mode::Deathmatch | Mode::Team | Mode::Gladiator | Mode::TeamGladiator
    )
}

/// Present in a LAN match, host or client.
#[derive(Resource)]
pub struct Lan;

pub struct NetPlugin;

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            (
                host_recv.run_if(resource_exists::<Host>),
                client_recv.run_if(resource_exists::<Client>),
            ),
        )
        .add_systems(
            PostUpdate,
            (
                host_send.run_if(resource_exists::<Host>),
                client_send.run_if(resource_exists::<Client>),
            ),
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Encoding

struct W(Vec<u8>);

impl W {
    fn new(tag: u8) -> Self {
        W(vec![tag])
    }
    fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn f32(&mut self, v: f32) -> &mut Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn v3(&mut self, v: Vec3) -> &mut Self {
        self.f32(v.x).f32(v.y).f32(v.z)
    }
    /// At most 255 bytes, cut at a character boundary.
    fn str(&mut self, s: &str) -> &mut Self {
        let mut end = s.len().min(255);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        self.u8(end as u8);
        self.0.extend_from_slice(&s.as_bytes()[..end]);
        self
    }
}

/// Reads a frame; every getter is `None` past its end (or for a non-finite float), so a short
/// or garbled frame from a peer is rejected instead of trusted.
struct R<'a>(&'a [u8]);

impl<'a> R<'a> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let (a, b) = self.0.split_first_chunk::<N>()?;
        self.0 = b;
        Some(*a)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take::<1>().map(|b| b[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take().map(u16::from_le_bytes)
    }
    fn u32(&mut self) -> Option<u32> {
        self.take().map(u32::from_le_bytes)
    }
    fn u64(&mut self) -> Option<u64> {
        self.take().map(u64::from_le_bytes)
    }
    fn f32(&mut self) -> Option<f32> {
        self.take()
            .map(f32::from_le_bytes)
            .filter(|v| v.is_finite())
    }
    fn v3(&mut self) -> Option<Vec3> {
        Some(Vec3::new(self.f32()?, self.f32()?, self.f32()?))
    }
    fn str(&mut self) -> Option<String> {
        let n = self.u8()? as usize;
        let (s, rest) = self.0.split_at_checked(n)?;
        self.0 = rest;
        String::from_utf8(s.to_vec()).ok()
    }
}

fn team_code(t: Option<Team>) -> u8 {
    match t {
        None => 0,
        Some(Team::Red) => 1,
        Some(Team::Blue) => 2,
        Some(Team::Duel(n)) => 3u8.saturating_add(n),
    }
}

fn team_of(c: u8) -> Option<Team> {
    match c {
        0 => None,
        1 => Some(Team::Red),
        2 => Some(Team::Blue),
        n => Some(Team::Duel(n - 3)),
    }
}

/// One actor's state in a host snapshot. A client sends its own player in the same form as its
/// input (the host only takes the feet, intent, weapon slot and death flag from it).
#[derive(Debug, Default)]
struct Snap {
    id: u64,
    pos: Vec3,
    /// `slot` is not sent: `current` says which weapon is out.
    intent: Intent,
    current: u8,
    dead: bool,
    /// hp, ap, max hp, max ap.
    vitals: [f32; 4],
    /// kills, deaths.
    score: [u32; 2],
    team: Option<Team>,
    /// Magazine and reserve per loadout slot.
    ammo: Vec<(u32, u32)>,
    name: String,
}

impl Snap {
    fn write(&self, w: &mut W) {
        let i = &self.intent;
        let flags = [i.jump, i.attack, i.reload, i.guard, i.taunt, self.dead]
            .iter()
            .enumerate()
            .fold(0u8, |f, (b, on)| f | (*on as u8) << b);
        let emote = match i.emote {
            Some("taunt") => 6,
            Some(e) => EMOTES
                .iter()
                .position(|x| x.0 == e)
                .map_or(0, |p| p as u8 + 1),
            None => 0,
        };
        w.u64(self.id)
            .v3(self.pos)
            .f32(i.walk.x)
            .f32(i.walk.y)
            .f32(i.yaw)
            .f32(i.pitch)
            .u8(flags)
            .u8(emote)
            .u8(self.current);
        for v in self.vitals {
            w.f32(v);
        }
        w.u32(self.score[0])
            .u32(self.score[1])
            .u8(team_code(self.team))
            .u8(self.ammo.len().min(255) as u8);
        for &(m, r) in self.ammo.iter().take(255) {
            w.u32(m).u32(r);
        }
        w.str(&self.name);
    }

    fn read(r: &mut R) -> Option<Self> {
        let (id, pos) = (r.u64()?, r.v3()?);
        let (x, y, yaw, pitch) = (r.f32()?, r.f32()?, r.f32()?, r.f32()?);
        let (flags, emote, current) = (r.u8()?, r.u8()?, r.u8()?);
        let bit = |b: u8| flags & 1 << b != 0;
        let intent = Intent {
            walk: Vec2::new(x, y).clamp(Vec2::NEG_ONE, Vec2::ONE),
            yaw,
            pitch,
            jump: bit(0),
            attack: bit(1),
            reload: bit(2),
            slot: None,
            guard: bit(3),
            taunt: bit(4),
            emote: match emote {
                1..=5 => Some(EMOTES[emote as usize - 1].0),
                6 => Some("taunt"),
                _ => None,
            },
        };
        let vitals = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
        let score = [r.u32()?, r.u32()?];
        let team = team_of(r.u8()?);
        let ammo = (0..r.u8()?)
            .map(|_| Some((r.u32()?, r.u32()?)))
            .collect::<Option<_>>()?;
        Some(Snap {
            id,
            pos,
            intent,
            current,
            dead: bit(5),
            vitals,
            score,
            team,
            ammo,
            name: r.str()?,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Transport

/// A non-blocking TCP connection with its unparsed input and unsent output.
struct Link {
    stream: TcpStream,
    rx: Vec<u8>,
    tx: Vec<u8>,
    closed: bool,
}

impl Link {
    fn new(stream: TcpStream) -> io::Result<Self> {
        stream.set_nodelay(true)?;
        stream.set_nonblocking(true)?;
        Ok(Link {
            stream,
            rx: Vec::new(),
            tx: Vec::new(),
            closed: false,
        })
    }

    fn send(&mut self, frame: &W) {
        if self.tx.len() > BACKLOG {
            self.closed = true;
            return;
        }
        self.tx.extend((frame.0.len() as u32).to_le_bytes());
        self.tx.extend_from_slice(&frame.0);
    }

    /// Writes as much of the output as the socket takes now.
    fn flush(&mut self) {
        while !self.tx.is_empty() && !self.closed {
            match self.stream.write(&self.tx) {
                Ok(0) => self.closed = true,
                Ok(n) => drop(self.tx.drain(..n)),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(_) => self.closed = true,
            }
        }
    }

    /// Reads what arrived and returns the complete frames.
    fn recv(&mut self) -> Vec<Vec<u8>> {
        let mut buf = [0u8; 16384];
        while !self.closed {
            match self.stream.read(&mut buf) {
                Ok(0) => self.closed = true,
                Ok(n) => self.rx.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(_) => self.closed = true,
            }
        }
        let (mut frames, mut at) = (Vec::new(), 0);
        while let Some(len) = self.rx.get(at..at + 4) {
            let n = u32::from_le_bytes(len.try_into().unwrap()) as usize;
            if n > MAX_FRAME {
                self.closed = true;
                break;
            }
            let Some(frame) = self.rx.get(at + 4..at + 4 + n) else {
                break;
            };
            frames.push(frame.to_vec());
            at += 4 + n;
        }
        self.rx.drain(..at);
        frames
    }
}

// ---------------------------------------------------------------------------------------------
// Host

#[derive(Resource)]
pub struct Host {
    /// The map as the clients name it to load it.
    map: String,
    listener: TcpListener,
    probe: UdpSocket,
    peers: Vec<Peer>,
}

struct Peer {
    link: Link,
    /// Its actor, once its hello was accepted.
    actor: Option<Entity>,
    /// Actors it has been told about.
    known: HashSet<Entity>,
}

impl Host {
    /// Listens on [`PORT`] (TCP) and answers LAN probes on it (UDP).
    pub fn bind(map: &str) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, PORT))?;
        listener.set_nonblocking(true)?;
        let probe = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, PORT))?;
        probe.set_nonblocking(true)?;
        Ok(Host {
            map: map.into(),
            listener,
            probe,
            peers: Vec::new(),
        })
    }
}

/// Accepts connections and probes, admits hellos and applies the clients' input.
#[allow(clippy::too_many_arguments)]
fn host_recv(
    mut host: ResMut<Host>,
    mut spawner: ActorSpawner,
    data: Res<ActorData>,
    rules: Res<Rules>,
    clock: Res<Clock>,
    mut actors: Query<(
        Entity,
        &mut Transform,
        &mut Intent,
        &Loadout,
        &Name,
        Has<Dead>,
    )>,
) {
    let host = &mut *host;
    while let Ok((stream, from)) = host.listener.accept() {
        match Link::new(stream) {
            Ok(link) => {
                info!("lan: {from} connected");
                host.peers.push(Peer {
                    link,
                    actor: None,
                    known: HashSet::new(),
                });
            }
            Err(e) => warn!("lan: {from}: {e}"),
        }
    }
    let mut buf = [0u8; 16];
    while let Ok((n, from)) = host.probe.recv_from(&mut buf) {
        if &buf[..n] == PROBE {
            let _ = host.probe.send_to(ANSWER, from);
        }
    }
    let mut names: Vec<String> = actors.iter().map(|q| q.4.to_string()).collect();
    let others: Vec<(Entity, Vec3)> = actors.iter().map(|q| (q.0, q.1.translation)).collect();
    for peer in &mut host.peers {
        for frame in peer.link.recv() {
            let mut r = R(&frame);
            match (r.u8(), peer.actor) {
                (Some(HELLO), None) => {
                    let admitted = admit(&mut r, &data, &rules, &clock, &mut names, &others);
                    let (w, refused) = match admitted {
                        Ok((name, spec)) => {
                            let (at, yaw) = (spec.pos, spec.yaw);
                            let e = spawner.spawn(ActorSpec { name, ..spec });
                            spawner.commands.entity(e).remove::<Bot>().insert(Remote);
                            peer.actor = Some(e);
                            let mut w = W::new(WELCOME);
                            w.u64(e.to_bits())
                                .str(&host.map)
                                .str(rules.mode.arg())
                                .u32(rules.time_limit.unwrap_or(0))
                                .u32(rules.kill_limit.unwrap_or(0))
                                .f32(rules.respawn)
                                .f32(rules.protect)
                                .v3(at)
                                .f32(yaw);
                            (w, false)
                        }
                        Err(why) => {
                            warn!("lan: refused a player: {why}");
                            let mut w = W::new(REFUSE);
                            w.str(&why);
                            (w, true)
                        }
                    };
                    peer.link.send(&w);
                    peer.link.flush();
                    peer.link.closed |= refused;
                }
                (Some(INPUT), Some(e)) => {
                    let Some(s) = Snap::read(&mut r) else {
                        peer.link.closed = true;
                        continue;
                    };
                    let Ok((_, mut tf, mut intent, load, _, dead)) = actors.get_mut(e) else {
                        continue;
                    };
                    let cur = s.current as usize;
                    let slot = (cur != load.current && cur < load.slots.len()).then_some(cur);
                    *intent = Intent { slot, ..s.intent };
                    // a corpse stays where it fell; the respawn point is the client's
                    if !s.dead && !dead {
                        tf.translation = s.pos;
                    }
                }
                _ => peer.link.closed = true,
            }
        }
    }
    host.peers.retain(|p| {
        if !p.link.closed {
            return true;
        }
        if let Some(e) = p.actor
            && let Ok(mut gone) = spawner.commands.get_entity(e)
        {
            let who = actors.get(e).map_or("?".into(), |q| q.4.to_string());
            info!("lan: {who} left");
            gone.despawn();
        }
        false
    });
}

/// Checks a hello: the protocol version, a running match, the character and its weapons (an
/// unknown item would panic the spawner). Returns a unique name and where to spawn.
fn admit(
    r: &mut R,
    data: &ActorData,
    rules: &Rules,
    clock: &Clock,
    names: &mut Vec<String>,
    others: &[(Entity, Vec3)],
) -> Result<(String, ActorSpec), String> {
    let bad = || "bad hello".to_string();
    if r.take::<4>().as_ref() != Some(MAGIC) || r.u16() != Some(VERSION) {
        return Err(format!("not a GunZRs {VERSION} client"));
    }
    let name = r.str().ok_or_else(bad)?;
    let woman = r.u8().ok_or_else(bad)? != 0;
    let outfit = r.u8().ok_or_else(bad)?;
    let kit: Vec<u32> = (0..r.u8().ok_or_else(bad)?.min(8))
        .map(|_| r.u32())
        .collect::<Option<_>>()
        .ok_or_else(bad)?;
    if clock.over.is_some() {
        return Err("the match is over".into());
    }
    let parts = data.character(woman).parts.len();
    let kit = kit
        .into_iter()
        .filter(|&id| {
            data.items
                .get(id)
                .is_some_and(|i| i.weapon.is_some() && data.items.model(i).is_some())
        })
        .collect();
    let base: String = name.chars().filter(|c| !c.is_control()).take(20).collect();
    let base = match base.trim() {
        "" => "Player",
        b => b,
    };
    let mut name = base.to_string();
    for n in 2.. {
        if !names.contains(&name) {
            break;
        }
        name = format!("{base} {n}");
    }
    names.push(name.clone());
    let (pos, dir) = pick_spawn(&data.spawns, Entity::PLACEHOLDER, others);
    info!("lan: {name} joins {}", rules.mode.arg());
    Ok((
        name,
        ActorSpec {
            name: String::new(),
            pos: pos + Vec3::Y * 0.1,
            yaw: yaw_of(dir),
            woman,
            loadout: kit,
            outfit: Some(outfit as usize).filter(|&p| p < parts),
            bot: true,
        },
    ))
}

/// Tells every joined client about new and removed actors, then sends the snapshot, kills and
/// wounds of this frame.
#[allow(clippy::type_complexity)]
fn host_send(
    mut host: ResMut<Host>,
    clock: Res<Clock>,
    actors: Query<(
        Entity,
        &Actor,
        &Name,
        &Transform,
        &Intent,
        &Vitals,
        &Score,
        &Loadout,
        Option<&Team>,
        Has<Dead>,
        Has<Bot>,
    )>,
    mut killed: MessageReader<Killed>,
    mut wounded: MessageReader<Wounded>,
) {
    let mut events = Vec::new();
    for k in killed.read() {
        let mut w = W::new(KILL);
        w.u64(k.victim.to_bits())
            .u64(k.killer.to_bits())
            .u32(k.item);
        events.push(w);
    }
    for h in wounded.read() {
        let mut w = W::new(WOUND);
        w.u64(h.target.to_bits())
            .u64(h.attacker.to_bits())
            .f32(h.hp)
            .f32(h.ap);
        events.push(w);
    }
    if host.peers.is_empty() {
        return;
    }
    let mut state = W::new(STATE);
    state
        .f32(clock.elapsed)
        .u16(actors.iter().len().min(u16::MAX as usize) as u16);
    for (e, _, name, tf, intent, v, s, load, team, dead, _) in &actors {
        Snap {
            id: e.to_bits(),
            pos: tf.translation,
            intent: intent.clone(),
            current: load.current as u8,
            dead,
            vitals: [v.hp, v.ap, v.max_hp, v.max_ap],
            score: [s.kills, s.deaths],
            team: team.copied(),
            ammo: load.slots.iter().map(|s| (s.magazine, s.reserve)).collect(),
            name: name.to_string(),
        }
        .write(&mut state);
    }
    for peer in host.peers.iter_mut().filter(|p| p.actor.is_some()) {
        let (known, link) = (&mut peer.known, &mut peer.link);
        for (e, a, name, tf, intent, .., team, _, bot) in &actors {
            if known.insert(e) {
                let mut w = W::new(SPAWN);
                w.u64(e.to_bits())
                    .u8(!bot as u8)
                    .str(name)
                    .u8(a.woman as u8)
                    .u8(a.outfit.map_or(u8::MAX, |p| p.min(254) as u8))
                    .u8(a.kit.len().min(255) as u8);
                for &id in a.kit.iter().take(255) {
                    w.u32(id);
                }
                w.v3(tf.translation)
                    .f32(intent.yaw)
                    .u8(team_code(team.copied()));
                link.send(&w);
            }
        }
        known.retain(|&e| {
            let here = actors.contains(e);
            if !here {
                let mut w = W::new(GONE);
                w.u64(e.to_bits());
                link.send(&w);
            }
            here
        });
        link.send(&state);
        for w in &events {
            link.send(w);
        }
        link.flush();
    }
}

// ---------------------------------------------------------------------------------------------
// Client

/// A client's connection to its host and the host's actors as local entities.
#[derive(Resource)]
pub struct Client {
    link: Link,
    /// The host's id for this client's player.
    me: u64,
    ids: HashMap<u64, Entity>,
    /// Whether the host had each actor dead in the last snapshot (deaths and respawns are
    /// acted on when this changes).
    dead: HashMap<u64, bool>,
}

/// What the host told a joining client: the match to set up.
pub struct Welcome {
    pub map: String,
    pub mode: Mode,
    pub time_limit: Option<u32>,
    pub kill_limit: Option<u32>,
    pub respawn: f32,
    pub protect: f32,
    /// Where the player starts (feet, Bevy metres) and its facing.
    pub at: Vec3,
    pub yaw: f32,
}

/// Finds a host on the LAN: broadcasts a probe (and asks this machine) for 3 seconds.
fn discover() -> Result<SocketAddr, String> {
    let fail = |e: io::Error| format!("LAN probe: {e}");
    let s = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).map_err(fail)?;
    s.set_broadcast(true).map_err(fail)?;
    s.set_read_timeout(Some(Duration::from_millis(250)))
        .map_err(fail)?;
    let until = Instant::now() + Duration::from_secs(3);
    let mut buf = [0u8; 16];
    while Instant::now() < until {
        for ip in [Ipv4Addr::BROADCAST, Ipv4Addr::LOCALHOST] {
            let _ = s.send_to(PROBE, (ip, PORT));
        }
        if let Ok((n, from)) = s.recv_from(&mut buf)
            && &buf[..n] == ANSWER
        {
            return Ok(SocketAddr::new(from.ip(), PORT));
        }
    }
    Err(format!("no LAN host answered on UDP port {PORT}"))
}

/// Connects to the host at `addr` (`HOST[:PORT]`, or `lan` to look for one), says hello with
/// this player's look and weapons and waits for the match to join.
pub fn join(
    addr: &str,
    name: &str,
    woman: bool,
    outfit: Option<usize>,
    kit: &[u32],
) -> Result<(Welcome, Client), String> {
    let to = if addr == "lan" {
        discover()?
    } else {
        let full = match addr.contains(':') {
            true => addr.to_string(),
            false => format!("{addr}:{PORT}"),
        };
        full.to_socket_addrs()
            .ok()
            .and_then(|mut a| a.next())
            .ok_or_else(|| format!("cannot resolve {full}"))?
    };
    let stream = TcpStream::connect_timeout(&to, Duration::from_secs(3))
        .map_err(|e| format!("{to}: {e}"))?;
    let mut link = Link::new(stream).map_err(|e| format!("{to}: {e}"))?;
    let mut w = W::new(HELLO);
    w.0.extend_from_slice(MAGIC);
    w.u16(VERSION)
        .str(name)
        .u8(woman as u8)
        .u8(outfit.map_or(u8::MAX, |p| p.min(254) as u8))
        .u8(kit.len().min(8) as u8);
    for &id in kit.iter().take(8) {
        w.u32(id);
    }
    link.send(&w);
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until && !link.closed {
        link.flush();
        // Frames after the welcome stay queued for the game's first update.
        let mut frames = link.recv().into_iter();
        if let Some(f) = frames.next() {
            let rest: Vec<u8> = frames
                .flat_map(|f| [(f.len() as u32).to_le_bytes().to_vec(), f].concat())
                .collect();
            link.rx.splice(0..0, rest);
            let mut r = R(&f);
            return match r.u8() {
                Some(WELCOME) => welcome(&mut r)
                    .map(|(me, w)| {
                        let client = Client {
                            link,
                            me,
                            ids: HashMap::new(),
                            dead: HashMap::new(),
                        };
                        (w, client)
                    })
                    .ok_or_else(|| format!("{to}: bad welcome")),
                Some(REFUSE) => Err(format!("{to} refused: {}", r.str().unwrap_or_default())),
                _ => Err(format!("{to}: not a GunZRs host")),
            };
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err(format!("{to}: no answer"))
}

fn welcome(r: &mut R) -> Option<(u64, Welcome)> {
    let me = r.u64()?;
    let map = r.str()?;
    let mode = r.str()?.parse().ok()?;
    let limit = |n: u32| (n > 0).then_some(n);
    Some((
        me,
        Welcome {
            map,
            mode,
            time_limit: limit(r.u32()?),
            kill_limit: limit(r.u32()?),
            respawn: r.f32()?,
            protect: r.f32()?,
            at: r.v3()?,
            yaw: r.f32()?,
        },
    ))
}

/// Applies what the host sent: new and removed actors, the snapshot (other actors' intent and
/// feet, everyone's health, score, team, ammo and name, deaths and respawns), kills and wounds.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn client_recv(
    mut client: ResMut<Client>,
    mut spawner: ActorSpawner,
    rules: Res<Rules>,
    mut clock: ResMut<Clock>,
    mut vtime: ResMut<Time<Virtual>>,
    player: Query<Entity, With<Player>>,
    mut actors: Query<(
        &mut Transform,
        &mut Intent,
        &mut Vitals,
        &mut Score,
        &mut Loadout,
        &mut Name,
        Option<&Team>,
        Option<&mut Dead>,
        Has<Player>,
    )>,
    mut killed: MessageWriter<Killed>,
    mut wounded: MessageWriter<Wounded>,
) {
    let client = &mut *client;
    if client.link.closed {
        return;
    }
    if !client.ids.contains_key(&client.me)
        && let Ok(p) = player.single()
    {
        client.ids.insert(client.me, p);
    }
    for frame in client.link.recv() {
        let mut r = R(&frame);
        let parsed = match r.u8() {
            Some(SPAWN) => spawn(&mut r, client, &mut spawner),
            Some(GONE) => r.u64().map(|id| {
                if let Some(e) = client.ids.remove(&id)
                    && let Ok(mut gone) = spawner.commands.get_entity(e)
                {
                    gone.despawn();
                }
                client.dead.remove(&id);
            }),
            Some(STATE) => (|| {
                let elapsed = r.f32()?;
                if clock.over.is_none() {
                    clock.elapsed = elapsed;
                }
                for _ in 0..r.u16()? {
                    let s = Snap::read(&mut r)?;
                    let Some(&e) = client.ids.get(&s.id) else {
                        continue;
                    };
                    let was = client.dead.insert(s.id, s.dead).unwrap_or(false);
                    let Ok((
                        mut tf,
                        mut intent,
                        mut v,
                        mut score,
                        mut load,
                        mut name,
                        team,
                        dead,
                        mine,
                    )) = actors.get_mut(e)
                    else {
                        continue;
                    };
                    if !mine {
                        let cur = s.current as usize;
                        let slot = (cur != load.current && cur < load.slots.len()).then_some(cur);
                        *intent = Intent { slot, ..s.intent };
                        tf.translation = s.pos;
                    }
                    [v.hp, v.ap, v.max_hp, v.max_ap] = s.vitals;
                    (score.kills, score.deaths) = (s.score[0], s.score[1]);
                    for (slot, &(m, res)) in load.slots.iter_mut().zip(&s.ammo) {
                        (slot.magazine, slot.reserve) = (m, res);
                    }
                    if name.as_str() != s.name {
                        *name = Name::new(s.name);
                    }
                    if team.copied() != s.team {
                        match s.team {
                            Some(t) => spawner.commands.entity(e).insert(t),
                            None => spawner.commands.entity(e).remove::<Team>(),
                        };
                    }
                    match (s.dead, was, dead) {
                        // the death animation; the mode sets the respawn wait (`dead_added`)
                        (true, false, None) => {
                            spawner.commands.entity(e).insert(Dead {
                                respawn: rules.respawn.max(0.1),
                            });
                        }
                        // the host respawned it: so does the actor controller, now
                        (false, true, Some(mut d)) => d.respawn = 0.0,
                        _ => {}
                    }
                }
                Some(())
            })(),
            // Kills and wounds of an actor this client never saw (it already left) are dropped.
            Some(KILL) => (|| {
                let (victim, killer, item) = (r.u64()?, r.u64()?, r.u32()?);
                if let Some(&victim) = client.ids.get(&victim) {
                    let killer = client.ids.get(&killer).copied().unwrap_or(victim);
                    killed.write(Killed {
                        victim,
                        killer,
                        item,
                    });
                }
                Some(())
            })(),
            Some(WOUND) => (|| {
                let (target, attacker, hp, ap) = (r.u64()?, r.u64()?, r.f32()?, r.f32()?);
                if let Some(&target) = client.ids.get(&target) {
                    let attacker = client.ids.get(&attacker).copied().unwrap_or(target);
                    wounded.write(Wounded {
                        target,
                        attacker,
                        hp,
                        ap,
                    });
                }
                Some(())
            })(),
            _ => None,
        };
        if parsed.is_none() {
            warn!("lan: bad frame from the host");
            client.link.closed = true;
            break;
        }
    }
    if client.link.closed {
        warn!("lan: lost the host");
        if clock.over.is_none() {
            clock.over = Some("HOST LEFT".into());
            freeze(&mut spawner.commands, &mut vtime, true);
        }
    }
}

/// A [`SPAWN`] frame: another actor of the host appears (this client's own player is already
/// there).
fn spawn(r: &mut R, client: &mut Client, spawner: &mut ActorSpawner) -> Option<()> {
    let (id, human, name, woman, outfit) =
        (r.u64()?, r.u8()? != 0, r.str()?, r.u8()? != 0, r.u8()?);
    let kit: Vec<u32> = (0..r.u8()?).map(|_| r.u32()).collect::<Option<_>>()?;
    let (pos, yaw, team) = (r.v3()?, r.f32()?, team_of(r.u8()?));
    if id == client.me || client.ids.contains_key(&id) {
        return Some(());
    }
    info!("lan: {name} is in the match");
    let e = spawner.spawn(ActorSpec {
        name,
        pos,
        yaw,
        woman,
        loadout: kit,
        outfit: (outfit != u8::MAX).then_some(outfit as usize),
        bot: true,
    });
    let mut ec = spawner.commands.entity(e);
    if human {
        ec.remove::<Bot>().insert(Remote);
    }
    if let Some(t) = team {
        ec.insert(t);
    }
    client.ids.insert(id, e);
    Some(())
}

/// Sends this client's player: feet, intent, weapon slot, dead or alive.
fn client_send(
    mut client: ResMut<Client>,
    player: Query<(&Transform, &Intent, &Loadout, Has<Dead>), With<Player>>,
) {
    if let Ok((tf, intent, load, dead)) = player.single() {
        let mut w = W::new(INPUT);
        Snap {
            pos: tf.translation,
            intent: intent.clone(),
            current: load.current as u8,
            dead,
            ..default()
        }
        .write(&mut w);
        client.link.send(&w);
    }
    client.link.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A snapshot survives the wire byte for byte, frames split across reads are reassembled,
    /// and a cut-off frame is rejected rather than half-read.
    #[test]
    fn snapshot_frames_round_trip() {
        let s = Snap {
            id: 0xdead_beef_0042,
            pos: Vec3::new(1.5, -2.0, 30.25),
            intent: Intent {
                walk: Vec2::new(0.0, -1.0),
                yaw: 1.25,
                pitch: -0.5,
                attack: true,
                guard: true,
                emote: Some("dance"),
                ..default()
            },
            current: 2,
            dead: true,
            vitals: [42.0, 0.0, 100.0, 50.0],
            score: [7, 3],
            team: Some(Team::Blue),
            ammo: vec![(0, 0), (6, 30)],
            name: "Bot 3".into(),
        };
        let mut w = W::new(STATE);
        s.write(&mut w);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut tx =
            Link::new(TcpStream::connect(listener.local_addr().unwrap()).unwrap()).unwrap();
        let mut rx = Link::new(listener.accept().unwrap().0).unwrap();
        tx.send(&w);
        tx.send(&w);
        // Deliver the second frame in two pieces.
        let cut = tx.tx.len() - 7;
        let tail = tx.tx.split_off(cut);
        tx.flush();
        let read = |rx: &mut Link, want: usize| {
            let until = Instant::now() + Duration::from_secs(2);
            let mut got = Vec::new();
            while got.len() < want && Instant::now() < until {
                got.extend(rx.recv());
            }
            got
        };
        let first = read(&mut rx, 1);
        assert_eq!(first.len(), 1, "the cut frame must wait for its tail");
        tx.tx = tail;
        tx.flush();
        let second = read(&mut rx, 1);
        for f in first.iter().chain(&second) {
            let mut r = R(f);
            assert_eq!(r.u8(), Some(STATE));
            let back = Snap::read(&mut r).unwrap();
            assert_eq!(format!("{back:?}"), format!("{s:?}"));
            assert!(r.0.is_empty());
        }
        // Every truncation of the payload fails to parse.
        for n in 1..w.0.len() {
            assert!(Snap::read(&mut R(&w.0[1..n])).is_none(), "cut at {n}");
        }
    }
}
