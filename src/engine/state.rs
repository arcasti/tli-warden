//! The game state, shaped like the JS state (js/engine.js newGame) so that
//! `serde_json::to_value` diffs cleanly against a JS state without its log.

use super::data::{Beast, Card, Gift, LOOT, RAINBOW, Tool, VAULT_CAP, data};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The defaults of normalizeParams (DEFAULT_PARAMS); Lab editing is not ported.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Params {
    pub drill_days: u32,
    pub destinations: u32,
    pub fewer_doors_chance: u32,
    pub swaps_per_day: u32,
    pub reusable_cells: bool,
    pub tier_weights: [u32; RAINBOW as usize],
    pub beast_weights: [u32; 4],
    pub incident_spawn_chance: u32,
    pub execution_rerolls: bool,
    pub ivory_count: u32,
    pub ivory_includes_swapped: bool,
    pub prey_rounds: u32,
    pub scratch_stacks_base: bool,
    pub nest_days: u32,
    pub nest_tier: i32,
    pub rewards: bool,
    pub start_tools: u32,
    pub objective: u32,
    pub theme: i32,
    /// Chromatics the vault holds; once full, inmates stop at Ultimate. None
    /// means no cap, as in the JS engine (its traces deserialize to None).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault_cap: Option<u32>,
    /// Whether the Severed Iron Maiden's rise applies. The JS engine has no
    /// such field, so a state that keeps the rise leaves it out.
    #[serde(default = "rises", skip_serializing_if = "bool::clone")]
    pub maiden: bool,
    /// Whether a marker's incident stays unseen until it fires, as in the
    /// game; the JS engine shows it, and its traces leave the field out.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden_incidents: bool,
}

fn rises() -> bool {
    true
}

fn is_false(b: &bool) -> bool {
    !b
}

impl Default for Params {
    fn default() -> Self {
        let d = data();
        Params {
            drill_days: d.days,
            destinations: d.dice_options,
            fewer_doors_chance: 0,
            swaps_per_day: 1,
            reusable_cells: true,
            tier_weights: d.tier_weights,
            beast_weights: [25; 4],
            incident_spawn_chance: d.incident_spawn_chance,
            execution_rerolls: false,
            ivory_count: 4,
            ivory_includes_swapped: true,
            prey_rounds: 3,
            scratch_stacks_base: true,
            nest_days: 2,
            nest_tier: 2,
            rewards: true,
            start_tools: 0,
            objective: 0,
            theme: 0,
            vault_cap: Some(VAULT_CAP),
            maiden: true,
            hidden_incidents: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Patrol,
    Manage,
    Event,
    Ended,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inmate {
    pub id: u32,
    pub beast: Beast,
    pub tier: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Nest {
    pub due: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    pub n: usize,
    pub sealed: bool,
    pub inmate: Option<Inmate>,
    pub hidden: Option<Inmate>,
    pub known: u8,
    pub scratch: bool,
    pub nest: Option<Nest>,
    pub event: Option<u32>,
    pub ivory: bool,
    /// False when the player sees a marker but not its incident (set by the
    /// board reader, never by the engine); None leaves it out of the JSON.
    #[serde(rename = "eventKnown", default, skip_serializing_if = "Option::is_none")]
    pub event_known: Option<bool>,
}

impl Cell {
    /// blankCell: the office is never sealed and never holds anyone.
    pub fn blank(n: usize) -> Cell {
        Cell { n, sealed: n != data().room, inmate: None, hidden: None, known: 0, scratch: false, nest: None, event: None, ivory: false, event_known: None }
    }

    /// An open cell with an inmate in it.
    pub fn on_board(&self) -> bool {
        !self.sealed && self.inmate.is_some()
    }
}

/// The incident waiting for its confirm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRef {
    pub id: u32,
    pub cell: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultEntry {
    pub id: u32,
    pub beast: Beast,
    pub day: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolReason {
    Start,
    Objective,
}

/// A choice the player makes before anything else.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Pending {
    Tool { options: Vec<Tool>, reason: ToolReason },
    Gift { options: Vec<Gift>, picks: u32 },
    Interrogation { options: Vec<Card> },
    /// A chain consume: the eater in cell `from` may bite one of `options`.
    Chain {
        from: usize,
        #[serde(rename = "eaterId")]
        eater_id: u32,
        options: Vec<usize>,
    },
    /// A cell to move to, for the incident `id` (Sewer Ratway).
    Cell { id: u32, options: Vec<usize> },
    /// The Hunt's "Select an Objective": the Lion's Consumable targets, one
    /// to mark as prey. Only a board read on that screen sets it.
    Prey { options: Vec<usize> },
}

impl Pending {
    pub fn is_empty(&self) -> bool {
        match self {
            Pending::Tool { options, .. } => options.is_empty(),
            Pending::Gift { options, .. } => options.is_empty(),
            Pending::Interrogation { options } => options.is_empty(),
            Pending::Chain { options, .. } | Pending::Cell { options, .. } | Pending::Prey { options } => options.is_empty(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectiveState {
    pub id: u32,
    pub progress: u32,
    pub done: bool,
}

/// Actions used today (freshDay).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Used {
    pub swap: u32,
    pub devour: u32,
    pub hunt: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreyLink {
    pub lion: u32,
    pub prey: u32,
    pub prey_beast: Beast,
    pub rounds: i32,
    pub next: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    pub patrols: u32,
    pub opened: u32,
    pub devours: u32,
    pub chains: u32,
    pub swaps: u32,
    pub executions: u32,
    pub best: i32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RosterRow {
    pub beast: Beast,
    pub tier: i32,
    #[serde(rename = "where")]
    pub place: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Roster {
    pub rows: Vec<RosterRow>,
    pub by_tier: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunResult {
    pub day: u32,
    pub early: bool,
    pub roster: Roster,
    pub cache: u32,
    pub best: i32,
    pub stats: Stats,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub v: u32,
    pub scenario: String,
    pub seed: String,
    pub rng: u32,
    pub params: Params,
    pub day: u32,
    pub max_days: u32,
    pub phase: Phase,
    pub position: usize,
    pub dice: Vec<u32>,
    pub destinations: Vec<usize>,
    pub chain: bool,
    pub event: Option<EventRef>,
    /// Markers placed per incident id; JSON keys are the ids as strings.
    pub incident_counts: BTreeMap<u32, u32>,
    /// Draw weights the run changed, by incident id (Usurpation stages).
    pub incident_weights: BTreeMap<u32, u32>,
    pub cells: Vec<Cell>,
    pub vault: Vec<VaultEntry>,
    pub cache: u32,
    pub tools: Vec<Tool>,
    pub pending: Vec<Pending>,
    pub objective: Option<ObjectiveState>,
    pub theme: Option<u32>,
    pub used: Used,
    pub prey: Vec<PreyLink>,
    pub next_id: u32,
    pub stats: Stats,
    pub result: Option<RunResult>,
}

impl State {
    pub fn has(&self, tool: Tool) -> bool {
        self.tools.contains(&tool)
    }

    /// The highest tier an inmate can reach: Chromatic while the vault has
    /// room, Ultimate once it is full.
    pub fn top_tier(&self) -> i32 {
        if self.params.vault_cap.is_none_or(|cap| self.vault.len() < cap as usize) { RAINBOW } else { RAINBOW - 1 }
    }

    /// The Riot loot of this board in Chromatics: each vaulted Chromatic plus
    /// the cache of each inmate on an open cell, weighted by `LOOT`.
    pub fn loot(&self) -> f64 {
        let board: f64 = self.cells.iter().filter(|c| c.on_board()).filter_map(|c| c.inmate).map(|i| LOOT[i.tier as usize]).sum();
        self.vault.len() as f64 * LOOT[RAINBOW as usize] + board
    }

    /// Index of the cell holding inmate `id`, sealed or open (cellOf).
    pub fn cell_of(&self, id: u32) -> Option<usize> {
        self.cells.iter().position(|c| c.inmate.is_some_and(|i| i.id == id))
    }

    /// cellOf, only when the cell is open (boardCellOf).
    pub fn board_cell_of(&self, id: u32) -> Option<usize> {
        self.cell_of(id).filter(|&n| !self.cells[n].sealed)
    }

    /// Index in `prey` of the first mark inmate `id` is part of (linkOf).
    pub fn link_of(&self, id: u32) -> Option<usize> {
        self.prey.iter().position(|l| l.lion == id || l.prey == id)
    }

    /// Open cells with an inmate (boardCells), as indices.
    pub fn board_cells(&self) -> Vec<usize> {
        self.cells.iter().filter(|c| c.on_board()).map(|c| c.n).collect()
    }

    pub fn sealed_cells(&self) -> Vec<usize> {
        self.cells.iter().filter(|c| c.sealed).map(|c| c.n).collect()
    }
}
