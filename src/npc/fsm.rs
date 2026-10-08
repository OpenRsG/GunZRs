//! The `aifsm.xml` interpreter's decision step: which transition of a state fires, given what the
//! actor senses. Pure; `npc.rs` fills [`Sense`] from the world and acts on the answer.
//! Condition semantics that the file does not spell out are **inferred**: `docs/formats.md`.

use super::data::{Cond, Fsm, Next};

/// What the actor knows this tick. Lengths in cm, angles in degrees.
pub struct Sense<'a> {
    pub elapsed_ms: f32,
    pub hp: f32,
    pub groggy: f32,
    pub has_target: bool,
    pub dist: f32,
    pub sees: bool,
    /// Angle between the facing and the direction to the target.
    pub look: f32,
    /// Elevation of the target above the horizon.
    pub elevation: f32,
    /// Target height above the actor's feet.
    pub height: f32,
    pub end_action: bool,
    pub path_failed: bool,
    pub summons: u32,
    /// Free floor `dist` cm away in the direction `angle` (degrees clockwise from facing).
    pub empty: &'a dyn Fn(f32, f32) -> bool,
    /// The state's stored random number in `[0, 1)` (see [`next`]).
    pub dice: f32,
}

pub fn holds(c: &Cond, s: &Sense) -> bool {
    match *c {
        Cond::Groggy(n) => s.groggy > n,
        Cond::Hp(n) => (s.hp - n).abs() < 0.5,
        // Banded by `next`, which never calls this for a `dice` term.
        Cond::Dice(_) => false,
        Cond::Elapsed(ms) => s.elapsed_ms >= ms,
        Cond::EndAction => s.end_action,
        Cond::Dist(a, b) => s.has_target && (a..=b).contains(&s.dist),
        Cond::CanSee => s.has_target && s.sees,
        Cond::CannotSee => s.has_target && !s.sees,
        Cond::HasTarget => s.has_target,
        Cond::NoTarget => !s.has_target,
        Cond::Default => true,
        Cond::FailedPath => s.path_failed,
        Cond::EmptySpace(a, d) => (s.empty)(a, d),
        Cond::Elevation(a, b) => s.has_target && (a..=b).contains(&s.elevation),
        Cond::LookAt(d) => s.has_target && s.look <= d,
        Cond::SummonLess(n) => s.summons < n,
        Cond::Higher(cm) => s.has_target && s.height > cm,
    }
}

/// The first transition of `state` whose conditions all hold and whose target state is off
/// cooldown (`since_ms(target)`: ms since that state was last entered; **inferred**: a state's
/// `cooltime` is the minimum time between two entries).
///
/// `dice:N` (**inferred**): every state with a `dice` row (104 of 559) also has the `dice` entry
/// or step function, so the state holds one random number `s.dice` in `[0, 1)`; scaled by the
/// sum of the state's `dice` values, each row owns the next `N` of that range, in file order
/// (`waitrandom`: four rows of 25 = four equal chances; `orbit1`: 75 + 75 + 25 + 25 + 25 =
/// "orbit" 2/3 of the time, "shoot" 1/3). Sums run 50 to 1 300, so they are not percentages or
/// permille. A row whose other terms fail still keeps its share.
pub fn next(fsm: &Fsm, state: usize, s: &Sense, since_ms: &dyn Fn(usize) -> f32) -> Option<Next> {
    let dice = |c: &Cond| match c {
        Cond::Dice(n) => Some(*n),
        _ => None,
    };
    let rows = &fsm.states[state].trans;
    let total: f32 = rows
        .iter()
        .flat_map(|t| t.conds.iter().filter_map(dice))
        .sum();
    let roll = s.dice * total;
    let mut before = 0.0;
    rows.iter()
        .find(|t| {
            let lo = before;
            before += t.conds.iter().filter_map(dice).sum::<f32>();
            t.conds.iter().all(|c| match c {
                Cond::Dice(n) => roll >= lo && roll < lo + n,
                c => holds(c, s),
            }) && match t.next {
                Next::State(i) => since_ms(i) >= fsm.states[i].cooltime_ms,
                Next::Die => true,
            }
        })
        .map(|t| t.next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::npc::data::parse_fsms;

    #[test]
    fn transitions_respect_conditions_and_cooldown() {
        let f = &parse_fsms(
            r#"<XML><FSM name="k" entrystate="combat">
              <STATE name="combat" cooltime="0" action="run">
                <TRANS cond="hpEqual:0" next="__die"/>
                <TRANS cond="groggyGreater:30" next="hurt"/>
                <TRANS cond="distTarget:0;200,lookAtTarget:45" next="slash"/>
                <TRANS cond="dice:1000" next="combat"/></STATE>
              <STATE name="slash" cooltime="3000" action="slash"><TRANS cond="endAction" next="combat"/></STATE>
              <STATE name="hurt" cooltime="0"><TRANS cond="default" next="combat"/></STATE>
            </FSM></XML>"#,
        )
        .unwrap()["k"];
        let empty = |_: f32, _: f32| true;
        let mut s = Sense {
            elapsed_ms: 0.0,
            hp: 50.0,
            groggy: 0.0,
            has_target: true,
            dist: 150.0,
            sees: true,
            look: 10.0,
            elevation: 0.0,
            height: 0.0,
            end_action: false,
            path_failed: false,
            summons: 0,
            empty: &empty,
            dice: 0.5,
        };
        let ready = |_: usize| 1e9;
        assert_eq!(next(f, 0, &s, &ready), Some(Next::State(1)));
        // slash was entered 1 s ago: its 3 s cooldown skips it; the lone dice row owns all of
        // its range and fires.
        let recent = |i: usize| if i == 1 { 1000.0 } else { 1e9 };
        assert_eq!(next(f, 0, &s, &recent), Some(Next::State(0)));
        s.groggy = 31.0;
        assert_eq!(next(f, 0, &s, &ready), Some(Next::State(2)));
        s.hp = 0.0;
        assert_eq!(next(f, 0, &s, &ready), Some(Next::Die));
        s.hp = 50.0;
        s.groggy = 0.0;
        s.look = 90.0; // facing away: the slash row fails, the dice row still fires
        assert_eq!(next(f, 0, &s, &ready), Some(Next::State(0)));
    }

    #[test]
    fn dice_rows_split_one_roll() {
        // The disposer's `orbit1`: 75 + 75 -> orbit, then 25 + 25 + 25 -> shoot (225 in all).
        let f = &parse_fsms(
            r#"<XML><FSM name="d" entrystate="o">
              <STATE name="o" cooltime="0">
                <TRANS cond="dice:75" next="o"/><TRANS cond="dice:75" next="o"/>
                <TRANS cond="dice:25,canSeeTarget" next="s"/><TRANS cond="dice:25" next="s"/>
                <TRANS cond="dice:25" next="s"/></STATE>
              <STATE name="s" cooltime="0"/></FSM></XML>"#,
        )
        .unwrap()["d"];
        let empty = |_: f32, _: f32| true;
        let mut s = Sense {
            elapsed_ms: 0.0,
            hp: 50.0,
            groggy: 0.0,
            has_target: true,
            dist: 0.0,
            sees: true,
            look: 0.0,
            elevation: 0.0,
            height: 0.0,
            end_action: false,
            path_failed: false,
            summons: 0,
            empty: &empty,
            dice: 0.0,
        };
        let ready = |_: usize| 1e9;
        // 0.5 * 225 = 112 falls in the second band (orbit); 0.9 * 225 = 202 in the last (shoot).
        s.dice = 0.5;
        assert_eq!(next(f, 0, &s, &ready), Some(Next::State(0)));
        s.dice = 0.9;
        assert_eq!(next(f, 0, &s, &ready), Some(Next::State(1)));
        // Out of sight, the first shoot band (150..175) is void and the roll finds nothing.
        s.dice = 0.7;
        s.sees = false;
        assert_eq!(next(f, 0, &s, &ready), None);
    }
}
