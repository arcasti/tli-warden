//! The inputs of a learned value function: a sparse encoding of any state
//! that has not ended, from what the player can see. A morning (a Patrol with
//! no pending choice and no incident to confirm) uses only the first
//! `MORNING_INPUTS` inputs. It never reads the game's dice, the occupant
//! behind a door beyond what the door reveals, or which incident a marker
//! holds.

use crate::engine::data::{CAGE_FIGHT, Card, Gift, IRON_LAW, OPULENCE, RAINBOW, TYRANNY, Tool, data};
use crate::engine::state::{Pending, Phase, State, ToolReason};

/// The active inputs of one state: (input index, value), indices ascending, no duplicates.
pub type Row = Vec<(u16, f32)>;

/// Cells on the board, `data().cell_count`.
const CELLS: usize = 16;
/// Tiers Normal to Chromatic.
const TIERS: usize = RAINBOW as usize + 1;
/// One input per beast and tier, at `beast.index() * TIERS + tier`.
const KINDS: usize = 4 * TIERS;
/// Objectives in the data's objective list.
const OBJECTIVES: usize = 5;
/// Themes with an input of their own, after the input for no theme.
const THEMES: [u32; 4] = [TYRANNY, OPULENCE, IRON_LAW, CAGE_FIGHT];

// Offsets inside one cell's block.
const C_INMATE: usize = 0;
const C_EMPTY: usize = C_INMATE + KINDS;
const C_SEALED: usize = C_EMPTY + 1;
const C_HIDDEN_TIER: usize = C_SEALED + 1;
const C_HIDDEN_BEAST: usize = C_HIDDEN_TIER + TIERS;
const C_SCRATCH: usize = C_HIDDEN_BEAST + 4;
const C_NEST: usize = C_SCRATCH + 1;
const C_NEST_DUE: usize = C_NEST + 1;
const C_MARKER: usize = C_NEST_DUE + 1;
const C_IVORY: usize = C_MARKER + 1;
const C_WARDEN: usize = C_IVORY + 1;
const C_DESTINATION: usize = C_WARDEN + 1;
const C_LION: usize = C_DESTINATION + 1;
const C_PREY: usize = C_LION + 1;
/// Inputs per cell.
const CELL: usize = C_PREY + 1;

// Offsets of the global block.
const G_DAYS: usize = CELLS * CELL;
const DAYS: usize = 21;
const G_VAULT: usize = G_DAYS + DAYS;
const VAULT: usize = 11;
const G_TOOLS: usize = G_VAULT + VAULT;
const G_REROLLS: usize = G_TOOLS + Tool::ALL.len();
const G_OBJECTIVE: usize = G_REROLLS + 1;
const G_PROGRESS: usize = G_OBJECTIVE + OBJECTIVES;
const G_DONE: usize = G_PROGRESS + 1;
const G_THEME: usize = G_DONE + 1;
const G_BOARD: usize = G_THEME + 1 + THEMES.len();

/// Inputs a morning can use: the cell and global blocks.
pub const MORNING_INPUTS: usize = G_BOARD + KINDS;

/// Incidents in the data's incident list.
const INCIDENTS: usize = 17;
/// Kinds of pending choice: Tool for Start, Tool for Objective, Gift, Interrogation, Chain, Cell, Prey.
const PENDING_KINDS: usize = 7;

// Offsets of the day block, all zero on a morning.
const D_MANAGE: usize = MORNING_INPUTS;
const D_EVENT: usize = D_MANAGE + 1;
const D_INCIDENT: usize = D_EVENT + 1;
const D_CHAIN: usize = D_INCIDENT + INCIDENTS;
const D_USED: usize = D_CHAIN + 1;
const D_PENDING: usize = D_USED + 3;
const D_MORE_PENDING: usize = D_PENDING + PENDING_KINDS;
const D_TOOL_OPTIONS: usize = D_MORE_PENDING + 1;
const D_GIFT_OPTIONS: usize = D_TOOL_OPTIONS + Tool::ALL.len();
const D_GIFT_PICKS: usize = D_GIFT_OPTIONS + Gift::ALL.len();
const D_CARD_OPTIONS: usize = D_GIFT_PICKS + 1;
const D_CELL_OPTIONS: usize = D_CARD_OPTIONS + Card::ALL.len();
const D_EATER: usize = D_CELL_OPTIONS + CELLS;

/// Total input count.
pub const INPUTS: usize = D_EATER + CELLS;

/// Whether `st` is a morning: a Patrol with no pending choice and no incident to confirm.
pub fn is_morning(st: &State) -> bool {
    st.phase == Phase::Patrol && st.pending.is_empty() && st.event.is_none()
}

/// The inputs of `st`, from what the player can see; zero inputs are left out.
pub fn encode(st: &State) -> Row {
    let d = data();
    assert_eq!(d.cell_count, CELLS, "cell count");
    assert_eq!(d.objectives.len(), OBJECTIVES, "objective count");
    assert_eq!(d.incidents.len(), INCIDENTS, "incident count");
    let mut row = Row::new();
    let mut put = |i: usize, v: f32| {
        if v != 0.0 {
            row.push((i as u16, v));
        }
    };
    let mut board = [0u32; KINDS];
    for (n, c) in st.cells.iter().enumerate().take(CELLS) {
        let at = n * CELL;
        if !c.sealed {
            match c.inmate {
                Some(i) => {
                    let kind = i.beast.index() * TIERS + i.tier.clamp(0, RAINBOW) as usize;
                    board[kind] += 1;
                    put(at + C_INMATE + kind, 1.0);
                }
                None => put(at + C_EMPTY, 1.0),
            }
        } else {
            put(at + C_SEALED, 1.0);
            if let Some(h) = c.hidden {
                if c.known >= 1 {
                    put(at + C_HIDDEN_TIER + h.tier.clamp(0, RAINBOW) as usize, 1.0);
                }
                if c.known >= 2 {
                    put(at + C_HIDDEN_BEAST + h.beast.index(), 1.0);
                }
            }
        }
        put(at + C_SCRATCH, f32::from(c.scratch));
        if let Some(nest) = c.nest {
            put(at + C_NEST, 1.0);
            put(at + C_NEST_DUE, (nest.due as f32 - st.day as f32).max(0.0) / 4.0);
        }
        put(at + C_MARKER, f32::from(c.event.is_some()));
        put(at + C_IVORY, f32::from(c.ivory));
        put(at + C_WARDEN, f32::from(st.position == n));
        put(at + C_DESTINATION, f32::from(st.destinations.contains(&n)));
        let id = c.inmate.map(|i| i.id);
        let rounds = |link: &crate::engine::state::PreyLink| (link.rounds as f32 / 3.0).max(1.0 / 3.0);
        if let Some(link) = st.prey.iter().find(|l| Some(l.lion) == id) {
            put(at + C_LION, rounds(link));
        }
        if let Some(link) = st.prey.iter().find(|l| Some(l.prey) == id) {
            put(at + C_PREY, rounds(link));
        }
    }
    put(G_DAYS + (st.max_days.saturating_sub(st.day) as usize).min(DAYS - 1), 1.0);
    put(G_VAULT + st.vault.len().min(VAULT - 1), 1.0);
    let mut held = [false; Tool::ALL.len()];
    for &t in &st.tools {
        held[t as usize] = true;
    }
    for (t, &h) in held.iter().enumerate() {
        put(G_TOOLS + t, f32::from(h));
    }
    put(G_REROLLS, f32::from(st.rerolls.min(3)) / 3.0);
    if let Some(o) = st.objective {
        if let Some(k) = d.objectives.iter().position(|x| x.id == o.id) {
            put(G_OBJECTIVE + k, 1.0);
        }
        put(G_PROGRESS, (o.progress as f32 / 4.0).min(1.0));
        put(G_DONE, f32::from(o.done));
    }
    let theme = st.theme.and_then(|id| THEMES.iter().position(|&t| t == id)).map_or(0, |k| k + 1);
    put(G_THEME + theme, 1.0);
    for (kind, &count) in board.iter().enumerate() {
        put(G_BOARD + kind, count as f32 / 4.0);
    }
    put(D_MANAGE, f32::from(st.phase == Phase::Manage));
    put(D_EVENT, f32::from(st.phase == Phase::Event));
    if let Some(e) = st.event
        && let Some(k) = d.incidents.iter().position(|x| x.id == e.id)
    {
        put(D_INCIDENT + k, 1.0);
    }
    // A Chain Reaction can wait into the next morning, where the morning inputs leave it out.
    put(D_CHAIN, f32::from(st.chain && !is_morning(st)));
    for (k, n) in [st.used.swap, st.used.devour, st.used.hunt].into_iter().enumerate() {
        put(D_USED + k, n.min(3) as f32 / 3.0);
    }
    let Some(first) = st.pending.first() else {
        return row;
    };
    let kind = match first {
        Pending::Tool { reason: ToolReason::Start, .. } => 0,
        Pending::Tool { reason: ToolReason::Objective, .. } => 1,
        Pending::Gift { .. } => 2,
        Pending::Interrogation { .. } => 3,
        Pending::Chain { .. } => 4,
        Pending::Cell { .. } => 5,
        Pending::Prey { .. } => 6,
    };
    put(D_PENDING + kind, 1.0);
    put(D_MORE_PENDING, (st.pending.len() - 1).min(3) as f32 / 3.0);
    // Sorted, so tool options in draw order still give ascending indices.
    let (at, mut options): (usize, Vec<usize>) = match first {
        Pending::Tool { options, .. } => (D_TOOL_OPTIONS, options.iter().map(|&t| t as usize).collect()),
        Pending::Gift { options, .. } => (D_GIFT_OPTIONS, options.iter().map(|&g| g as usize).collect()),
        Pending::Interrogation { options } => (D_CARD_OPTIONS, options.iter().map(|&c| c as usize).collect()),
        Pending::Chain { options, .. } | Pending::Cell { options, .. } | Pending::Prey { options } => (D_CELL_OPTIONS, options.clone()),
    };
    options.sort_unstable();
    for i in options {
        put(at + i, 1.0);
    }
    if let Pending::Gift { picks, .. } = first {
        put(D_GIFT_PICKS, (*picks).min(3) as f32 / 3.0);
    }
    if let Pending::Chain { from, .. } = first {
        put(D_EATER + from, 1.0);
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Action, apply, new_game};
    use crate::solver::bench::start;
    use crate::solver::solve::hide_future;
    use crate::solver::{Rng32, TUNED, decide};

    /// The morning states of games 0..8 of seed 1 played by the strategy.
    fn mornings() -> Vec<State> {
        states().into_iter().filter(is_morning).collect()
    }

    /// Every state before the end of games 0..8 of seed 1 played by the strategy.
    fn states() -> Vec<State> {
        let mut out = Vec::new();
        for k in 0..8 {
            let (mut st, mut rng) = start(1, k, &[]);
            for _ in 0..2000 {
                if st.phase == Phase::Ended {
                    break;
                }
                out.push(st.clone());
                let Some(action) = decide(&st, &TUNED, &mut rng) else {
                    break;
                };
                if !apply(&mut st, &action).is_ok() && !(action != Action::End && apply(&mut st, &Action::End).is_ok()) {
                    break;
                }
            }
        }
        out
    }

    #[test]
    fn rows_are_sparse_ascending_and_in_range() {
        let states = states();
        assert!(states.iter().filter(|st| is_morning(st)).count() >= 8);
        let mut day_block = false;
        for st in &states {
            let row = encode(st);
            assert!(row.windows(2).all(|w| w[0].0 < w[1].0), "ascending");
            for &(i, v) in &row {
                assert!((i as usize) < INPUTS);
                assert!(v.is_finite() && v != 0.0);
            }
            let beyond = row.iter().any(|&(i, _)| i as usize >= MORNING_INPUTS);
            if is_morning(st) {
                assert!(!beyond, "a morning uses the day block");
            }
            day_block |= beyond;
        }
        assert!(day_block, "no state uses the day block");
    }

    #[test]
    fn hidden_information_does_not_leak() {
        for st in &mornings() {
            let mut h = st.clone();
            hide_future(&mut h, 12345);
            assert_eq!(encode(st), encode(&h));
        }
    }

    #[test]
    fn first_morning_encodes_the_warden_and_days_left() {
        // A fresh game waits on its starting instrument pick, so it is not a morning yet.
        let mut st = new_game("1-0");
        assert!(!is_morning(&st));
        let mut rng = Rng32::new(7);
        while !is_morning(&st) {
            let action = decide(&st, &TUNED, &mut rng).expect("a move");
            assert!(apply(&mut st, &action).is_ok());
        }
        let row = encode(&st);
        assert!(row.contains(&((st.position * CELL + C_WARDEN) as u16, 1.0)));
        let days = (st.max_days - st.day) as usize;
        let day_inputs: Vec<u16> = row.iter().map(|&(i, _)| i).filter(|&i| (G_DAYS..G_DAYS + DAYS).contains(&(i as usize))).collect();
        assert_eq!(day_inputs, vec![(G_DAYS + days.min(DAYS - 1)) as u16]);
        assert!(row.contains(&((G_VAULT) as u16, 1.0)));
        let globals = row.iter().filter(|&&(i, _)| i as usize >= G_DAYS).count();
        assert!(globals <= 21, "{globals} globals");
    }
}
