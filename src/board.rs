//! The board model: what a player can see of a run (every cell, the warden,
//! today's doors, the instruments, the vault), and `build_state`, which turns
//! it into an engine state the solver can play from. A port of js/analyze.js
//! (blankModel, normalize, buildState).

use crate::engine::data::{Beast, Card, Gift, RAINBOW, Tool, data};
use crate::engine::incidents::{CHAIN_REACTION, EXECUTION_EFFICIENCY, SEWER_RATWAY};
use crate::engine::devour::{HUNT_NEEDS, can_eat, hunt_targets};
use crate::engine::rules::{new_game_with, steps_between};
use crate::engine::state::{Cell, EventRef, Inmate, Nest, ObjectiveState, Params, Pending, Phase, PreyLink, State, ToolReason, Used, VaultEntry};
use crate::engine::text::cell_label;
use crate::solver::Rng32;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Cells hold Normal to Ultimate: Chromatics leave for the vault.
const TOP_TIER: i64 = RAINBOW as i64 - 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Step {
    Patrol,
    Manage,
    Event,
    Choice,
}

/// The incident behind a marker: "unknown" when the player sees the marker
/// but not its kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EventMark {
    Execution,
    Chain,
    Unknown,
}

/// A Lion's prey mark: the marked cell and the rounds left.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct PreyMark {
    /// None when the saved value is not an integer (no cell).
    pub cell: Option<i64>,
    pub rounds: i64,
}

/// One cell as the player sees it. Tiers and due days are kept as entered
/// and clamped by `build_state`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum CellModel {
    Office,
    Sealed { event: Option<EventMark> },
    /// An open cell can hold a marker an incident put there.
    Empty { event: Option<EventMark> },
    Inmate { beast: Beast, tier: i64, ivory: bool, prey: Option<PreyMark>, event: Option<EventMark> },
    /// None: due tomorrow.
    Nest { due: Option<i64> },
    Scouted { beast: Beast, tier: i64, event: Option<EventMark> },
    Scratch { tier: i64, event: Option<EventMark> },
}

impl CellModel {
    /// The cell's incident marker, if any.
    pub fn event(&self) -> Option<EventMark> {
        match self {
            CellModel::Sealed { event }
            | CellModel::Empty { event }
            | CellModel::Inmate { event, .. }
            | CellModel::Scouted { event, .. }
            | CellModel::Scratch { event, .. } => *event,
            CellModel::Office | CellModel::Nest { .. } => None,
        }
    }

    /// Whether the cell's inmate carries the Tusk.
    pub fn ivory(&self) -> bool {
        matches!(self, CellModel::Inmate { ivory: true, .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChoiceKind {
    Tool,
    Gift,
    /// The Sewer Ratway's "Select an Objective": any cell but the room.
    Cell,
    /// The Interrogation Room's Instigate or Bestow.
    Interrogation,
    /// The Hunt's target pick: one of the Lion's Consumable targets.
    Prey,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Choice {
    pub kind: ChoiceKind,
    pub options: Vec<Tool>,
}

/// The analyzer's board model. Deserializing goes through `normalize`, so
/// missing or malformed fields take the blank model's values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "Value")]
pub struct Model {
    pub v: u32,
    pub day: i64,
    pub max_days: i64,
    pub step: Step,
    pub warden: i64,
    pub dest: Vec<usize>,
    pub devour_used: bool,
    pub swap_used: bool,
    pub hunt_used: bool,
    pub event_kind: EventKindModel,
    pub choice: Choice,
    pub chain_armed: bool,
    pub tools: Vec<Tool>,
    pub vault: Vec<Beast>,
    pub cells: Vec<CellModel>,
    /// The run's objective, when the player has read it off the objective panel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub objective: Option<ObjectiveModel>,
}

/// The run's objective: its data id and the progress shown on the panel.
/// Completion follows from the data target (1 for a rating objective).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ObjectiveModel {
    pub id: u32,
    pub progress: u32,
}

/// The kind of the incident in the warden's cell on the event step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EventKindModel {
    Execution,
    Chain,
}

impl Model {
    /// blankModel: day 1 in the office, every cell sealed.
    pub fn blank() -> Model {
        let d = data();
        Model {
            v: 1,
            day: 1,
            max_days: i64::from(Params::default().drill_days),
            step: Step::Patrol,
            warden: d.room as i64,
            dest: Vec::new(),
            devour_used: false,
            swap_used: false,
            hunt_used: false,
            event_kind: EventKindModel::Chain,
            choice: Choice { kind: ChoiceKind::Tool, options: Vec::new() },
            chain_armed: false,
            tools: Vec::new(),
            vault: Vec::new(),
            cells: (0..d.cell_count).map(|n| if n == d.room { CellModel::Office } else { CellModel::Sealed { event: None } }).collect(),
            objective: None,
        }
    }
}

impl From<Value> for Model {
    fn from(v: Value) -> Model {
        normalize(&v)
    }
}

/// JS Number(v) for JSON values; NaN for what it can't read.
fn js_number(v: &Value) -> f64 {
    match v {
        Value::Null => 0.0,
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => {
            let s = s.trim();
            if s.is_empty() { 0.0 } else { s.parse().unwrap_or(f64::NAN) }
        }
        _ => f64::NAN,
    }
}

/// toInt: a number or numeric string, rounded half up as Math.round; the
/// fallback for anything else or a missing value.
fn to_int(v: Option<&Value>, fallback: i64) -> i64 {
    let x = v.map_or(f64::NAN, js_number);
    if !x.is_finite() {
        return fallback;
    }
    let f = x.floor();
    (if x - f >= 0.5 { f + 1.0 } else { f }) as i64
}

/// JS truthiness.
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|x| x != 0.0 && !x.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

/// A JSON number that is an integer (Number.isInteger).
fn integer(v: &Value) -> Option<i64> {
    v.as_f64().filter(|x| x.is_finite() && x.fract() == 0.0).map(|x| x as i64)
}

fn strings(v: Option<&Value>) -> impl Iterator<Item = &str> {
    v.and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str)
}

fn event_mark(v: Option<&Value>) -> Option<EventMark> {
    if !truthy(v) {
        return None;
    }
    Some(match v.and_then(Value::as_str) {
        Some("execution") => EventMark::Execution,
        Some("unknown") => EventMark::Unknown,
        _ => EventMark::Chain,
    })
}

fn beast(v: Option<&Value>) -> Beast {
    v.cloned().and_then(|b| serde_json::from_value(b).ok()).unwrap_or(Beast::Mammoth)
}

fn cell_model(c: &Value) -> CellModel {
    let empty = Map::new();
    let c = c.as_object().unwrap_or(&empty);
    let tier = to_int(c.get("tier"), 0);
    let event = event_mark(c.get("event"));
    match c.get("kind").and_then(Value::as_str) {
        Some("empty") => CellModel::Empty { event },
        Some("inmate") => CellModel::Inmate {
            beast: beast(c.get("beast")),
            tier,
            ivory: truthy(c.get("ivory")),
            prey: truthy(c.get("prey")).then(|| PreyMark {
                cell: c["prey"].get("cell").and_then(integer),
                rounds: to_int(c["prey"].get("rounds"), 1),
            }),
            event,
        },
        Some("nest") => CellModel::Nest { due: c.get("due").filter(|v| js_number(v).is_finite()).map(|v| to_int(Some(v), 0)) },
        Some("scouted") => CellModel::Scouted { beast: beast(c.get("beast")), tier, event },
        Some("scratch") => CellModel::Scratch { tier, event },
        // Sealed, or a kind the analyzer does not know: a sealed door.
        _ => CellModel::Sealed { event },
    }
}

/// normalize: the blank model with whatever `v` sets; a cells list of the
/// wrong length gives the blank model.
fn normalize(v: &Value) -> Model {
    let d = data();
    let mut m = Model::blank();
    let Some(o) = v.as_object() else { return m };
    let Some(cells) = o.get("cells").and_then(Value::as_array).filter(|c| c.len() == d.cell_count) else { return m };
    if let Some(x) = o.get("day") {
        m.day = to_int(Some(x), 1);
    }
    if let Some(x) = o.get("maxDays") {
        m.max_days = to_int(Some(x), 20);
    }
    if let Some(x) = o.get("warden") {
        m.warden = to_int(Some(x), 0);
    }
    m.step = match o.get("step").and_then(Value::as_str) {
        Some("manage") => Step::Manage,
        Some("event") => Step::Event,
        Some("choice") => Step::Choice,
        _ => Step::Patrol,
    };
    // Negative doors pass the JS filter; they are dropped here.
    m.dest = o
        .get("dest")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(integer)
        .filter(|&n| n >= 0 && (n as usize) < d.cell_count)
        .map(|n| n as usize)
        .collect();
    m.devour_used = truthy(o.get("devourUsed"));
    m.swap_used = truthy(o.get("swapUsed"));
    m.hunt_used = truthy(o.get("huntUsed"));
    if o.get("eventKind").and_then(Value::as_str) == Some("execution") {
        m.event_kind = EventKindModel::Execution;
    }
    if let Some(choice) = o.get("choice").and_then(Value::as_object) {
        match choice.get("kind").and_then(Value::as_str) {
            Some("gift") => m.choice.kind = ChoiceKind::Gift,
            Some("cell") => m.choice.kind = ChoiceKind::Cell,
            Some("prey") => m.choice.kind = ChoiceKind::Prey,
            Some("interrogation") => m.choice.kind = ChoiceKind::Interrogation,
            _ => {}
        }
        m.choice.options = strings(choice.get("options")).filter_map(Tool::from_key).collect();
    }
    m.chain_armed = truthy(o.get("chainArmed"));
    m.tools = strings(o.get("tools")).filter_map(Tool::from_key).collect();
    m.vault = o.get("vault").and_then(Value::as_array).into_iter().flatten().filter_map(|b| serde_json::from_value(b.clone()).ok()).collect();
    m.objective = o.get("objective").and_then(Value::as_object).and_then(|g| {
        let id = g.get("id").and_then(integer).filter(|&id| id >= 0)?;
        Some(ObjectiveModel { id: id as u32, progress: to_int(g.get("progress"), 0).max(0) as u32 })
    });
    m.cells = cells.iter().enumerate().map(|(n, c)| if n == d.room { CellModel::Office } else { cell_model(c) }).collect();
    m
}

/// cellLabel for a prey mark's cell, "NaN" for one that is not an integer.
fn label(n: Option<i64>) -> String {
    n.map_or_else(|| "NaN".to_string(), |n| format!("{:02}", n + 1))
}

fn make(next_id: &mut u32, beast: Beast, tier: i64) -> Inmate {
    let id = *next_id;
    *next_id += 1;
    Inmate { id, beast, tier: tier.clamp(0, TOP_TIER) as i32 }
}

/// buildState: the engine state for a model, and anything that stops it from
/// being analyzed. `seed` stands in for the JS Math.random: the state's dice,
/// then a scratch door's beast, in cell order. An incident the player can't
/// name keeps a kind the solver re-rolls.
pub fn build_state(m: &Model, seed: u32) -> (State, Vec<String>) {
    let d = data();
    let mut problems = Vec::new();
    let mut random = Rng32::new(seed);
    let drill_days = m.max_days.clamp(5, 60) as u32;
    // Logged Replaces holding the Severed Iron Maiden never rose (none of the
    // about 10 its rule predicted), so a read board gives it no rise.
    let mut st = new_game_with("ANALYZER", Params { drill_days, maiden: false, ..Params::default() });
    st.rng = (random.roll() * 4294967296.0) as u32;
    // The objective comes from the model when the player read it; the board
    // shows no theme, so the run's own draw would add one.
    st.objective = None;
    if let Some(goal) = m.objective {
        match d.objective(goal.id) {
            Some(o) => {
                let progress = goal.progress.min(o.target);
                st.objective = Some(ObjectiveState { id: goal.id, progress, done: progress >= o.target });
            }
            None => problems.push(format!("There is no objective with id {}.", goal.id)),
        }
    }
    st.theme = None;
    st.day = m.day.clamp(1, i64::from(st.max_days)) as u32;
    st.next_id = 1;
    let day = st.day;
    let mut cells = Vec::with_capacity(m.cells.len());
    for (n, c) in m.cells.iter().enumerate() {
        let mut cell = Cell::blank(n);
        if n != d.room {
            match c {
                CellModel::Empty { event } => {
                    cell.sealed = false;
                    set_event(&mut cell, event);
                }
                CellModel::Inmate { beast, tier, ivory, event, .. } => {
                    cell.sealed = false;
                    cell.inmate = Some(make(&mut st.next_id, *beast, *tier));
                    cell.ivory = *ivory;
                    set_event(&mut cell, event);
                }
                // A nest due today hatched this morning, so a sealed one is due later.
                CellModel::Nest { due } => cell.nest = Some(Nest { due: due.unwrap_or(i64::from(day) + 1).max(i64::from(day) + 1) as u32 }),
                CellModel::Scouted { beast, tier, event } => {
                    cell.known = 2;
                    cell.hidden = Some(make(&mut st.next_id, *beast, *tier));
                    set_event(&mut cell, event);
                }
                // A "?" with a rarity glow is a Scratch mark only with Filthy
                // Claws; an incident that shows the rarity behind doors draws
                // the same glow on doors a Wolf can't eat through.
                CellModel::Scratch { tier, event } => {
                    cell.scratch = m.tools.contains(&Tool::Claws);
                    cell.known = 1;
                    let beast = Beast::ALL[(random.roll() * 4.0).floor() as usize];
                    cell.hidden = Some(make(&mut st.next_id, beast, *tier));
                    set_event(&mut cell, event);
                }
                CellModel::Sealed { event } => set_event(&mut cell, event),
                CellModel::Office => {}
            }
        }
        cells.push(cell);
    }
    st.cells = cells;

    // Prey marks: a Lion and the same-rarity meal it marked.
    st.prey = Vec::new();
    for (n, c) in m.cells.iter().enumerate() {
        let CellModel::Inmate { beast: Beast::Lion, prey: Some(mark), .. } = c else { continue };
        let Some(lion) = st.cells[n].inmate else { continue };
        let target = mark.cell.and_then(|t| usize::try_from(t).ok()).and_then(|t| st.cells.get(t)).and_then(|t| t.inmate);
        let Some(meal) = target.filter(|meal| can_eat(&st, &lion, Some(meal))) else {
            problems.push(format!("The prey mark from the Lion in cell {} needs a same-rarity meal in cell {}.", cell_label(n), label(mark.cell)));
            continue;
        };
        if st.prey.iter().any(|l| [l.lion, l.prey].contains(&meal.id) || [l.lion, l.prey].contains(&lion.id)) {
            problems.push(format!("Cell {} already has a prey mark.", label(mark.cell)));
            continue;
        }
        st.prey.push(PreyLink { lion: lion.id, prey: meal.id, prey_beast: meal.beast, rounds: mark.rounds.clamp(1, 9) as i32, next: day + 1 });
    }

    st.vault = m.vault.iter().map(|&beast| make_vault(&mut st.next_id, beast)).collect();
    st.cache = st.vault.len() as u32;
    st.tools = d.tool_ids.iter().copied().filter(|t| m.tools.contains(t)).collect();
    st.position = m.warden.clamp(0, d.cell_count as i64 - 1) as usize;
    st.chain = m.chain_armed;
    let top = st.cells.iter().filter_map(|c| c.inmate).map(|i| i.tier).max().unwrap_or(0);
    st.stats.best = top.max(0).max(if st.vault.is_empty() { 0 } else { RAINBOW });
    st.used = Used::default();
    st.destinations = Vec::new();
    st.event = None;
    st.pending = Vec::new();
    st.phase = Phase::Manage;

    let used_today = Used { swap: u32::from(m.swap_used), devour: u32::from(m.devour_used), hunt: u32::from(m.hunt_used) };
    match m.step {
        Step::Patrol => {
            st.phase = Phase::Patrol;
            let mut doors: Vec<usize> = Vec::new();
            for &n in &m.dest {
                if n < d.cell_count && !doors.contains(&n) {
                    doors.push(n);
                }
            }
            doors.sort_by_key(|&n| steps_between(st.position, n));
            if doors.is_empty() {
                problems.push("Mark today's doors: tick them under \u{201c}Select a Destination\u{201d}, or select a cell and press Today's door.".to_string());
            }
            st.destinations = doors;
        }
        Step::Event => {
            let here = &st.cells[st.position];
            if here.sealed || here.inmate.is_none() {
                problems.push(format!(
                    "A {} needs the Inmate that came out of the door in the warden's cell: put the warden there and set its Inmate.",
                    d.incident_term
                ));
            }
            st.phase = Phase::Event;
            let id = if m.event_kind == EventKindModel::Execution { EXECUTION_EFFICIENCY } else { CHAIN_REACTION };
            st.event = Some(EventRef { id, cell: st.position });
        }
        Step::Choice => {
            st.used = used_today;
            // The JS pending has no reason or picks; Start and 1 act the same.
            let pending = match m.choice.kind {
                ChoiceKind::Gift => Pending::Gift { options: Gift::ALL.to_vec(), picks: 1 },
                ChoiceKind::Cell => Pending::Cell { id: SEWER_RATWAY, options: (0..d.cell_count).filter(|&n| n != d.room).collect() },
                ChoiceKind::Interrogation => Pending::Interrogation { options: Card::ALL.to_vec() },
                // The pick comes before the mark: the targets as the Manage phase has them.
                ChoiceKind::Prey => Pending::Prey { options: hunt_targets(&st) },
                ChoiceKind::Tool => {
                    let mut options: Vec<Tool> = Vec::new();
                    for &t in &m.choice.options {
                        if !options.contains(&t) && !st.tools.contains(&t) {
                            options.push(t);
                        }
                    }
                    Pending::Tool { options, reason: ToolReason::Start }
                }
            };
            if pending.is_empty() {
                problems.push(if m.choice.kind == ChoiceKind::Prey {
                    HUNT_NEEDS.to_string()
                } else {
                    format!("Pick the {}s on offer.", d.instrument_name)
                });
            }
            st.pending = vec![pending];
        }
        Step::Manage => st.used = used_today,
    }
    (st, problems)
}

fn set_event(cell: &mut Cell, event: &Option<EventMark>) {
    if let Some(e) = event {
        cell.event = Some(if *e == EventMark::Execution { EXECUTION_EFFICIENCY } else { CHAIN_REACTION });
    }
}

fn make_vault(next_id: &mut u32, beast: Beast) -> VaultEntry {
    let id = *next_id;
    *next_id += 1;
    VaultEntry { id, beast, day: 1 }
}
