//! Melee combat: the sword state machine (slash combo, jump/dive slash, uppercut, charged
//! massive, guard) and its hit resolution. Per actor a [`Melee`] component decides which
//! retail clip plays when (`ActionRequest` to the actor controller) and when the blade lands
//! (`Strike`); [`resolve`] turns a strike into `Damage`, `Blocked`, knockback `Push` and the
//! victim's flinch. Constants marked *inferred* are not in the retail data (the executable is
//! packed): docs/formats.md "Melee".
//!
//! Controls (the same `Intent` bots write): a click (`attack` pressed) slashes, `attack`
//! held from the click charges; `guard` held raises the guard; `attack` while guarding is the
//! uppercut. Bots hold `attack` to chain slashes. A dagger has no guard: its `guard` press is the
//! alternative fire, a lunge on the ground (its `uppercut` clip launches); its click stabs
//! (`attackS`) on the ground and in the air.

use crate::{
    actor::{Actor, ActorData, ActorSet},
    ani::FPS,
    col::MapCollision,
    combat::{SWITCH_DELAY, Vfx, is_melee, rnd, shape},
    game::*,
    item::WeaponKind,
    phase::MatchSystems,
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
const UPPERCUT_BACK: f32 = 3.0;
const MASSIVE_UP: f32 = 5.5;
const MASSIVE_BACK: f32 = 5.0;
/// Air juggle: a hit on an actor already in the blast chain lifts it again, at most this often.
const JUGGLE_UP: f32 = 5.5;
const MAX_JUGGLE: u8 = 3;
/// Seconds after a switch-cancelled air slash during which switching back to the blade needs no
/// draw delay (flash step / quick slash; *inferred*).
const FLASH_WINDOW: f32 = 1.5;
/// Block cancel (the quadruple butterfly's glitch): a guard this many seconds or more into a blow
/// lands the blow at once and cuts the rest of its swing (*inferred*).
const BLOCK_CANCEL: f32 = 0.05;
/// A slash may start this many seconds into a dash (*inferred*).
const DASH_SLASH: f32 = 0.05;

pub struct MeleePlugin;

impl Plugin for MeleePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Strike>().add_systems(
            Update,
            (init, drive, resolve)
                .chain()
                .after(ActorSet::Input)
                .before(ActorSet::Drive)
                .in_set(MatchSystems),
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
/// The dagger's alternative fire (guard key, on the ground): its `uppercut` clip is a lunge that
/// launches (`blast_dagger`); *inferred*.
const LUNGE: Blow = Blow {
    name: "lunge",
    arc: 70.0,
    effect: Effect::Launch,
    ..SLASH
};
/// The extended lunge (a lunge that starts in a dash, or on the press that starts one): twice the
/// reach of the plain one (the guides' "about 2x", *inferred*).
const E_LUNGE: Blow = Blow {
    name: "extended lunge",
    reach: 2.0,
    ..LUNGE
};
/// A blocked blow recoils a dagger stabber (its `damage` clip) unless it looks up or down at
/// least this steeply, radians (block tap; *inferred*).
const TAP_PITCH: f32 = 1.2;

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
    /// The guard went up in the air (air butterfly): it may stay up there until the actor lands.
    air_guard: bool,
    /// A dagger's guard press (alternative fire) is pending until this time.
    lunge_until: f32,
    /// Air hits taken since landing.
    juggle: u8,
    /// Item last seen in hand (weapon switches restart the state).
    item: u32,
    /// When the guard was last pressed and whether it was held last frame (a block cancel needs a
    /// press during the blow; a guard held into an uppercut is not one).
    guard_at: f32,
    prev_guard: bool,
}

impl Melee {
    /// A blow's clip is playing: the blade is swinging ([`crate::slashfx`] trails it).
    pub fn swinging(&self) -> bool {
        matches!(self.phase, Phase::Blow { .. })
    }
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
        &Actor,
        Option<&Name>,
    )>,
) {
    let (now, dt) = (time.elapsed_secs(), time.delta_secs().min(0.05));
    for (e, mut m, intent, load, motor, tf, acting, dead, bot, has_guard, actor, name) in
        &mut actors
    {
        let m = &mut *m;
        let who = name.map_or("?", |n| n.as_str());
        let click = intent.attack && !m.prev_attack;
        m.prev_attack = intent.attack;
        if click {
            m.down_at = now;
        }
        let guard_press = intent.guard && !m.prev_guard;
        if guard_press {
            m.guard_at = now;
        }
        m.prev_guard = intent.guard;
        if !motor.blast && motor.grounded {
            m.juggle = 0;
        }
        let item = load.slots.get(load.current).map_or(0, |s| s.item);
        let weapon = data.items.weapon(item);
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
                m.ready = if air_cancel || flash || actor.quick_draw(now) {
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
        if guard_press && w.kind == WeaponKind::Dagger {
            m.lunge_until = now + BUFFER;
        }
        let grounded = motor.grounded;
        let guard_ok = intent.guard && grounded && !motor.wall && secs("guard_idle").is_some();
        // Air butterfly: the guard also cancels an air slash in the air; that guard stays up
        // (and carries on as a normal one after landing) while the button is held.
        let guard_clip = intent.guard && !motor.wall && secs("guard_idle").is_some();
        let air_bf = guard_clip && !grounded;
        let guard_hold = guard_clip && (grounded || m.air_guard);

        // The actor controller ended the clip early: a jump, dash or wall kick cancelled it (the
        // `Acting` of a clip requested last frame is there from this frame on; a dash shows at once).
        if m.phase != Phase::Idle
            && acting.is_none()
            && t > 0.0
            && (t < m.secs - 0.05 || motor.tumble.is_some())
        {
            debug!(
                "t={now:.2} melee {who}: {:?} K-style cancel (tumble/jump took over) at {t:.2}s",
                m.phase
            );
            // the jump or dash came on the hit frame itself: the blade still lands
            if let Phase::Blow { blow, strike, .. } = m.phase
                && !m.struck
                && t >= strike - 0.05
            {
                strikes.write(Strike {
                    attacker: e,
                    item,
                    blow,
                    power: m.power,
                });
            }
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
        let free = motor.tumble.is_none_or(|t| t > DASH_SLASH);
        // the dagger's alternative fire on the ground: the `uppercut` clip as a lunge, the extended
        // one when a dash is on (`Actor::dash_wanted`: the same press) or just began
        let extended = motor.tumble.is_some() || actor.dash_wanted(now, intent).is_some();
        let lunge =
            (now < m.lunge_until && grounded && secs("uppercut").is_some()).then_some(Next::Blow {
                blow: if extended { E_LUNGE } else { LUNGE },
                clip: "uppercut",
                ret: None,
                dive: false,
                combo: 0,
                power: 0.0,
            });

        let next = match m.phase {
            Phase::Idle if !free => None,
            Phase::Idle => {
                if guard_ok {
                    Some(Next::Guard(Stage::Start))
                } else if lunge.is_some() && now >= m.ready {
                    lunge
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
                let cancel_guard = guard_ok || air_bf && (blow == AIR || blow == DIVE);
                // block cancel: slashes only (not the uppercut, the slam or a charged swing)
                let block_cancel = cancel_guard
                    && [SLASH, FINISHER, AIR, DIVE].contains(&blow)
                    && m.guard_at >= m.t0
                    && t >= BLOCK_CANCEL;
                if !m.struck && (t >= strike || block_cancel) {
                    if t < strike {
                        debug!(
                            "t={now:.2} melee {who}: tech: block cancel (the guard lands {} at {t:.2}s and cuts its swing)",
                            blow.name
                        );
                    }
                    m.struck = true;
                    strikes.write(Strike {
                        attacker: e,
                        item,
                        blow,
                        power: m.power,
                    });
                    debug!(
                        "t={now:.2} melee {who}: {} strikes at {t:.3}s (hit frame {:.1}) at ({:.2}, {:.2})",
                        blow.name,
                        strike * FPS,
                        tf.translation.x,
                        tf.translation.z
                    );
                }
                if !m.struck {
                    None
                } else if cancel_guard {
                    // butterfly: the guard cancels the recovery
                    Some(Next::Guard(Stage::Start))
                } else if lunge.is_some() && now >= m.ready {
                    lunge
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
                let over = clip_over(t, dt, m.secs, acting.is_none());
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
                    && over
                    && m.alt
                    && secs("guard_block1_ret").is_some()
                {
                    // `alt` is set right after a block1 pose (see `resolve`)
                    Some(Next::Guard(Stage::BlockRet))
                } else if stage != Stage::Hold && over {
                    Some(Next::Guard(Stage::Hold))
                } else {
                    None
                }
            }
            Phase::Hurt { stand } => (t >= m.secs).then_some({
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
                    "t={now:.2} melee {who}: {} ({clip}, {len:.2}s, strike {strike:.2}s){} at ({:.2}, {:.2})",
                    blow.name,
                    if dash { " from a dash" } else { "" },
                    tf.translation.x,
                    tf.translation.z
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
                m.lunge_until = 0.0;
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
                    m.air_guard = !grounded;
                    let what = match m.phase {
                        Phase::Blow { .. } if !grounded => Some("air butterfly"),
                        Phase::Blow { blow, .. } if blow.effect == Effect::Launch => {
                            Some("quick launch")
                        }
                        Phase::Blow { .. } => Some("butterfly"),
                        Phase::Recover { .. } => Some("recovery cancel"),
                        _ => None,
                    };
                    if let Some(what) = what {
                        debug!("t={now:.2} melee {who}: tech: {what} (guard cancels the recovery)");
                    }
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
                go(m, &mut req, clip, len, ActionMove::Locked, 0.0);
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
                debug!(
                    "t={now:.2} melee {who}: idle at ({:.2}, {:.2})",
                    tf.translation.x, tf.translation.z
                );
                if !matches!(m.phase, Phase::Guard(_)) {
                    m.combo_until = now;
                }
                m.phase = Phase::Idle;
            }
        }
    }
}

/// Whether a clip of `secs` that began `t` seconds ago is over by the actor controller's turn.
/// That runs after this system, and its own action clock is one frame ahead of ours: a clip that
/// ends in it must already have its successor requested now, or the body runs free for a frame
/// between `guard_start` and `guard_idle` (a millisecond of slack for the two clocks' rounding).
/// An action that is already gone (`gone`) is over whatever the clock says.
fn clip_over(t: f32, dt: f32, secs: f32, gone: bool) -> bool {
    gone || t + dt + 0.001 >= secs
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
        // a dagger stabs in the air as on the ground (no `attack_Jump`, only `attackS`)
        if !has("attack_Jump") && has("attackS") {
            return Some(Next::Blow {
                blow: SLASH,
                clip: "attackS",
                ret: None,
                dive: false,
                combo: 0,
                power,
            });
        }
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
    mut hits: MessageWriter<crate::slashfx::Hit>,
    mut melee: Query<&mut Melee>,
    actors: Query<
        (
            Entity,
            &GlobalTransform,
            Option<&Intent>,
            Option<&Motor>,
            Option<&Team>,
            Has<Guarding>,
            Has<Protected>,
            Option<&Loadout>,
            Option<&HitShape>,
            Has<Npc>,
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
        let Some(w) = data.items.weapon(s.item) else {
            continue;
        };
        let Ok((_, g, Some(intent), am, team, _, _, _, _, _, aname)) = actors.get(s.attacker)
        else {
            continue;
        };
        let me = team.copied();
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
        let mut recoil = false;
        for (e, tg, vi, vm, vteam, guarding, protected, vload, vshape, npc, vname) in &actors {
            if e == s.attacker || protected || friendly(me, vteam.copied()) {
                continue;
            }
            // phantom: a tumbling or lunging actor has a smaller capsule (`actor.rs`)
            let (vr, vh) = shape(vshape.filter(|_| !npc));
            let p = tg.translation() + Vec3::Y * vh * 0.5;
            let to = Vec3::new(p.x - centre.x, 0.0, p.z - centre.z);
            let dist = to.length();
            debug!(
                "t={now:.2} melee: candidate {}: {dist:.2} m (reach {:.2}), {:.0} deg off (half {:.0}), dy {:.2}, protected {protected}",
                vname.map_or("?", |n| n.as_str()),
                range + vr,
                fwd.angle_between(to).to_degrees(),
                half.to_degrees(),
                p.y - centre.y,
            );
            if dist - vr > range || (p.y - centre.y).abs() > REACH_Y {
                continue;
            }
            if dist > vr && fwd.angle_between(to) > half {
                continue;
            }
            let v = p - centre;
            let d = v.normalize_or(fwd);
            if col
                .raycast(centre, d, v.length())
                .is_some_and(|h| h.distance < v.length() - vr)
            {
                continue;
            }
            let flat = to.normalize_or(fwd);
            let label = vname.map_or("?", |n| n.as_str());
            let vmotion = vload
                .and_then(|l| l.slots.get(l.current))
                .and_then(|s| data.items.weapon(s.item))
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
                recoil = true;
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
            hits.write(crate::slashfx::Hit {
                attacker: s.attacker,
                point: p,
                dir: d,
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
                        if w.kind == WeaponKind::Dagger
                            && matches!(vstate.phase, Phase::Hurt { .. })
                        {
                            debug!(
                                "t={now:.2} melee: tech: dagger machine ({label} is stabbed again inside its flinch)"
                            );
                        }
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
        if recoil && w.kind == WeaponKind::Dagger {
            let who = aname.map_or("?", |n| n.as_str());
            if intent.pitch.abs() >= TAP_PITCH {
                debug!("t={now:.2} melee: tech: block tap {who} (looking up or down, no recoil)");
            } else if let Ok(mut st) = melee.get_mut(s.attacker)
                && let Some(len) = am.and_then(|m| data.clip_secs(m.woman, 7, "damage"))
            {
                debug!("t={now:.2} melee: {who} recoils off the block");
                (st.phase, st.t0, st.secs, st.queued_until) =
                    (Phase::Hurt { stand: false }, now, len, 0.0);
                req.write(ActionRequest {
                    actor: s.attacker,
                    clip: "damage",
                    speed: 1.0,
                    moving: ActionMove::Locked,
                    cancel_from: f32::INFINITY,
                });
                commands.entity(s.attacker).insert(Push(-fwd * FLINCH_PUSH));
            }
        }
        if landed == 0
            && let Some(h) = col.raycast(centre, fwd, WALL_SPARK.min(range))
        {
            impact.write(Impact {
                point: h.point,
                normal: h.normal,
                blade: true,
                item: s.item,
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

    #[test]
    fn guard_clip_chains_a_frame_early() {
        // `guard_start` (0.10 s) at 60 fps: the actor controller ends it in the frame where this
        // clock reads 5/60 s (the guard glitch: the body ran free for that frame).
        let dt = 1.0 / 60.0;
        assert!(!clip_over(4.0 * dt, dt, 0.1, false));
        assert!(clip_over(5.0 * dt, dt, 0.1, false));
        // a vanished action ends it at once; a looping clip (`guard_idle`) never ends
        assert!(clip_over(0.01, dt, 0.1, true));
        assert!(!clip_over(100.0, dt, f32::INFINITY, false));
    }
}
