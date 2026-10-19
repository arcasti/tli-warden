//! Setup, day flow, doors and incident markers (js/engine.js lines 406-1260).
//! Function names follow the JS ones in snake case.

use super::data::{Beast, Gift, IRON_LAW, OPULENCE, ObjectiveKind, RAINBOW, Spawn, Tool, data};
use super::rng::{hash_seed, pick_index, pick_one, rand, shuffle};
use super::state::{
    Cell, EventRef, Inmate, ObjectiveState, Params, Pending, Phase, Roster, RosterRow, RunResult, State, Stats, ToolReason,
    Used, VaultEntry,
};
use std::collections::BTreeMap;

const VERSION: u32 = 4;

/// Who opened a door: a patrol, the run start, a side door (no triggers) or
/// an instrument whose effect lets "after opening" instruments fire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    Patrol,
    Start,
    Side,
    Tool,
}

/// Steps clockwise from a to b, so 0 means the same cell.
pub fn steps_between(a: usize, b: usize) -> usize {
    let cells = data().cell_count;
    (b + cells - a) % cells
}

pub fn weights<const N: usize>(list: &[u32; N]) -> [f64; N] {
    list.map(f64::from)
}

/// A held instrument's trigger roll at its data chance; no draw when not held.
pub fn fires(s: &mut State, tool: Tool) -> bool {
    s.has(tool) && rand(&mut s.rng) * 100.0 < data().tool(tool).chance
}

pub fn make_inmate(s: &mut State, beast: Beast, tier: i32) -> Inmate {
    let id = s.next_id;
    s.next_id += 1;
    Inmate { id, beast, tier: tier.clamp(0, s.top_tier()) }
}

/// Rarity odds for whoever is behind a door. The Sorting Hook keeps out Normals.
pub fn door_weights(s: &State) -> [f64; RAINBOW as usize] {
    let mut w = weights(&s.params.tier_weights);
    if s.has(Tool::Hook) && w[1..].iter().any(|&x| x > 0.0) {
        w[0] = 0.0;
    }
    w
}

pub fn roll_inmate(s: &mut State, tier_weights: &[f64]) -> Inmate {
    let tier = pick_index(&mut s.rng, tier_weights) as i32;
    let beast = Beast::ALL[pick_index(&mut s.rng, &weights(&s.params.beast_weights))];
    make_inmate(s, beast, tier)
}

/// Who comes out of a door: the inmate already behind it, unless the Sorting
/// Hook rules out a Normal the player hasn't seen.
fn door_inmate(s: &mut State, n: usize) -> Inmate {
    let cell = &s.cells[n];
    if let Some(h) = cell.hidden
        && (h.tier > 0 || cell.known > 0 || !s.has(Tool::Hook))
    {
        return h;
    }
    let w = door_weights(s);
    roll_inmate(s, &w)
}

/// Ends every prey mark that is no longer valid.
pub fn check_prey(s: &mut State) {
    let links = std::mem::take(&mut s.prey);
    s.prey = links
        .into_iter()
        .filter(|link| {
            let lion = s.board_cell_of(link.lion).and_then(|n| s.cells[n].inmate);
            let prey = s.board_cell_of(link.prey).and_then(|n| s.cells[n].inmate);
            matches!((lion, prey), (Some(l), Some(p)) if l.beast == Beast::Lion && p.beast == link.prey_beast)
        })
        .collect();
}

/// Empties a cell. With reusable cells it seals again.
pub fn empty_cell(s: &mut State, n: usize) -> Option<Inmate> {
    let sealed = s.params.reusable_cells;
    let cell = &mut s.cells[n];
    let inmate = cell.inmate.take();
    cell.hidden = None;
    cell.known = 0;
    cell.scratch = false;
    cell.nest = None;
    cell.ivory = false;
    cell.sealed = sealed;
    inmate
}

/// Draws up to `count` distinct instruments the warden can still take, by
/// weight, without the held ones and any a held instrument excludes.
pub fn draw_tools(s: &mut State, count: usize) -> Vec<Tool> {
    let d = data();
    let barred: Vec<Tool> = s.tools.iter().flat_map(|&t| d.tool(t).exclude.iter().copied()).collect();
    let mut pool: Vec<Tool> = d.tool_ids.iter().copied().filter(|t| !s.has(*t) && !barred.contains(t) && d.tool(*t).weight > 0).collect();
    let mut out = Vec::new();
    while out.len() < count && !pool.is_empty() {
        let w: Vec<f64> = pool.iter().map(|&t| f64::from(d.tool(t).weight)).collect();
        out.push(pool.remove(pick_index(&mut s.rng, &w)));
    }
    out
}

/// Rewards for growth: objective progress, and the vault for anyone who
/// reaches Chromatic.
pub fn after_growth(s: &mut State, inmate: Inmate) {
    let d = data();
    s.stats.best = s.stats.best.max(inmate.tier);
    if let Some(goal) = s.objective
        && d.objective(goal.id).is_some_and(|o| inmate.tier >= o.rating)
    {
        advance_objective(s, ObjectiveKind::ReachRating, 1);
    }
    if inmate.tier < RAINBOW {
        return;
    }
    let Some(n) = s.cell_of(inmate.id) else { return };
    empty_cell(s, n);
    s.vault.push(VaultEntry { id: inmate.id, beast: inmate.beast, day: s.day });
    s.cache += 1;
    if s.params.rewards {
        s.pending.push(Pending::Gift { options: Gift::ALL.to_vec(), picks: treasure_picks(s.theme) });
    }
}

/// The picks a Warden's Treasure gives in `theme`: Opulence gives more from the same treasure.
pub fn treasure_picks(theme: Option<u32>) -> u32 {
    let extra = if theme == Some(OPULENCE) { data().theme(OPULENCE).and_then(|t| t.value).unwrap_or(0) as u32 } else { 0 };
    1 + extra
}

/// Objective progress; a completed objective queues an instrument pick.
pub fn advance_objective(s: &mut State, kind: ObjectiveKind, amount: u32) {
    let d = data();
    let Some(goal) = s.objective else { return };
    if goal.done || amount == 0 {
        return;
    }
    let Some(objective) = d.objective(goal.id) else { return };
    if objective.kind != kind {
        return;
    }
    let progress = objective.target.min(goal.progress + amount);
    let done = progress >= objective.target;
    s.objective = Some(ObjectiveState { id: goal.id, progress, done });
    if !done || !s.params.rewards {
        return;
    }
    let options = draw_tools(s, d.instrument_picks);
    if !options.is_empty() {
        s.pending.push(Pending::Tool { options, reason: ToolReason::Objective });
    }
}

/// Opens a sealed cell; `forced` places that beast and tier instead of the
/// occupant behind the door.
pub fn open_cell(s: &mut State, n: usize, how: How, forced: Option<(Beast, i32)>) {
    if !s.cells[n].sealed {
        return;
    }
    let inmate = match forced {
        Some((beast, tier)) => make_inmate(s, beast, tier),
        None => door_inmate(s, n),
    };
    let cell = &mut s.cells[n];
    cell.sealed = false;
    cell.inmate = Some(inmate);
    cell.hidden = None;
    cell.known = 0;
    cell.scratch = false;
    cell.nest = None;
    s.stats.opened += 1;
    if how != How::Start {
        advance_objective(s, ObjectiveKind::OpenCellCount, 1);
    }
    after_growth(s, inmate);
    if how != How::Patrol && how != How::Tool {
        return;
    }
    super::tools::after_opening(s, n, inmate.id)
}

/// Opens `count` random unopened cells for an instrument or incident.
pub fn open_random(s: &mut State, count: usize, how: How) {
    let mut doors = s.sealed_cells();
    shuffle(&mut s.rng, &mut doors);
    doors.truncate(count);
    for n in doors {
        open_cell(s, n, how, None);
    }
}

// ---------- Prison Incident markers ----------

/// SpawnCellState; the Interrogation Room never carries a marker.
pub fn incident_fits(id: u32, cell: &Cell) -> bool {
    let d = data();
    if cell.n == d.room {
        return false;
    }
    match d.incident(id).spawn {
        Spawn::Any => true,
        Spawn::Unopened => cell.sealed,
        Spawn::Opened => !cell.sealed,
    }
}

pub fn incident_weight(s: &State, id: u32) -> u32 {
    s.incident_weights.get(&id).copied().unwrap_or_else(|| data().incident(id).weight)
}

/// Incidents that can be drawn now: weight above 0, no held instrument in
/// excludeRelic, under repeatLimit when the data sets one.
fn incident_pool(s: &State) -> Vec<u32> {
    data()
        .incidents
        .iter()
        .filter(|e| {
            incident_weight(s, e.id) > 0
                && !e.exclude.iter().any(|&t| s.has(t))
                && !(e.repeat_limit > 0 && s.incident_counts.get(&e.id).copied().unwrap_or(0) >= e.repeat_limit)
        })
        .map(|e| e.id)
        .collect()
}

/// Draws an incident by weight among those that fit at least one of `spots`.
pub fn draw_incident(s: &mut State, spots: &[usize]) -> Option<u32> {
    let pool: Vec<u32> = incident_pool(s).into_iter().filter(|&id| spots.iter().any(|&n| incident_fits(id, &s.cells[n]))).collect();
    if pool.is_empty() {
        return None;
    }
    let w: Vec<f64> = pool.iter().map(|&id| f64::from(incident_weight(s, id))).collect();
    Some(pool[pick_index(&mut s.rng, &w)])
}

fn free_cells(s: &State) -> Vec<usize> {
    let room = data().room;
    s.cells.iter().filter(|c| c.n != room && c.event.is_none()).map(|c| c.n).collect()
}

/// Puts a marker on `cell`, or on a random free cell that fits the drawn incident.
pub fn add_marker(s: &mut State, cell: Option<usize>) -> bool {
    let free = match cell {
        Some(n) => vec![n],
        None => free_cells(s),
    };
    let Some(id) = draw_incident(s, &free) else { return false };
    let fit: Vec<usize> = free.into_iter().filter(|&n| incident_fits(id, &s.cells[n])).collect();
    let at = fit[pick_one(&mut s.rng, fit.len())];
    s.cells[at].event = Some(id);
    *s.incident_counts.entry(id).or_insert(0) += 1;
    true
}

/// Drops markers whose cell no longer fits and tops the board up to the minimum.
pub fn sync_markers(s: &mut State) {
    for c in &mut s.cells {
        if let Some(id) = c.event
            && !incident_fits(id, c)
        {
            c.event = None;
        }
    }
    let mut count = s.cells.iter().filter(|c| c.event.is_some()).count();
    while count < data().min_markers && add_marker(s, None) {
        count += 1;
    }
}

// ---------- Setup ----------

/// newGame for the `empty` scenario (a full run) with default params.
pub fn new_game(seed: &str) -> State {
    new_game_with(seed, Params::default())
}

/// newGame for the `empty` scenario with the given params.
pub fn new_game_with(seed: &str, params: Params) -> State {
    let d = data();
    let seed: String = seed.trim().to_uppercase().chars().take(40).collect();
    let scenario = "empty".to_string();
    let mut s = State {
        v: VERSION,
        rng: hash_seed(&format!("{seed}|{scenario}")),
        scenario,
        seed,
        day: 1,
        max_days: params.drill_days,
        params,
        phase: Phase::Patrol,
        position: d.room,
        dice: Vec::new(),
        destinations: Vec::new(),
        chain: false,
        event: None,
        incident_counts: BTreeMap::new(),
        incident_weights: BTreeMap::new(),
        cells: (0..d.cell_count).map(Cell::blank).collect(),
        vault: Vec::new(),
        cache: 0,
        tools: Vec::new(),
        pending: Vec::new(),
        objective: None,
        theme: None,
        used: Used::default(),
        prey: Vec::new(),
        next_id: 1,
        stats: Stats::default(),
        result: None,
    };
    draw_run_goals(&mut s);
    run_start(&mut s);
    begin_day(&mut s);
    sync_markers(&mut s);
    s
}

/// The run's objective and special theme, from a stream of their own.
fn draw_run_goals(s: &mut State) {
    let d = data();
    let mut stream = hash_seed(&format!("{}|goals", s.seed));
    let mut objective = d.objective(s.params.objective);
    if objective.is_none() && d.objectives_per_run > 0 {
        let w: Vec<f64> = d.objectives.iter().map(|o| f64::from(o.weight)).collect();
        objective = Some(&d.objectives[pick_index(&mut stream, &w)]);
    }
    s.objective = objective.map(|o| ObjectiveState { id: o.id, progress: 0, done: false });
    let mut theme = u32::try_from(s.params.theme).ok().filter(|&id| d.theme(id).is_some());
    if s.params.theme == 0 {
        let roll = rand(&mut stream) * 10000.0;
        // The caller's runsWithoutTheme is 0 here.
        let pity = d.theme_pity_runs == 0;
        if pity || roll < d.theme_chance_per_10000 {
            let w: Vec<f64> = d.themes.iter().map(|t| f64::from(t.weight)).collect();
            theme = Some(d.themes[pick_index(&mut stream, &w)].id);
        }
    }
    s.theme = theme;
    if theme == Some(IRON_LAW) {
        let days = d.theme(IRON_LAW).and_then(|t| t.value).unwrap_or(0) as u32;
        s.max_days = s.max_days.max(days);
    }
}

/// The run start: random rooms open and the player picks an instrument.
fn run_start(s: &mut State) {
    let d = data();
    let mut rooms = s.sealed_cells();
    shuffle(&mut s.rng, &mut rooms);
    rooms.truncate(d.open_rooms_at_start);
    for n in rooms {
        open_cell(s, n, How::Start, None);
    }
    let options = draw_tools(s, d.instrument_picks);
    if !options.is_empty() {
        s.pending.insert(0, Pending::Tool { options, reason: ToolReason::Start });
    }
}

// ---------- Day flow ----------

/// Today's moves from the dice: the cells they land on, nearest first, the room only when a die
/// lands there (the S16 cards, docs/s16-screen.md). `roll_dice` draws distinct steps, so only a
/// state copied from a JS trace (tests/common `follow_js_repeat`) can hold a repeated step.
pub fn destinations_from_dice(s: &mut State) {
    let d = data();
    let pos = s.position;
    let mut ahead: Vec<usize> = Vec::new();
    for &pips in &s.dice {
        let n = (pos + pips as usize) % d.cell_count;
        if !ahead.contains(&n) {
            ahead.push(n);
        }
    }
    ahead.sort_by_key(|&n| steps_between(pos, n));
    s.destinations = ahead;
}

/// "-1 daily destination option" (Barbed Whip, Forged Key, Burning Stake).
pub fn fewer_dice(s: &State) -> i32 {
    s.tools.iter().map(|&t| data().tool(t).fewer_dice).sum()
}

/// Rolls today's dice and derives today's moves from them. The game draws the steps as distinct
/// values (46 of 46 recorded three-card mornings showed three different steps), so a die that
/// repeats an earlier one is redrawn: uniform over the distinct sets, and the same rng stream as
/// independent dice on a morning without a repeat.
pub fn roll_dice(s: &mut State) {
    let d = data();
    let fewer = rand(&mut s.rng) * 100.0 < f64::from(s.params.fewer_doors_chance);
    let pips = d.max_pips - d.min_pips + 1;
    let count = (s.params.destinations as i32 - i32::from(fewer) - fewer_dice(s)).clamp(1, pips as i32) as usize;
    let mut dice: Vec<u32> = Vec::with_capacity(count);
    while dice.len() < count {
        let die = d.min_pips + (rand(&mut s.rng) * f64::from(pips)).floor() as u32;
        if !dice.contains(&die) {
            dice.push(die);
        }
    }
    s.dice = dice;
    destinations_from_dice(s);
}

pub fn begin_day(s: &mut State) {
    let d = data();
    s.phase = Phase::Patrol;
    s.used = Used::default();
    check_prey(s);
    super::tools::prey_rise(s);
    check_prey(s);
    for n in 0..s.cells.len() {
        if n != d.room && s.cells[n].event.is_none() && rand(&mut s.rng) * 10000.0 < f64::from(s.params.incident_spawn_chance) {
            add_marker(s, Some(n));
        }
    }
    super::tools::hatch_nests(s);
    super::tools::morning_tools(s);
    roll_dice(s);
}

pub fn clockwise_path(s: &State, to: usize) -> Vec<usize> {
    let cells = data().cell_count;
    let mut path = Vec::new();
    let mut n = s.position;
    while n != to {
        n = (n + 1) % cells;
        path.push(n);
    }
    if path.is_empty() { vec![to] } else { path }
}

pub fn patrol_options(s: &State) -> Vec<usize> {
    if s.phase == Phase::Patrol { s.destinations.clone() } else { Vec::new() }
}

/// A move onto cell n and what the landing triggers.
pub fn land(s: &mut State, n: usize) {
    let d = data();
    let marker = if n == d.room { None } else { s.cells[n].event };
    s.cells[n].event = None;
    if s.chain {
        // Chain Reaction: every sealed door on the clockwise walk opens.
        for k in clockwise_path(s, n) {
            open_cell(s, k, How::Side, None);
        }
        s.chain = false;
    } else if n != d.room && s.cells[n].sealed {
        open_cell(s, n, How::Patrol, None);
    }
    let steps = steps_between(s.position, n);
    s.position = n;
    advance_objective(s, ObjectiveKind::MoveDistance, steps as u32);
    super::tools::shackles(s, steps);
    if n == d.room {
        super::gifts::interrogation_room_stop(s);
    }
    check_prey(s);
    match marker {
        Some(id) => {
            s.phase = Phase::Event;
            s.event = Some(EventRef { id, cell: n });
        }
        None => s.phase = Phase::Manage,
    }
}

pub fn advance_day(s: &mut State) {
    if s.day >= s.max_days {
        finish(s, false);
        return;
    }
    s.day += 1;
    begin_day(s)
}

pub fn finish(s: &mut State, early: bool) {
    s.phase = Phase::Ended;
    s.dice.clear();
    s.destinations.clear();
    s.event = None;
    s.chain = false;
    let roster = roster_of(s);
    let best = roster.rows.iter().map(|r| r.tier).fold(s.stats.best.max(0), i32::max);
    s.result = Some(RunResult { day: s.day, early, roster, cache: s.cache, best, stats: s.stats });
}

pub fn roster_of(s: &State) -> Roster {
    let mut rows: Vec<RosterRow> = s
        .cells
        .iter()
        .filter(|c| c.on_board())
        .filter_map(|c| c.inmate.map(|i| RosterRow { beast: i.beast, tier: i.tier, place: format!("Cell {:02}", c.n + 1) }))
        .collect();
    rows.extend(s.vault.iter().map(|v| RosterRow { beast: v.beast, tier: RAINBOW, place: format!("Vault, day {}", v.day) }));
    rows.sort_by(|a, b| b.tier.cmp(&a.tier).then(a.beast.index().cmp(&b.beast.index())));
    let by_tier = (0..=RAINBOW).map(|t| rows.iter().filter(|r| r.tier == t).count() as u32).collect();
    Roster { rows, by_tier }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rolls `mornings` seeded mornings and counts each pip, checking every roll is `count` distinct pips.
    fn roll_many(s: &mut State, mornings: usize, count: usize) -> [usize; 7] {
        let mut seen = [0usize; 7];
        for _ in 0..mornings {
            roll_dice(s);
            let mut dice = s.dice.clone();
            dice.sort_unstable();
            dice.dedup();
            assert_eq!(dice.len(), count, "dice {:?}", s.dice);
            for &p in &dice {
                assert!((1..=6).contains(&p), "pip {p}");
                seen[p as usize] += 1;
            }
        }
        seen
    }

    #[test]
    fn dice_are_distinct_and_uniform() {
        let mut s = new_game("DISTINCT-DICE");
        s.params.fewer_doors_chance = 0;
        s.tools.clear();
        let seen = roll_many(&mut s, 6000, 3);
        // Each pip shows on half the mornings: 3000 expected, sd about 39.
        for (pip, &n) in seen.iter().enumerate().skip(1) {
            assert!((2800..=3200).contains(&n), "pip {pip} seen {n} times");
        }
        s.tools.push(Tool::Whip);
        roll_many(&mut s, 2000, 2);
    }

    /// Promotes an Ultimate on a board whose vault already holds `vaulted` Chromatics.
    fn promote_ultimate(vaulted: usize) -> State {
        let mut s = new_game("VAULT-CAP");
        s.pending.clear();
        s.vault = (0..vaulted as u32).map(|id| VaultEntry { id: 1000 + id, beast: Beast::ALL[0], day: 1 }).collect();
        s.cells[0].sealed = false;
        let ultimate = make_inmate(&mut s, Beast::ALL[0], RAINBOW - 1);
        s.cells[0].inmate = Some(ultimate);
        crate::engine::tools::promote(&mut s, ultimate, 1, false);
        s
    }

    #[test]
    fn a_full_vault_stops_inmates_at_ultimate() {
        let gift = |s: &State| s.pending.iter().any(|p| matches!(p, Pending::Gift { .. }));
        let full = promote_ultimate(10);
        assert_eq!(full.cells[0].inmate.map(|i| i.tier), Some(RAINBOW - 1));
        assert_eq!(full.vault.len(), 10);
        assert!(!gift(&full));
        let room = promote_ultimate(9);
        assert_eq!(room.cells[0].inmate, None);
        assert_eq!(room.vault.len(), 10);
        assert!(gift(&room));
    }
}
