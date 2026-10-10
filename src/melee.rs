//! Melee combat: the sword state machine (slash combo, jump/dive slash, uppercut, charged
//! massive, guard) and its hit resolution. Per actor a [`Melee`] component decides which
//! retail clip plays when (`ActionRequest` to the actor controller) and when the blade lands
//! (`Strike`); [`resolve`] turns a strike into `Damage`, `Blocked`, knockback `Push` and the
//! victim's flinch. Constants marked *inferred* are not in the retail data (the executable is
//! packed): docs/formats.md "Melee".
//!
//! Controls (the same `Intent` bots write): a click (`attack` pressed) slashes, `attack`
//! held from the click charges; `guard` held raises the guard; `attack` while guarding is the
//! uppercut. Bots hold `attack` to chain slashes.

use crate::{
    actor::{ActorData, ActorSet},
    ani::FPS,
    col::MapCollision,
    combat::{HIT_RADIUS, SWITCH_DELAY, Vfx, is_melee, rnd},
    game::*,
};
use bevy::prelude::*;

/// Hit frame of a clip: the frame of the fastest sword-hand tip (`R Finger0Nub`, forward
/// kinematics on the `.ani` keys; probe `.local/py/hit.py`), *inferred* to be the moment the blade
/// meets the target. `(motion type, clip, frame)`; clips without an entry strike at 45 % of
/// their length. `system/animationevent.xml` has no player-clip events (quest-NPC sounds only,
/// docs/formats.md), so nothing in the data replaces these frames.
const STRIKE: &[(u32, &str, f32)] = &[
    (1, "attack1", 5.0),
    (1, "attack2", 7.0),
    (1, "attack3", 10.0),
    (1, "attack4", 10.0),
    (1, "attack_Jump", 5.0),
    (1, "uppercut", 8.0),
    (1, "slash", 12.0),
    (1, "jump_slash1", 9.0),
    (1, "jump_slash2", 16.0),
    (7, "attackS", 4.0),
    (7, "uppercut", 10.0),
    (7, "slash", 11.0),
    (7, "jump_slash1", 9.0),
    (7, "jump_slash2", 15.0),
    (12, "attack1", 8.0),
    (12, "attack2", 10.0),
    (12, "attack3", 9.0),
    (12, "attack4", 16.0),
    (12, "attack_Jump", 6.0),
    (12, "uppercut", 32.0),
    (12, "slash", 13.0),
    (12, "jump_slash1", 12.0),
    (13, "attack1", 5.0),
    (13, "attack2", 5.0),
    (13, "attack3", 6.0),
    (13, "attack4", 11.0),
    (13, "attack_Jump", 5.0),
    (13, "uppercut", 5.0),
    (13, "slash", 11.0),
    (13, "jump_slash1", 8.0),
    (14, "attack1", 3.0),
    (14, "attack2", 4.0),
    (14, "attack_Jump", 10.0),
    (14, "uppercut", 7.0),
];

fn strike_secs(motion: u32, clip: &str, clip_secs: f32) -> f32 {
    STRIKE
        .iter()
        .find(|s| s.0 == motion && s.1 == clip)
        .map_or(clip_secs * 0.45, |s| s.2 / FPS)
        .min(clip_secs)
}

const ATTACKS: [&str; 4] = ["attack1", "attack2", "attack3", "attack4"];
const RETURNS: [&str; 4] = ["attack1_ret", "attack2_ret", "attack3_ret", "attack4_ret"];

/// Seconds a click is remembered while the previous blow cannot be left yet (*inferred*).
const BUFFER: f32 = 0.4;
/// Seconds `attack` must be held from the click before the charge starts (*inferred*).
const CHARGE_HOLD: f32 = 0.5;
/// Charge needed for a massive swing, and for the full-power one, seconds into the `charge` clip
/// (*inferred*; the clip is 2 s).
const CHARGE_MIN: f32 = 0.4;
const CHARGE_FULL: f32 = 1.2;
/// Extra damage of a fully charged massive swing (*inferred*).
const MASSIVE_BONUS: f32 = 0.5;
/// Seconds after a cancelled blow during which the combo continues (K-style dash/jump cancels,
/// butterfly guard cancels; *inferred*).
const CANCEL_COMBO: f32 = 0.9;
/// Swing width in degrees when the item has no `angle` (all player melee items): per blow below.
/// Vertical reach of a blow around the chest (m), *inferred*.
const REACH_Y: f32 = 1.2;
/// Blade hits on map geometry within this distance spark (m), *inferred*.
const WALL_SPARK: f32 = 1.3;
/// A guard covers this half-angle in front of the guarder (degrees, *inferred*).
const GUARD_HALF: f32 = 90.0;
/// Knockback of the blows in horizontal m/s and launch speed (*inferred*; `zeffect.xml` has none
/// for blades). The actor controller starts the blast chain on `Push.y >= 5`.
const FLINCH_PUSH: f32 = 1.5;
const KNOCKDOWN_PUSH: f32 = 3.5;
const UPPERCUT_UP: f32 = 7.0;
const UPPERCUT_BACK: f32 = 1.0;
const MASSIVE_UP: f32 = 5.5;
const MASSIVE_BACK: f32 = 5.0;
/// Air juggle: a hit on an actor already in the blast chain lifts it again, at most this often.
const JUGGLE_UP: f32 = 5.5;
const MAX_JUGGLE: u8 = 3;
/// Seconds after a switch-cancelled air slash during which switching back to the blade needs no
/// draw delay (flash step / quick slash; *inferred*).
const FLASH_WINDOW: f32 = 1.5;

pub struct MeleePlugin;

impl Plugin for MeleePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Strike>().add_systems(
            Update,
            (init, drive, resolve)
                .chain()
                .after(ActorSet::Input)
                .before(ActorSet::Drive),
        );
    }
}

/// What a blow does to the actor it hits.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Effect {
    /// `damage` clip, short stagger.
    Flinch,
    /// `damage_down`, knocked flat.
    Knockdown,
    /// Thrown up into the blast chain.
    Launch,
    /// Thrown back into the blast chain; ignores the guard.
    Massive,
}

/// One kind of blow. `reach` multiplies the item's `range`, `arc` is the swing width in degrees
/// and `damage` multiplies the item's `damage` (all *inferred*).
#[derive(Clone, Copy, PartialEq, Debug)]
struct Blow {
    name: &'static str,
    reach: f32,
    arc: f32,
    damage: f32,
    effect: Effect,
}

const SLASH: Blow = Blow {
    name: "slash",
    reach: 1.0,
    arc: 90.0,
    damage: 1.0,
    effect: Effect::Flinch,
};
const FINISHER: Blow = Blow {
    name: "finisher",
    arc: 110.0,
    effect: Effect::Knockdown,
    ..SLASH
};
const AIR: Blow = Blow {
    name: "air slash",
    arc: 100.0,
    ..SLASH
};
const DIVE: Blow = Blow {
    name: "dive slash",
    reach: 1.1,
    arc: 120.0,
    damage: 1.2,
    ..SLASH
};
const SLAM: Blow = Blow {
    name: "landing slam",
    arc: 120.0,
    damage: 0.6,
    ..SLASH
};
const UPPERCUT: Blow = Blow {
    name: "uppercut",
    reach: 0.9,
    arc: 70.0,
    effect: Effect::Launch,
    ..SLASH
};
const MASSIVE: Blow = Blow {
    name: "massive",
    reach: 1.2,
    arc: 160.0,
    effect: Effect::Massive,
    ..SLASH
};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Stage {
    /// `guard_start`.
    Start,
    /// `guard_idle` (loops).
    Hold,
    /// `guard_block1/2` after a blocked blow.
    Block,
    /// `guard_block1_ret`: the return out of a `guard_block1` pose back to the guard (the
    /// block2 pose has no return clip in the data).
    BlockRet,
    /// `guard_cancel`.
    Release,
}

#[derive(Clone, Copy, PartialEq, Debug, Default)]
enum Phase {
    #[default]
    Idle,
    /// A blow clip: the blade lands `strike` seconds in; `ret` is its recovery clip.
    Blow {
        blow: Blow,
        strike: f32,
        ret: Option<&'static str>,
        /// Falls on until the actor lands (`jump_slash1` holds its last pose in the air).
        dive: bool,
    },
    /// A recovery clip (`attackN_ret`, `cancel`); blows and guard may start from `act_from`.
    Recover {
        act_from: f32,
    },
    /// `charge`, held while `attack` is down.
    Charge,
    Guard(Stage),
    /// `damage` / `damage_down` flinch; `stand` plays `blast_stand` afterwards.
    Hurt {
        stand: bool,
    },
}

/// Melee state of an actor (added to every actor with a `Loadout`).
#[derive(Component, Default)]
pub struct Melee {
    phase: Phase,
    /// When the current clip was requested and how long it lasts (`INFINITY` for loops).
    t0: f32,
    secs: f32,
    /// Index of the next combo slash and until when a fresh slash from `Idle` still continues it.
    combo: usize,
    combo_until: f32,
    /// A click is pending until this time.
    queued_until: f32,
    /// Earliest start of the next blow (the item's `delay`).
    ready: f32,
    /// When `attack` went down.
    down_at: f32,
    prev_attack: bool,
    /// The current blow already struck / charge already announced full / power of a charged blow.
    struck: bool,
    charged: bool,
    power: f32,
    /// Alternates `guard_block1/2` and `damage/damage2`.
    alt: bool,
    /// Until when drawing the blade again after an air slash was switch-cancelled is instant.
    flash: f32,
    /// The guard came out of a blow (butterfly / slide): the actor may move while it is up.
    slide: bool,
    /// That guard went up in the air (air butterfly): it may stay up there until the actor lands.
    air_guard: bool,
    /// Air hits taken since landing.
    juggle: u8,
    /// Item last seen in hand (weapon switches restart the state).
    item: u32,
}

/// A blow reached its hit frame.
#[derive(Message, Clone, Debug)]
struct Strike {
    attacker: Entity,
    item: u32,
    blow: Blow,
    power: f32,
}

fn init(mut commands: Commands, q: Query<Entity, (With<Loadout>, Without<Melee>)>) {
    for e in &q {
        commands.entity(e).insert(Melee::default());
    }
}

/// What the actor does next, decided from its phase and the input.
enum Next {
    Blow {
        blow: Blow,
        clip: &'static str,
        ret: Option<&'static str>,
        dive: bool,
        combo: usize,
        power: f32,
    },
    Guard(Stage),
    Charge,
    Recover {
        clip: &'static str,
        act_from: f32,
    },
    Idle,
}

#[allow(clippy::too_many_arguments)]
fn drive(
    time: Res<Time>,
    data: Res<ActorData>,
    mut commands: Commands,
    mut req: MessageWriter<ActionRequest>,
    mut strikes: MessageWriter<Strike>,
    mut fire: MessageWriter<Fire>,
    mut sound: MessageWriter<ActorSound>,
    mut actors: Query<(
        Entity,
        &mut Melee,
        &Intent,
        &Loadout,
        &Motor,
        &Transform,
        Option<&Acting>,
        Has<Dead>,
        Has<Bot>,
        Has<Guarding>,
        Option<&Name>,
    )>,
) {
    let now = time.elapsed_secs();
    for (e, mut m, intent, load, motor, tf, acting, dead, bot, has_guard, name) in &mut actors {
        let m = &mut *m;
        let who = name.map_or("?", |n| n.as_str());
        let click = intent.attack && !m.prev_attack;
        m.prev_attack = intent.attack;
        if click {
            m.down_at = now;
        }
        if !motor.blast && motor.grounded {
            m.juggle = 0;
        }
        let item = load.slots.get(load.current).map_or(0, |s| s.item);
        let weapon = data.items.get(item).and_then(|i| i.weapon.as_ref());
        if m.item != item {
            if m.item != 0 {
                // K-style: switching away in the air cancels the slash (and its delay), drawing
                // the blade again soon after needs no draw time (flash step).
                let air_cancel = matches!(m.phase, Phase::Blow { .. }) && !motor.grounded;
                let flash = weapon.is_some_and(|w| is_melee(w.kind)) && now < m.flash;
                if air_cancel {
                    m.flash = now + FLASH_WINDOW;
                    debug!("t={now:.2} melee {who}: tech: switch-cancelled air slash");
                } else if flash {
                    m.flash = 0.0;
                    debug!(
                        "t={now:.2} melee {who}: tech: flash step (quick slash after a switch cancel)"
                    );
                }
                m.ready = if air_cancel || flash {
                    now
                } else {
                    m.ready.max(now + SWITCH_DELAY)
                };
            }
            m.item = item;
            m.phase = Phase::Idle;
        }
        let Some(w) = weapon.filter(|w| is_melee(w.kind) && !dead && !motor.blast) else {
            m.phase = Phase::Idle;
            m.queued_until = 0.0;
            if has_guard {
                commands.entity(e).remove::<Guarding>();
            }
            continue;
        };
        sync_guard(&mut commands, e, m.phase, has_guard);
        let (motion, woman) = (w.kind.motion_type(), motor.woman);
        let secs = |clip: &str| data.clip_secs(woman, motion, clip);
        let t = now - m.t0;
        if click || (bot && intent.attack) {
            m.queued_until = now + BUFFER;
        }
        let queued = now < m.queued_until;
        let grounded = motor.grounded;
        let guard_ok = intent.guard && grounded && !motor.wall && secs("guard_idle").is_some();
        // Air butterfly: the guard also cancels an air slash in the air; that guard stays up
        // (and carries on as a normal one after landing) while the button is held.
        let guard_clip = intent.guard && !motor.wall && secs("guard_idle").is_some();
        let air_bf = guard_clip && !grounded;
        let guard_hold = guard_clip && (grounded || m.air_guard);

        // The actor controller ended the clip early: a jump, dash or wall kick cancelled it.
        if m.phase != Phase::Idle && acting.is_none() && t > 0.1 && t < m.secs - 0.05 {
            debug!(
                "t={now:.2} melee {who}: {:?} K-style cancel (tumble/jump took over) at {t:.2}s",
                m.phase
            );
            m.phase = Phase::Idle;
            m.combo_until = now + CANCEL_COMBO;
        }

        let charge_ready = !bot
            && intent.attack
            && grounded
            && now - m.down_at >= CHARGE_HOLD
            && now >= m.ready
            && !queued
            && secs("charge").is_some();
        let blow_from = |m: &Melee, power: f32| plan(&data, woman, motion, m, now, motor, power);
        let free = motor.tumble.is_none_or(|t| t > 0.1);

        let next = match m.phase {
            Phase::Idle if !free => None,
            Phase::Idle => {
                if guard_ok {
                    Some(Next::Guard(Stage::Start))
                } else if queued && now >= m.ready {
                    blow_from(m, 0.0)
                } else if charge_ready {
                    Some(Next::Charge)
                } else {
                    None
                }
            }
            Phase::Blow {
                blow,
                strike,
                ret,
                dive,
            } => {
                if !m.struck && t >= strike {
                    m.struck = true;
                    strikes.write(Strike {
                        attacker: e,
                        item,
                        blow,
                        power: m.power,
                    });
                    debug!(
                        "t={now:.2} melee {who}: {} strikes at {t:.3}s (hit frame {:.1})",
                        blow.name,
                        strike * FPS
                    );
                }
                if !m.struck {
                    None
                } else if guard_ok || air_bf && (blow == AIR || blow == DIVE) {
                    // butterfly: the guard cancels the recovery
                    Some(Next::Guard(Stage::Start))
                } else if queued && now >= m.ready && !dive {
                    blow_from(m, 0.0)
                } else if dive && grounded && secs("jump_slash2").is_some() {
                    m.combo_until = now;
                    Some(Next::Blow {
                        blow: SLAM,
                        clip: "jump_slash2",
                        ret: None,
                        dive: false,
                        combo: 0,
                        power: 0.0,
                    })
                } else if t >= m.secs && !(dive && !grounded) {
                    // (no recovery clip over a dash or jump that already took the body)
                    Some(
                        match ret
                            .filter(|r| motor.tumble.is_none() && !motor.wall && secs(r).is_some())
                        {
                            Some(clip) => Next::Recover {
                                clip,
                                act_from: 0.0,
                            },
                            None => Next::Idle,
                        },
                    )
                } else {
                    None
                }
            }
            Phase::Recover { act_from } => {
                if t >= act_from && guard_ok {
                    Some(Next::Guard(Stage::Start))
                } else if t >= act_from && queued && now >= m.ready {
                    blow_from(m, 0.0)
                } else if t >= act_from && charge_ready {
                    Some(Next::Charge)
                } else if t >= m.secs {
                    Some(Next::Idle)
                } else {
                    None
                }
            }
            Phase::Charge => {
                if !m.charged && t >= CHARGE_FULL {
                    m.charged = true;
                    sound.write(ActorSound {
                        actor: e,
                        cue: Cue::Anim("fx_chargecomplete".into()),
                    });
                }
                if !intent.attack || t >= m.secs {
                    if t >= CHARGE_MIN {
                        let power = ((t - CHARGE_MIN) / (CHARGE_FULL - CHARGE_MIN)).clamp(0.0, 1.0);
                        secs("slash").map(|_| Next::Blow {
                            blow: MASSIVE,
                            clip: "slash",
                            ret: None,
                            dive: false,
                            combo: 0,
                            power,
                        })
                    } else {
                        Some(Next::Recover {
                            clip: "cancel",
                            act_from: 0.15,
                        })
                    }
                } else {
                    None
                }
            }
            Phase::Guard(stage) => {
                if stage == Stage::Release {
                    if queued && now >= m.ready {
                        blow_from(m, 0.0)
                    } else if t >= m.secs {
                        Some(Next::Idle)
                    } else {
                        None
                    }
                } else if !guard_hold {
                    Some(Next::Guard(Stage::Release))
                } else if queued && now >= m.ready && !grounded {
                    // click while the guard is up in the air: another air slash
                    blow_from(m, 0.0)
                } else if queued && now >= m.ready {
                    // guard + click: uppercut
                    secs("uppercut").map(|_| Next::Blow {
                        blow: UPPERCUT,
                        clip: "uppercut",
                        ret: None,
                        dive: false,
                        combo: 0,
                        power: 0.0,
                    })
                } else if stage == Stage::Block
                    && t >= m.secs
                    && m.alt
                    && secs("guard_block1_ret").is_some()
                {
                    // `alt` is set right after a block1 pose (see `resolve`)
                    Some(Next::Guard(Stage::BlockRet))
                } else if stage != Stage::Hold && t >= m.secs {
                    Some(Next::Guard(Stage::Hold))
                } else {
                    None
                }
            }
            Phase::Hurt { stand } => (t >= m.secs).then(|| {
                if stand {
                    Next::Recover {
                        clip: "blast_stand",
                        act_from: f32::INFINITY,
                    }
                } else {
                    Next::Idle
                }
            }),
        };

        let Some(next) = next else {
            continue;
        };
        let go = |m: &mut Melee,
                  req: &mut MessageWriter<ActionRequest>,
                  clip: &'static str,
                  secs: f32,
                  moving: ActionMove,
                  cancel_from: f32| {
            m.t0 = now;
            m.secs = secs;
            req.write(ActionRequest {
                actor: e,
                clip,
                speed: 1.0,
                moving,
                cancel_from,
            });
        };
        match next {
            Next::Blow {
                blow,
                clip,
                ret,
                dive,
                combo,
                power,
            } => {
                let Some(len) = secs(clip) else {
                    m.phase = Phase::Idle;
                    continue;
                };
                let strike = strike_secs(motion, clip, len);
                let dash = motor.tumble.is_some();
                debug!(
                    "t={now:.2} melee {who}: {} ({clip}, {len:.2}s, strike {strike:.2}s){}",
                    blow.name,
                    if dash { " from a dash" } else { "" }
                );
                m.phase = Phase::Blow {
                    blow,
                    strike,
                    ret,
                    dive,
                };
                m.struck = false;
                m.power = power;
                m.combo = combo;
                m.combo_until = f32::INFINITY;
                m.queued_until = 0.0;
                m.ready = now + w.delay as f32 / 1000.0;
                // K-style: a jump or dash may cut the recovery once the blade has landed.
                let moving = if dive || clip == "attack_Jump" {
                    ActionMove::Control(1.0)
                } else {
                    ActionMove::Root
                };
                go(m, &mut req, clip, len, moving, strike);
                let origin = tf.translation + Vec3::Y;
                let dir = Quat::from_rotation_y(intent.yaw) * Vec3::NEG_Z;
                fire.write(Fire {
                    shooter: e,
                    item,
                    origin,
                    dir,
                });
                if blow.effect == Effect::Launch {
                    sound.write(ActorSound {
                        actor: e,
                        cue: Cue::Anim("uppercut".into()),
                    });
                }
            }
            Next::Guard(stage) => {
                let clip = match stage {
                    Stage::Start => "guard_start",
                    Stage::Hold => "guard_idle",
                    Stage::Block => "guard_block1",
                    Stage::BlockRet => "guard_block1_ret",
                    Stage::Release => "guard_cancel",
                };
                // Some motion types have no `guard_start`: raise the guard straight away.
                let (stage, clip, len) = match secs(clip) {
                    Some(len) => (stage, clip, len),
                    None if stage == Stage::Start => (Stage::Hold, "guard_idle", f32::INFINITY),
                    None => {
                        m.phase = Phase::Idle;
                        continue;
                    }
                };
                if stage == Stage::Start {
                    m.slide = matches!(m.phase, Phase::Blow { .. } | Phase::Recover { .. });
                    m.air_guard = !grounded;
                    if m.slide {
                        let what = match m.phase {
                            Phase::Blow { .. } if !grounded => "air butterfly",
                            Phase::Blow { blow, .. } if blow.effect == Effect::Launch => {
                                "quick launch"
                            }
                            Phase::Blow { .. } => "butterfly",
                            _ => "slide",
                        };
                        debug!(
                            "t={now:.2} melee {who}: tech: {what} (guard cancels the recovery, may move)"
                        );
                    }
                } else if stage == Stage::Release {
                    m.slide = false;
                }
                debug!("t={now:.2} melee {who}: guard {stage:?} ({clip})");
                let len = if clip == "guard_idle" {
                    f32::INFINITY
                } else {
                    len
                };
                m.phase = Phase::Guard(stage);
                if stage == Stage::Release {
                    m.combo_until = now + CANCEL_COMBO;
                }
                let moving = if m.slide {
                    ActionMove::Control(1.0)
                } else {
                    ActionMove::Locked
                };
                go(m, &mut req, clip, len, moving, 0.0);
            }
            Next::Charge => {
                let Some(len) = secs("charge") else {
                    continue;
                };
                debug!("t={now:.2} melee {who}: charge ({len:.2}s)");
                m.phase = Phase::Charge;
                m.charged = false;
                go(m, &mut req, "charge", len, ActionMove::Locked, 0.0);
            }
            Next::Recover { clip, act_from } => match secs(clip) {
                Some(len) => {
                    debug!("t={now:.2} melee {who}: recover ({clip}, {len:.2}s)");
                    m.phase = Phase::Recover { act_from };
                    go(m, &mut req, clip, len, ActionMove::Locked, act_from);
                }
                None => {
                    m.phase = Phase::Idle;
                    m.combo_until = now;
                }
            },
            Next::Idle => {
                debug!("t={now:.2} melee {who}: idle");
                if !matches!(m.phase, Phase::Guard(_)) {
                    m.combo_until = now;
                }
                m.phase = Phase::Idle;
            }
        }
    }
}

/// `Guarding` is on the actor exactly while its guard is up.
fn sync_guard(commands: &mut Commands, e: Entity, phase: Phase, has: bool) {
    let up = matches!(phase, Phase::Guard(s) if s != Stage::Release);
    if up && !has {
        commands.entity(e).insert(Guarding);
    } else if !up && has {
        commands.entity(e).remove::<Guarding>();
    }
}

/// The blow a click starts: a combo slash on the ground (`attack1..4`, the single `attackS` of
/// daggers), `attack_Jump` rising / `jump_slash1` falling in the air.
fn plan(
    data: &ActorData,
    woman: bool,
    motion: u32,
    m: &Melee,
    now: f32,
    motor: &Motor,
    power: f32,
) -> Option<Next> {
    let has = |c: &str| data.clip_secs(woman, motion, c).is_some();
    if !motor.grounded {
        return if motor.vel.y > 0.0 && has("attack_Jump") {
            Some(Next::Blow {
                blow: AIR,
                clip: "attack_Jump",
                ret: None,
                dive: false,
                combo: m.combo,
                power,
            })
        } else if has("jump_slash1") {
            Some(Next::Blow {
                blow: DIVE,
                clip: "jump_slash1",
                ret: None,
                dive: true,
                combo: m.combo,
                power,
            })
        } else if has("attack_Jump") {
            Some(Next::Blow {
                blow: AIR,
                clip: "attack_Jump",
                ret: None,
                dive: false,
                combo: m.combo,
                power,
            })
        } else {
            None
        };
    }
    let n = ATTACKS.iter().take_while(|c| has(c)).count();
    if n == 0 {
        return has("attackS").then_some(Next::Blow {
            blow: SLASH,
            clip: "attackS",
            ret: None,
            dive: false,
            combo: 0,
            power,
        });
    }
    let i = if now < m.combo_until && m.combo < n {
        m.combo
    } else {
        0
    };
    Some(Next::Blow {
        blow: if n > 1 && i == n - 1 { FINISHER } else { SLASH },
        clip: ATTACKS[i],
        ret: Some(RETURNS[i]),
        dive: false,
        combo: (i + 1) % n,
        power,
    })
}

#[allow(clippy::too_many_arguments)]
fn resolve(
    mut strikes: MessageReader<Strike>,
    time: Res<Time>,
    data: Res<ActorData>,
    col: Res<MapCollision>,
    mut commands: Commands,
    mut damage: MessageWriter<Damage>,
    mut blocked: MessageWriter<Blocked>,
    mut impact: MessageWriter<Impact>,
    mut req: MessageWriter<ActionRequest>,
    mut vfx: MessageWriter<Vfx>,
    mut melee: Query<&mut Melee>,
    actors: Query<
        (
            Entity,
            &GlobalTransform,
            Option<&Intent>,
            Option<&Motor>,
            Option<&Team>,
            Has<Bot>,
            Has<Guarding>,
            Has<Protected>,
            Option<&Loadout>,
            Option<&Name>,
        ),
        (With<Vitals>, Without<Dead>),
    >,
    mut seed: Local<u32>,
) {
    if *seed == 0 {
        *seed = 0x9E37_79B9;
    }
    let now = time.elapsed_secs();
    for s in strikes.read() {
        let Some(w) = data.items.get(s.item).and_then(|i| i.weapon.as_ref()) else {
            continue;
        };
        let Ok((_, g, Some(intent), _, team, bot, _, _, _, aname)) = actors.get(s.attacker) else {
            continue;
        };
        let me = (team.copied(), bot);
        let centre = g.translation() + Vec3::Y;
        let fwd = Quat::from_rotation_y(intent.yaw) * Vec3::NEG_Z;
        let range = w.range.unwrap_or(150) as f32 * 0.01 * s.blow.reach;
        let half = w.angle.map_or(s.blow.arc, |a| a as f32).to_radians() * 0.5;
        let amount = w.damage as f32
            * s.blow.damage
            * if s.blow.effect == Effect::Massive {
                1.0 + MASSIVE_BONUS * s.power
            } else {
                1.0
            };
        let mut landed = 0;
        for (e, tg, vi, vm, vteam, vbot, guarding, protected, vload, vname) in &actors {
            if e == s.attacker || protected || friendly(me, (vteam.copied(), vbot)) {
                continue;
            }
            let p = tg.translation() + Vec3::Y * 0.9;
            let to = Vec3::new(p.x - centre.x, 0.0, p.z - centre.z);
            let dist = to.length();
            debug!(
                "t={now:.2} melee: candidate {}: {dist:.2} m (reach {:.2}), {:.0} deg off (half {:.0}), dy {:.2}, protected {protected}",
                vname.map_or("?", |n| n.as_str()),
                range + HIT_RADIUS,
                fwd.angle_between(to).to_degrees(),
                half.to_degrees(),
                p.y - centre.y,
            );
            if dist - HIT_RADIUS > range || (p.y - centre.y).abs() > REACH_Y {
                continue;
            }
            if dist > HIT_RADIUS && fwd.angle_between(to) > half {
                continue;
            }
            let v = p - centre;
            let d = v.normalize_or(fwd);
            if col
                .raycast(centre, d, v.length())
                .is_some_and(|h| h.distance < v.length() - HIT_RADIUS)
            {
                continue;
            }
            let flat = to.normalize_or(fwd);
            let label = vname.map_or("?", |n| n.as_str());
            let vmotion = vload
                .and_then(|l| l.slots.get(l.current))
                .and_then(|s| data.items.get(s.item))
                .and_then(|i| i.weapon.as_ref())
                .map_or(1, |w| w.kind.motion_type());
            // Quest monsters have no melee state: they take the damage and nothing else.
            let mut state = melee.get_mut(e).ok();
            let mut body = vi.zip(vm).zip(state.as_deref_mut());

            // Guard: blocks every blow from the front except a massive swing.
            if let Some(((vi, vm), vstate)) = body.as_mut()
                && guarding
                && s.blow.effect != Effect::Massive
                && (Quat::from_rotation_y(vi.yaw) * Vec3::NEG_Z).angle_between(-flat)
                    <= GUARD_HALF.to_radians()
            {
                let clip = if vstate.alt {
                    "guard_block2"
                } else {
                    "guard_block1"
                };
                vstate.alt = !vstate.alt;
                if let Some(len) = data.clip_secs(vm.woman, vmotion, clip) {
                    vstate.phase = Phase::Guard(Stage::Block);
                    vstate.t0 = now;
                    vstate.secs = len;
                    req.write(ActionRequest {
                        actor: e,
                        clip,
                        speed: 1.0,
                        moving: ActionMove::Locked,
                        cancel_from: 0.0,
                    });
                }
                blocked.write(Blocked(e));
                vfx.write(Vfx::Elu {
                    name: "ef_sword_flash",
                    at: Transform::from_translation(p),
                });
                debug!(
                    "t={now:.2} melee: {} blocked by {label} ({clip})",
                    aname.map_or("?", |n| n.as_str())
                );
                continue;
            }

            landed += 1;
            damage.write(Damage {
                target: e,
                attacker: s.attacker,
                amount,
                item: s.item,
                point: p,
                dir: d,
                pierce: None,
            });
            vfx.write(Vfx::Blood {
                point: p,
                dir: d,
                amount,
            });
            vfx.write(Vfx::Elu {
                name: ["sword_damage1", "sword_damage2", "sword_damage3"]
                    [(rnd(&mut seed) * 3.0) as usize % 3],
                at: Transform::from_translation(p),
            });

            // Reaction. An actor already thrown up is juggled instead of flinching; a launch
            // lifts an airborne one with full force (K-style juggle). A lying one is left alone.
            let reaction = match body.as_mut() {
                None => None,
                Some(((_, vm), _)) if vm.blast && vm.grounded => None,
                Some(((_, vm), vstate)) if vm.blast => {
                    if vstate.juggle < MAX_JUGGLE {
                        vstate.juggle += 1;
                        debug!(
                            "t={now:.2} melee: tech: juggle {label} x{} ({})",
                            vstate.juggle, s.blow.name
                        );
                        let up = if s.blow.effect == Effect::Launch {
                            UPPERCUT_UP
                        } else {
                            JUGGLE_UP
                        };
                        Some(Vec3::Y * up + flat * FLINCH_PUSH)
                    } else {
                        None
                    }
                }
                Some(((_, vm), vstate)) => match s.blow.effect {
                    Effect::Flinch | Effect::Knockdown => {
                        let (clip, stand, push) = if s.blow.effect == Effect::Knockdown {
                            ("damage_down", true, KNOCKDOWN_PUSH)
                        } else {
                            let c = if vstate.alt { "damage2" } else { "damage" };
                            vstate.alt = !vstate.alt;
                            (c, false, FLINCH_PUSH)
                        };
                        if let Some(len) = data.clip_secs(vm.woman, vmotion, clip) {
                            vstate.phase = Phase::Hurt { stand };
                            vstate.t0 = now;
                            vstate.secs = len;
                            vstate.queued_until = 0.0;
                            req.write(ActionRequest {
                                actor: e,
                                clip,
                                speed: 1.0,
                                moving: ActionMove::Locked,
                                cancel_from: f32::INFINITY,
                            });
                        }
                        Some(flat * push)
                    }
                    Effect::Launch => {
                        vstate.phase = Phase::Idle;
                        vstate.juggle = 1;
                        Some(Vec3::Y * UPPERCUT_UP + flat * UPPERCUT_BACK)
                    }
                    Effect::Massive => {
                        vstate.phase = Phase::Idle;
                        Some(Vec3::Y * MASSIVE_UP + flat * MASSIVE_BACK)
                    }
                },
            };
            if let Some(push) = reaction {
                commands.entity(e).insert(Push(push));
            }
            debug!(
                "t={now:.2} melee: {} {} hits {label} for {amount:.0} ({:?})",
                aname.map_or("?", |n| n.as_str()),
                s.blow.name,
                reaction
            );
        }
        if landed == 0
            && let Some(h) = col.raycast(centre, fwd, WALL_SPARK.min(range))
        {
            impact.write(Impact {
                point: h.point,
                normal: h.normal,
                blade: true,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strike_frames() {
        // katana attack1 lands at frame 5 of 9 (0.5 s into a 0.3 s clip is clamped to its end).
        assert!((strike_secs(1, "attack1", 0.3) - 5.0 / FPS).abs() < 1e-6);
        // unknown clips: 45 % of the clip.
        assert!((strike_secs(1, "damage", 1.0) - 0.45).abs() < 1e-6);
        // never later than the clip itself
        assert_eq!(strike_secs(1, "attack1", 0.1), 0.1);
    }
}
