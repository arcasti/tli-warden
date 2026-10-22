//! A board model from a read S16 screen: the scene gives the cells and the
//! warden, the panels give the step, the day, the doors and the vault. Every
//! low-confidence read and every part the screen does not show becomes a
//! warning for the player to check.

use super::panels::{Panels, Screen};
use super::scene::{CELLS, Scene};
use crate::board::{CellModel, ChoiceKind, EventMark, Model, ObjectiveModel, PreyMark, Step};
use crate::engine::data::{Beast, IRON_LAW, Tool, data};
use crate::engine::devour::eats;
use crate::engine::state::Params;
use crate::engine::text::{cell_label, tier_name};

/// Length of an S16 run: the counter starts at 20 days until Riot, except
/// under the Iron Law theme, which extends the run (see [`iron_law_days`]).
pub const RUN_DAYS: i64 = 20;

/// Length of an Iron Law run.
pub fn iron_law_days() -> i64 {
    data().theme(IRON_LAW).and_then(|t| t.value).unwrap_or(RUN_DAYS)
}
/// Padlock, beast, incident and warden reads below this confidence are flagged.
const CONF: f32 = 0.3;
/// Tier reads below this confidence are flagged (tests/reader.rs measures it).
pub const TIER_CONF: f32 = 0.4;

/// The model index of a screen cell number (1 = the Interrogation Room).
fn index(cell: u8) -> usize {
    cell as usize - 1
}

/// Something the reader asks the player to check before trusting the pick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Warning {
    /// The model index of the cell it is about, if any (the warden's cell for a warden read).
    pub cell: Option<usize>,
    pub kind: WarningKind,
    /// The sentence for the command line.
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WarningKind {
    /// A low-confidence or missing read, or a screen the solver can't use as is.
    Read,
    /// The held instruments show as icons only, so the player has to name them.
    HeldToolsUnknown,
}

impl Warning {
    fn read(text: String) -> Warning {
        Warning { cell: None, kind: WarningKind::Read, text }
    }

    fn cell(cell: usize, text: String) -> Warning {
        Warning { cell: Some(cell), kind: WarningKind::Read, text }
    }

    fn held_tools_unknown(text: String) -> Warning {
        Warning { cell: None, kind: WarningKind::HeldToolsUnknown, text }
    }
}

impl std::fmt::Display for Warning {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

/// `under` is the last board read before this frame: an instrument or treasure
/// pick covers the board, so the cells, the warden and the day come from it, or
/// from the run start (every cell locked) without one. On a board frame it
/// gives the cells a tooltip panel covers. `run_days` is the run's length.
pub fn to_model(scene: &Scene, panels: &Panels, held_tools: &[Tool], under: Option<&Model>, run_days: i64) -> (Model, Vec<Warning>) {
    let d = data();
    let mut warn = Vec::new();
    let covered = matches!(panels.screen, Screen::InstrumentPick | Screen::TreasurePick);
    let mut m = match under.filter(|_| covered) {
        Some(board) => {
            warn.push(Warning::read(format!("The pick covers the board: the cells are from the last board read (day {}).", board.day)));
            board.clone()
        }
        None if covered => {
            warn.push(Warning::read("The pick covers the board and no board was read before it: the run start is assumed (every cell locked). For a pick later in the run, read the board first.".to_string()));
            Model::blank()
        }
        None => Model::blank(),
    };
    m.max_days = run_days;
    m.dest.clear();
    if !covered {
        read_board(scene, under, &mut m, &mut warn);
    }

    match panels.days {
        Some(left) => m.day = (m.max_days - i64::from(left)).max(1),
        None if covered && under.is_some() => {}
        None => warn.push(Warning::read("The days counter did not read: day 1 assumed.".to_string())),
    }

    match panels.screen {
        Screen::Patrol => {
            m.step = Step::Patrol;
            for (k, door) in panels.doors.iter().enumerate() {
                match door {
                    Some(c) => m.dest.push(index(*c)),
                    None => warn.push(Warning::read(format!("Card {} did not read: check today's doors.", k + 1))),
                }
            }
            if panels.doors.is_empty() {
                warn.push(Warning::read("No destination card read: check today's doors.".to_string()));
            }
        }
        Screen::Manage => {
            m.step = Step::Manage;
            m.devour_used = !panels.decision.contains(&"Consume");
            m.swap_used = !panels.decision.contains(&"Replace");
            m.hunt_used = !panels.decision.contains(&"Hunt");
        }
        Screen::InstrumentPick => {
            m.step = Step::Choice;
            m.choice.kind = ChoiceKind::Tool;
            m.choice.options = panels.instruments.clone();
            m.choice.rerolls = panels.rerolls;
            if panels.instruments.len() != 3 {
                warn.push(Warning::read(format!("Read {} of the 3 {}s on offer.", panels.instruments.len(), d.instrument_name)));
            }
        }
        Screen::TreasurePick => {
            m.step = Step::Choice;
            m.choice.kind = ChoiceKind::Gift;
            m.choice.options.clear();
        }
        // The Hunt's target pick, played with the Blood-Stained Trap from a
        // Lion's cell, else the Sewer Ratway's cell pick.
        Screen::CellPick => {
            m.step = Step::Choice;
            let lion = usize::try_from(m.warden).ok().and_then(|n| m.cells.get(n)).is_some_and(|c| matches!(c, CellModel::Inmate { beast: Beast::Lion, .. }));
            m.choice.kind = if lion && held_tools.contains(&Tool::Trap) { ChoiceKind::Prey } else { ChoiceKind::Cell };
            m.choice.options.clear();
        }
        Screen::Interrogation => {
            m.step = Step::Choice;
            m.choice.kind = ChoiceKind::Interrogation;
            m.choice.options.clear();
        }
        // The session solves a Consume target pick from the manage board before it.
        Screen::ChainTarget => m.step = Step::Manage,
        // The board under a pick, with no decision on screen: the treasure
        // pick follows the day's consume, so it reads as the manage step with
        // nothing left but to end the day.
        Screen::PrisonStatus => {
            m.step = Step::Manage;
            m.devour_used = true;
            m.swap_used = true;
            m.hunt_used = true;
        }
        Screen::DayBanner => {
            m.step = Step::Manage;
            warn.push(Warning::read("This screen (the day banner) can't be solved: the board is read as the manage step.".to_string()));
        }
        Screen::Popup => {
            m.step = Step::Patrol;
            warn.push(Warning::read("A game dialog covers the board: close it and read again.".to_string()));
        }
        Screen::Theme => {
            m.step = Step::Patrol;
            let name = d.theme_name(panels.theme).unwrap_or("unknown");
            warn.push(Warning::read(format!("The run's theme: {name}. Nothing to solve on this screen.")));
        }
        Screen::RunEnd => {
            m.step = Step::Patrol;
            warn.push(Warning::read("The riot screen: the run is over, nothing to solve.".to_string()));
        }
        Screen::Unknown => {
            m.step = Step::Patrol;
            warn.push(Warning::read("No destination cards or decision buttons on screen: take the screenshot when they show.".to_string()));
        }
    }

    if !panels.vault.is_empty() {
        m.vault.clear();
    }
    for (k, slot) in panels.vault.iter().enumerate() {
        match slot.beast.filter(|_| slot.conf >= CONF) {
            Some(beast) => m.vault.push(beast),
            None => {
                m.vault.push(Beast::Mammoth);
                warn.push(Warning::read(format!("Chromatic Prisoner {}: beast not read, Mammoth assumed.", k + 1)));
            }
        }
    }

    // A pick, a tooltip or a dialog can hide the objective panel: the last
    // board's objective stands until the panel reads again.
    m.objective = panels.objective.map(|(id, progress)| ObjectiveModel { id, progress }).or(under.and_then(|b| b.objective));

    m.tools = held_tools.to_vec();
    if held_tools.is_empty() && panels.tool_icons && panels.held.is_none() {
        warn.push(Warning::held_tools_unknown(format!("The {} panel shows icons the reader can't name: pass them with --tools.", d.instrument_name)));
    }
    (m, warn)
}

/// The cells and the warden from the board scene.
fn read_board(scene: &Scene, under: Option<&Model>, m: &mut Model, warn: &mut Vec<Warning>) {
    let d = data();
    let mut kept = Vec::new();
    for c in CELLS {
        let r = scene.cell(c);
        let n = index(c);
        let at = cell_label(n);
        if r.covered {
            if let Some(board) = under {
                m.cells[n] = board.cells[n].clone();
                kept.push(at);
                continue;
            }
            warn.push(Warning::cell(n, format!("Check cell {at}: a tooltip covers it.")));
        }
        if r.locked_conf < CONF {
            warn.push(Warning::cell(n, format!("Check cell {at}: read as {} (confidence {:.2}).", if r.locked { "locked" } else { "open" }, r.locked_conf)));
        }
        if r.event && r.event_conf < CONF {
            warn.push(Warning::cell(n, format!("Check cell {at}: {} marker read with confidence {:.2}.", d.incident_term, r.event_conf)));
        }
        let event = r.event.then_some(EventMark::Unknown);
        m.cells[n] = if r.locked {
            match r.glow {
                Some(tier) if r.glow_conf >= TIER_CONF => CellModel::Scratch { tier: i64::from(tier), event },
                Some(tier) => {
                    warn.push(Warning::cell(n, format!("Check cell {at}: the glow behind the locked door read as {} (confidence {:.2}).", tier_name(i32::from(tier)), r.glow_conf)));
                    CellModel::Sealed { event }
                }
                None => CellModel::Sealed { event },
            }
        } else {
            match r.inmate {
                Some((beast, tier)) => {
                    let name = format!("{} {}", tier_name(i32::from(tier)), d.beast_names[beast.index()]);
                    if r.beast_conf < CONF {
                        warn.push(Warning::cell(n, format!("Check cell {at}: beast read as {name} (confidence {:.2}).", r.beast_conf)));
                    } else if r.tier_conf < TIER_CONF {
                        warn.push(Warning::cell(n, format!("Check cell {at}: rarity read as {name} (confidence {:.2}).", r.tier_conf)));
                    }
                    CellModel::Inmate { beast, tier: i64::from(tier), ivory: r.tusk, prey: None, event }
                }
                None => {
                    if r.beast_conf < CONF {
                        warn.push(Warning::cell(n, format!("Check cell {at}: read as an empty cell (confidence {:.2}).", r.beast_conf)));
                    }
                    CellModel::Empty { event }
                }
            }
        };
    }
    if let Some((last, rest)) = kept.split_last() {
        warn.push(Warning::read(match rest {
            [] => format!("A tooltip covers cell {last}: it is kept from the last board read."),
            _ => format!("A tooltip covers cells {} and {last}: they are kept from the last board read.", rest.join(", ")),
        }));
    }

    match scene.warden {
        Some(c) => {
            m.warden = index(c) as i64;
            if scene.warden_conf < CONF {
                warn.push(Warning::cell(index(c), format!("Check the warden: read at cell {} (confidence {:.2}).", cell_label(index(c)), scene.warden_conf)));
            }
        }
        None => warn.push(Warning::read("The warden's arrow was not found: the warden is put in the Interrogation Room.".to_string())),
    }

    // A Prey crosshair is drawn on the prey only: it goes to a Lion of the
    // same rarity that eats the prey, the warden's first (the Hunt is played
    // from the Lion's cell), else the lowest cell. A Lion holds one mark.
    for c in CELLS.filter(|&c| scene.cell(c).prey && !scene.cell(c).covered) {
        let n = index(c);
        let CellModel::Inmate { beast: meal, tier, .. } = m.cells[n] else { continue };
        let free = |l: usize| {
            l != n && matches!(&m.cells[l], CellModel::Inmate { beast: Beast::Lion, tier: t, prey: None, .. } if *t == tier && eats(Beast::Lion).contains(&meal))
        };
        let lion = usize::try_from(m.warden).ok().filter(|&l| l < m.cells.len() && free(l)).or_else(|| (0..m.cells.len()).find(|&l| free(l)));
        match lion {
            Some(l) => {
                if let CellModel::Inmate { prey, .. } = &mut m.cells[l] {
                    *prey = Some(PreyMark { cell: Some(n as i64), rounds: i64::from(Params::default().prey_rounds) });
                }
            }
            None => warn.push(Warning::cell(n, format!("Check cell {}: a Prey mark read with no {} to link it to.", cell_label(n), d.beast_names[Beast::Lion.index()]))),
        }
    }
}
