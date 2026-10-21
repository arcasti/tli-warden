//! The app's state without the UI: the reading of the last frame, the
//! player's corrections and held instruments, and the ranking, which a
//! worker thread fills in rounds so the window shows it as it firms up.

use crate::board::{CellModel, ChoiceKind, EventMark, Model, Step, build_state};
use crate::engine::data::{Beast, Gift, IRON_LAW, RAINBOW, Tool, VAULT_CAP, data};
use crate::engine::rules::treasure_picks;
use crate::engine::text::{cell_label, tier_name, tool_name};
use crate::reader::model::{RUN_DAYS, Warning, WarningKind, iron_law_days, to_model};
use crate::reader::panels::{Panels, Screen, read_panels_titled};
use crate::reader::scene::{CELLS, Scene, read_scene};
use crate::reader::layout::{is_16_9, lay_out_scaled};
use crate::reader::{REF_H, REF_W, Rgba};
use crate::solver::TUNED;
use super::runlog::{RunLog, timestamp};
use crate::solver::advise::{Advice, Pick, Shown, advise, also_good, shown};
use serde_json::json;
use crate::engine::{Action, ChoiceId};
use crate::engine::state::{Pending, State};
use crate::solver::solve::{Rollouts, SolveOption, instant, instant_bite};
use crate::engine::devour::{eats, offer_chain, start_options};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::SystemTime;

/// Rounds the worker plays between two partial rankings.
pub const ROUND: usize = 50;
/// The seed of the board's hidden parts and of the playouts, as in `warden-cli solve`.
const SEED: u32 = 1;

/// A correction to a cell the reader got wrong.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CellEdit {
    Locked,
    Empty,
    Inmate { beast: Beast, tier: i64 },
}

/// What the reader made of a frame, with the player's corrections applied.
#[derive(Clone, Debug)]
pub struct Reading {
    pub model: Model,
    pub warnings: Vec<Warning>,
    /// The screen the frame shows.
    pub screen: Screen,
    /// "N days until Riot", read or kept from an earlier reading (see [`DaysClock`]).
    pub days_left: Option<u32>,
    /// When the frame was read (corrections keep it).
    pub at: SystemTime,
}

impl Reading {
    /// The lines saying why the frames after this reading were not read
    /// (a popup, a screen without the board, a window narrower than 16:9, the
    /// theme scroll): empty unless the reading on show is a kept one. The
    /// next reading installed starts without them.
    pub fn kept_lines(&self) -> Vec<&str> {
        self.warnings.iter().filter(|w| kept_line(&w.text)).map(|w| w.text.as_str()).collect()
    }
}

/// Whether a warning is a line [`Session::take_read`] puts over a kept reading.
fn kept_line(text: &str) -> bool {
    text == POPUP || text == OFF_BOARD || text == FADING || text.starts_with(THEME_READ) || text.starts_with(SHAPE)
}

/// The days left until the Riot over a run's readings. The counter only
/// counts down, so a read equal to or lower than the last count is taken, a
/// missing read keeps the last count, and a higher read is a misread unless
/// the same value reads twice in a row (a new run).
#[derive(Clone, Copy, Debug, Default)]
pub struct DaysClock {
    left: Option<u32>,
    /// A higher read waiting for its second reading.
    higher: Option<u32>,
}

impl DaysClock {
    /// Takes one reading's days read and gives the count to use: None
    /// until a first read.
    pub fn update(&mut self, read: Option<u32>) -> Option<u32> {
        let higher = self.higher.take();
        match (read, self.left) {
            (Some(r), Some(left)) if r > left && higher != Some(r) => self.higher = Some(r),
            (Some(r), _) => self.left = Some(r),
            (None, _) => {}
        }
        self.left
    }

    /// [`DaysClock::update`], and whether it took a higher count (a new run).
    fn update_run(&mut self, read: Option<u32>) -> (Option<u32>, bool) {
        let before = self.left;
        let left = self.update(read);
        (left, matches!((before, left), (Some(b), Some(l)) if l > b))
    }
}

/// Whether `screen` is a pick that covers the board with no board read under
/// it (`under`): the run start is assumed.
fn run_start(screen: Screen, under: Option<&Model>) -> bool {
    matches!(screen, Screen::InstrumentPick | Screen::TreasurePick) && under.is_none()
}

/// The warning for a days count kept from an earlier reading, when `read`
/// is not the count in use.
fn kept_days(read: Option<u32>, left: Option<u32>) -> Option<Warning> {
    let left = left.filter(|&n| read != Some(n))?;
    let text = match read {
        Some(r) => format!("The days counter read {r}, more than the last count: {left} day(s) until Riot kept until {r} reads again."),
        None => format!("The days counter is hidden: {left} day(s) until Riot kept from the last read."),
    };
    Some(Warning { cell: None, kind: WarningKind::Read, text })
}

/// The board under a treasure pick after a consume, and the eater's model
/// index. The day's consume starts in the warden's cell and chain bites keep
/// the eater there; at Chromatic it leaves for the vault and its cell locks
/// again, as every consumed cell does, the last meal's cell (`meal`) too.
/// None when the warden's cell holds no inmate: the board read already shows
/// it gone. With no Consume target screen read since the board (`meal` None)
/// and the Ravenous Chains held, the Chromatic comes from the Chains at the
/// start of a day instead when they can lift the board's top inmate that far;
/// the removed neighbours are the returned list.
fn vaulted(board: &Model, meal: Option<Meal>, chains: bool) -> Option<(Model, usize, Vec<usize>)> {
    if meal.is_none() && chains && let Some(v) = chains_vaulted(board) {
        return Some(v);
    }
    let n = usize::try_from(board.warden).ok()?;
    let &CellModel::Inmate { beast, event, .. } = board.cells.get(n)? else { return None };
    let mut m = board.clone();
    m.cells[n] = CellModel::Sealed { event };
    m.vault.push(beast);
    if let Some(meal) = meal
        && let Some(&CellModel::Inmate { event, .. }) = m.cells.get(meal.cell)
    {
        m.cells[meal.cell] = CellModel::Sealed { event };
    }
    Some((m, n, Vec::new()))
}

/// The board after the Ravenous Chains sent the board's top inmate to the
/// vault: it and its neighbours on the ring lock. None when the top rarity is
/// shared (the Chains pick one at random) or the Chains' tiers, one roll per
/// neighbour, can't lift it to Chromatic.
fn chains_vaulted(board: &Model) -> Option<(Model, usize, Vec<usize>)> {
    if board.vault.len() >= VAULT_CAP as usize {
        return None;
    }
    let inmate = |c: &CellModel| match *c {
        CellModel::Inmate { beast, tier, .. } => Some((tier, beast)),
        _ => None,
    };
    let top = board.cells.iter().filter_map(|c| inmate(c).map(|(t, _)| t)).max()?;
    let [(n, beast)] = board.cells.iter().enumerate().filter_map(|(n, c)| inmate(c).filter(|&(t, _)| t == top).map(|(_, b)| (n, b))).collect::<Vec<_>>()[..] else {
        return None;
    };
    let len = board.cells.len();
    let sides: Vec<usize> = [n + len - 1, n + 1].map(|k| k % len).into_iter().filter(|&k| inmate(&board.cells[k]).is_some()).collect();
    let gain = data().tool(Tool::Chains).tiers.unwrap_or(0) * sides.len() as i32;
    if top + i64::from(gain) < i64::from(RAINBOW) {
        return None;
    }
    let mut m = board.clone();
    for &k in sides.iter().chain([&n]) {
        let event = m.cells[k].event();
        m.cells[k] = CellModel::Sealed { event };
    }
    m.vault.push(beast);
    Some((m, n, sides))
}

/// The meal picked on the last Consume target screen read: the only target
/// the screen offered (certain), or the one Warden advised there.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Meal {
    cell: usize,
    certain: bool,
}

/// The cells, as model indices, of the prey the board's Lions have marked.
fn prey_cells(m: &Model) -> impl Iterator<Item = usize> + '_ {
    m.cells.iter().filter_map(|c| match c {
        CellModel::Inmate { prey: Some(mark), .. } => mark.cell.and_then(|x| usize::try_from(x).ok()),
        _ => None,
    })
}

/// True when a Consume target screen read shows a bite since `before`: the
/// inmate in the warden's cell changed, or another inmate left its cell. The
/// screen then picks a chain's next meal, not the day's first.
fn bitten(before: &Model, read: &Model) -> bool {
    let eater = usize::try_from(read.warden).ok();
    before.cells.iter().zip(&read.cells).enumerate().any(|(n, pair)| match pair {
        (CellModel::Inmate { beast, tier, .. }, CellModel::Inmate { beast: b, tier: t, .. }) => {
            Some(n) == eater && (beast, tier) != (b, t)
        }
        (CellModel::Inmate { .. }, CellModel::Empty { .. } | CellModel::Sealed { .. }) => true,
        _ => false,
    })
}

/// A low-confidence cell read stops warning once it gave the same read on
/// this many reads in a row before the one on show.
const STEADY_READS: usize = 2;

/// The key of a low-confidence cell warning: its text without the
/// confidence number, so the same read on the same cell gives the same key.
fn steady_key(w: &Warning) -> Option<String> {
    let (text, _) = w.text.split_once("confidence ")?;
    text.starts_with(CHECK_CELL).then(|| text.to_string())
}

/// How the reader's warnings about one cell's read start.
const CHECK_CELL: &str = "Check cell ";

/// How the warning about Consume targets the board read does not allow starts.
const TARGETS_DIFFER: &str = "The Consume targets on screen are ";

/// The targets of a Consume target pick: the engine's `options` on the read
/// board that the screen marks with big jaws (`jawed`, model indices). When
/// the two differ, a warning on the eater's cell; when none of the jawed
/// cells is among the options, or no jaws were read, the options as they are.
pub fn consume_targets(options: &[usize], jawed: &[usize], eater: usize) -> (Vec<usize>, Option<Warning>) {
    if jawed.is_empty() || (jawed.len() == options.len() && options.iter().all(|n| jawed.contains(n))) {
        return (options.to_vec(), None);
    }
    let cells = |list: &[usize]| {
        let labels: Vec<String> = list.iter().map(|&n| cell_label(n)).collect();
        match &labels[..] {
            [] => "none".to_string(),
            [one] => format!("cell {one}"),
            [rest @ .., last] => format!("cells {} and {last}", rest.join(", ")),
        }
    };
    let text = format!(
        "{TARGETS_DIFFER}{} and the board read allows {}: check the eater in cell {} and those cells.",
        cells(jawed),
        cells(options),
        cell_label(eater),
    );
    let both: Vec<usize> = options.iter().copied().filter(|n| jawed.contains(n)).collect();
    let targets = if both.is_empty() { options.to_vec() } else { both };
    (targets, Some(Warning { cell: Some(eater), kind: WarningKind::Read, text }))
}

/// The cells a Consume target screen marks with big jaws, as model indices.
fn jawed(scene: &Scene) -> Vec<usize> {
    CELLS.filter(|&c| scene.cell(c).target).map(|c| c as usize - 1).collect()
}

/// Avenger's Casket opens a locked door after a bite with a meal for the
/// eater, at its rarity, and the target screen puts the big jaws on it, which
/// spoil its read. A jawed cell (`jawed`, model indices) that `m` holds
/// without an inmate becomes that meal: the eater's rarity and the beast
/// `read` shows when the eater eats it, else what the eater eats, with a
/// warning on the cell when that is more than one beast.
pub fn casket_meals(m: &mut Model, read: &Model, jawed: &[usize]) -> Vec<Warning> {
    let eater = usize::try_from(m.warden).ok().and_then(|w| m.cells.get(w));
    let Some(&CellModel::Inmate { beast: eater, tier, .. }) = eater else {
        return Vec::new();
    };
    let food = eats(eater);
    let mut warnings = Vec::new();
    for &n in jawed {
        if matches!(m.cells[n], CellModel::Inmate { .. }) {
            continue;
        }
        let seen = match read.cells[n] {
            CellModel::Inmate { beast, .. } => Some(beast).filter(|b| food.contains(b)),
            _ => None,
        };
        let beast = seen.unwrap_or(food[0]);
        if seen.is_none() && food.len() > 1 {
            let text = format!(
                "{CHECK_CELL}{}: a new Inmate from {}, taken as {} {}: click the cell to correct it.",
                cell_label(n),
                tool_name(Tool::Revenge.key()),
                tier_name(tier as i32),
                data().beast_names[beast.index()],
            );
            warnings.push(Warning { cell: Some(n), kind: WarningKind::Read, text });
        }
        m.cells[n] = CellModel::Inmate { beast, tier, ivory: m.cells[n].ivory(), prey: None, event: m.cells[n].event() };
    }
    warnings
}

/// The line shown over the last reading while a game dialog covers the board.
const POPUP: &str = "A popup covers the game: showing the last reading.";
/// The line shown over the last reading while the board is not on screen.
const OFF_BOARD: &str = "The prison board is not on screen: showing the last reading.";
/// The line shown over the last reading while the board fades in after a Consume target screen.
const FADING: &str = "The board is still coming in: showing the last reading.";
/// How the line shown over the last reading after the run theme scroll starts.
const THEME_READ: &str = "The run's theme: ";
/// The line for a read kept between runs, see [`Session::install`].
const RUN_OVER_WAIT: &str = "The run is over: waiting for the next run.";

/// Cells in which a board read with a hidden counter on the run's last day
/// must differ from the board on show to be taken as a screen after the run
/// (the riot screen missed). In the cloud run logs such reads differed from
/// the last day's board in 6 to 14 of 16 cells; 43 of about 16,000 same-day
/// read pairs in normal play differed in 6 or more.
const AFTER_RUN_CELLS: usize = 6;

/// What the player sees of a cell for [`AFTER_RUN_CELLS`]: locked or not,
/// the beast and the tier.
fn cell_sight(cell: &CellModel) -> (bool, Option<Beast>, Option<i64>) {
    match cell {
        CellModel::Sealed { .. } => (true, None, None),
        CellModel::Inmate { beast, tier, .. } | CellModel::Scouted { beast, tier, .. } => (false, Some(*beast), Some(*tier)),
        CellModel::Scratch { tier, .. } => (false, None, Some(*tier)),
        _ => (false, None, None),
    }
}

/// Playouts per option before the playouts can overrule the strategy's pick.
const LIVE_MIN: usize = 150;

/// The instruments' keys, as the settings keep them.
fn tool_keys(tools: &[Tool]) -> Vec<&'static str> {
    tools.iter().map(|t| t.key()).collect()
}

/// What the reader saw in a frame, the frame's size, the frame itself
/// when the run log keeps it, and the interface scale it was read at.
type Scanned = (Scene, Panels, (usize, usize), Option<Rgba>, f32);

/// What [`Session::take_read`] did with a read.
enum Taken {
    /// A reading was installed.
    Installed,
    /// A theme scroll, a popup, a screen without the board or a read between
    /// runs: nothing was installed; the line says so.
    Kept(String),
    /// Nothing changed, or the riot screen ended the run.
    Skipped,
}

/// Reads the frame at the reference size the reader's regions are measured
/// on, as `load_png` does, at the interface scale measured near `hint`; see
/// [`shape_line`] for the shapes refused. `keep` gives the frame back at its
/// own size.
fn scan(frame: Rgba, keep: bool, hint: Option<f32>) -> Scanned {
    let size = (frame.w, frame.h);
    let (f, title, u) = lay_out_scaled(&frame, hint);
    let (scene, panels) = (read_scene(&f), read_panels_titled(&f, title.as_ref().unwrap_or(&f)));
    (scene, panels, size, keep.then_some(frame), u)
}

/// How the line for a game window narrower than 16:9 starts.
const SHAPE: &str = "The game window is ";

/// A window as wide as 16:9 or wider is read: [`lay_out_scaled`] lays it out as
/// the reference frame. A window narrower than 16:9 (by more than 1%) has
/// never been seen, so how the game lays it out is untested: such a frame is
/// not read.
fn shape_line((w, h): (usize, usize)) -> Option<String> {
    let narrow = w * 9 < h * 16 && !is_16_9(w, h);
    narrow.then(|| format!("{SHAPE}{w} x {h}, narrower than 16:9: that layout is untested and isn't read. Set the game to a 16:9 or wider size: fullscreen or borderless, or a window such as 1920 x 1080 that is not maximized."))
}

/// The fingerprint grid: 64 x 36 blocks, 30 x 30 pixels each on a 1920 x 1080 frame.
const PRINT: (usize, usize) = (64, 36);
/// A block changes when its mean gray moves by more than this.
const BLOCK_DIFF: u8 = 16;
/// Two fingerprints differ when at least this many blocks changed.
const CHANGED_BLOCKS: usize = 4;

/// A frame's mean gray per block of a 64 x 36 grid: small enough to compare
/// every second, and a block average sees a change to a digit that a
/// resize, which samples a few pixels per block, can miss.
pub fn fingerprint(frame: &Rgba) -> Vec<u8> {
    let (gw, gh) = PRINT;
    let (mut sum, mut count) = (vec![0u64; gw * gh], vec![0u64; gw * gh]);
    for y in 0..frame.h {
        let row = y * gh / frame.h.max(1) * gw;
        for x in 0..frame.w {
            let i = (y * frame.w + x) * 4;
            let p = &frame.data[i..i + 3];
            let b = row + x * gw / frame.w;
            sum[b] += (p[0] as u64 * 299 + p[1] as u64 * 587 + p[2] as u64 * 114) / 1000;
            count[b] += 1;
        }
    }
    sum.iter().zip(&count).map(|(&s, &n)| s.checked_div(n).unwrap_or(0) as u8).collect()
}

/// True when two fingerprints show different screens: other sizes, or at
/// least [`CHANGED_BLOCKS`] blocks whose gray moved by more than [`BLOCK_DIFF`].
/// A hover glow on one cell may count, and so may an animation: a re-read of
/// the same scene keeps the solve.
pub fn differs(a: &[u8], b: &[u8]) -> bool {
    a.len() != b.len() || changed(a, b) >= CHANGED_BLOCKS
}

/// The blocks whose gray moved by more than [`BLOCK_DIFF`] between two prints of one size.
fn changed(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).filter(|(x, y)| x.abs_diff(**y) > BLOCK_DIFF).count()
}

/// A sample holds when at most this many blocks moved since the one before.
/// Measured at 4 samples a second on a 2026-10-17 recording: the Interrogation
/// Room's animation moves 7 to 13 blocks a sample, a screen change 40 to 280.
pub const HELD_BLOCKS: usize = 16;

/// Samples in a row that have to hold before auto read reads: 2 at one sample
/// every 250 ms waits about 0.5 s for transitions to settle.
pub const SETTLE: usize = 2;

/// Samples in a row a screen may differ from the last one read without
/// settling before it is read anyway, about 1.5 s: on the last day the frame's
/// edges pulse by up to 90 blocks a sample and never hold.
pub const MAX_WAIT: usize = 6;

/// Samples since the last read after which a screen that moved fewer than
/// [`CHANGED_BLOCKS`] blocks from it, but at least one, is read again: 5 s at 4
/// samples a second. A Prison Incident "!" that appears on a cell moves only 2
/// blocks (measured on a player's screenshot), and a hover glow that comes and
/// goes costs at most one read per 5 s.
pub const RECHECK: usize = 20;

/// Auto read's decision, one fingerprint per sample of the game: read a screen
/// once it differs from the last one read and has settled, or has differed
/// for longer than [`MAX_WAIT`] samples; read a screen that moved less than
/// that, but by at least one block, once [`RECHECK`] samples passed since the
/// last read.
#[derive(Default)]
pub struct Watch {
    /// The print of the last screen read; none before the first.
    read: Option<Vec<u8>>,
    /// The previous sample's print.
    prev: Option<Vec<u8>>,
    /// Samples in a row that held.
    stable: usize,
    /// Samples in a row that differed from the last screen read.
    waited: usize,
    /// Samples since the last screen read.
    since: usize,
    /// The next sample counts as read: a screenshot is on show.
    adopt: bool,
}

impl Watch {
    /// Takes a sample's print; true when it should be read, which records it as read.
    pub fn sample(&mut self, print: Vec<u8>) -> bool {
        self.stable = match &self.prev {
            Some(p) if p.len() == print.len() && changed(p, &print) <= HELD_BLOCKS => self.stable + 1,
            _ => 0,
        };
        self.prev = Some(print.clone());
        if std::mem::take(&mut self.adopt) {
            self.mark_read(print);
            return false;
        }
        self.since += 1;
        let n = self.read.as_ref().filter(|r| r.len() == print.len()).map(|r| changed(r, &print));
        if let Some(n) = n.filter(|&n| n < CHANGED_BLOCKS) {
            self.waited = 0;
            if n > 0 && self.since >= RECHECK {
                self.mark_read(print);
                return true;
            }
            return false;
        }
        self.waited += 1;
        if self.stable >= SETTLE || self.waited > MAX_WAIT {
            self.mark_read(print);
            return true;
        }
        false
    }

    /// Records a screen read some other way (the read hotkey).
    pub fn mark_read(&mut self, print: Vec<u8>) {
        self.read = Some(print);
        self.waited = 0;
        self.since = 0;
    }

    /// Counts the next sample as read, so an opened screenshot stays on show
    /// until the game's screen changes.
    pub fn adopt_next(&mut self) {
        self.adopt = true;
    }
}

struct Progress {
    rounds: usize,
    done: bool,
    options: Vec<SolveOption>,
}

pub struct Session {
    pub reading: Option<Reading>,
    /// Set by the riot screen that ends a run: the line the window shows in
    /// place of the board and the advice, see [`Session::install`].
    pub run_over: Option<String>,
    /// Why the last read installed nothing while no reading is on show:
    /// a theme scroll, a popup or a screen without the board.
    pub missed: Option<String>,
    pub tools: Vec<Tool>,
    /// The run's theme, an id of `data().themes`: read from the lobby's
    /// scroll or set by the player; None for a run without one.
    pub theme: Option<u32>,
    pub pick: Option<Pick>,
    /// `build_state` objections: the model can't be played, nothing is solved.
    pub problems: Vec<String>,
    pub ranking: Vec<SolveOption>,
    /// Rounds behind `ranking`.
    pub rounds: usize,
    pub solving: bool,
    /// Playouts per option for a full solve.
    pub rollouts: usize,
    /// The state the running solve plays from.
    state: Option<State>,
    /// The stored read, its days replaced by the clock's count.
    scene: Option<(Scene, Panels, SystemTime)>,
    /// The size of the last frame read, before it was scaled.
    size: (usize, usize),
    /// The interface scale the last frame was read at (see
    /// [`crate::reader::layout::ui_scale`]): the next read searches near it.
    /// A game setting, so a new run keeps it.
    ui_scale: Option<f32>,
    /// The days counter as the stored read gave it.
    days_read: Option<u32>,
    days: DaysClock,
    /// How many instruments were held before the read whose higher count
    /// waits for its second reading: a new run clears these and keeps any
    /// picked since.
    held_before_higher: usize,
    /// The instrument advised on the instrument pick screen on show or last
    /// read: held from the next screen on, see [`Session::install`].
    advised_tool: Option<Tool>,
    /// The instrument panel's read when the player last set the held list
    /// (None inside: unread or unsure); None when the player has not set it.
    /// While reads give the same panel read, the player's list stands; the
    /// session's own changes to the list (an advised pick, a new run) clear it.
    held_edit: Option<Option<Vec<Tool>>>,
    /// The player set the held list on the pick on show: nothing is inferred
    /// from it.
    pick_named: bool,
    /// The last read was kept as the board fading in, see [`Session::take_read`].
    fade_kept: bool,
    /// The session's lines for the stored read (a held list it changed).
    notes: Vec<String>,
    /// The session changed `tools` or `theme`, see [`Session::take_tools_changed`].
    tools_changed: bool,
    /// `theme` was set (scroll or panel) after the last board read: a new
    /// run keeps it, see [`Session::install`].
    theme_fresh: bool,
    /// The picks taken from the Warden's Treasure open since its first pick
    /// screen; None when no treasure is open, see [`Session::install`].
    treasure: Option<u32>,
    /// The last model of a patrol, manage or Sewer Ratway cell pick screen, corrections included:
    /// what an instrument or treasure pick covers.
    board: Option<Model>,
    /// The board since the last manage screen, with the cells and the warden of
    /// any Consume target screen read after it (earlier bites locked): what a
    /// treasure pick after a consume covers, see [`vaulted`]. None after a
    /// patrol or cell pick screen.
    consume: Option<Model>,
    /// The longest vault read this run and the day it was read: a treasure
    /// pick reading fewer keeps it, and the run end counts it. A reading on
    /// an earlier day is a new run's.
    vault_top: Option<(i64, Vec<Beast>)>,
    /// The day each Prey mark on the board was first read, by the prey's
    /// cell: the reader reads every mark at its full rounds, and a mark loses
    /// one each day after. A reading on an earlier day is a new run's.
    prey_seen: Vec<(usize, i64)>,
    /// A Consume target screen read since the last manage screen showed a
    /// bite: the target screen is a chain's (see [`bitten`]).
    chained: bool,
    /// The meal picked on the last Consume target screen since the last
    /// patrol, manage or cell pick screen: what a treasure pick covers, see
    /// [`vaulted`].
    meal: Option<Meal>,
    /// Per low-confidence cell warning (see [`steady_key`]), the reads in a
    /// row up to the stored one that gave it.
    steady: BTreeMap<String, usize>,
    edits: BTreeMap<u8, CellEdit>,
    /// Per cell, an incident marker the player added (true) or removed.
    markers: BTreeMap<u8, bool>,
    rx: Option<Receiver<Progress>>,
    /// A frame the reader is working on, off the UI thread.
    scan: Option<Receiver<Scanned>>,
    /// The newest frame that arrived while `scan` ran; read when it ends.
    queued: Option<Rgba>,
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
    /// The run log the session writes its reads, advice and corrections to; none by default.
    pub log: Option<RunLog>,
}

impl Session {
    /// `days` is the days count the held `tools` were saved at: a higher
    /// count read twice is a new run and clears them.
    pub fn new(rollouts: usize, tools: Vec<Tool>, theme: Option<u32>, days: Option<u32>) -> Session {
        Session {
            reading: None,
            run_over: None,
            missed: None,
            tools,
            theme,
            pick: None,
            problems: Vec::new(),
            ranking: Vec::new(),
            rounds: 0,
            solving: false,
            rollouts: rollouts.max(1),
            state: None,
            scene: None,
            size: (REF_W, REF_H),
            ui_scale: None,
            days_read: None,
            days: DaysClock { left: days, higher: None },
            held_before_higher: 0,
            advised_tool: None,
            held_edit: None,
            pick_named: false,
            fade_kept: false,
            notes: Vec::new(),
            tools_changed: false,
            theme_fresh: false,
            treasure: None,
            board: None,
            consume: None,
            vault_top: None,
            prey_seen: Vec::new(),
            chained: false,
            meal: None,
            steady: BTreeMap::new(),
            edits: BTreeMap::new(),
            markers: BTreeMap::new(),
            rx: None,
            scan: None,
            queued: None,
            notify: None,
            log: None,
        }
    }

    /// True while a frame from [`Session::read_in_background`] is being read.
    pub fn scanning(&self) -> bool {
        self.scan.is_some()
    }

    /// Called from the worker after each partial ranking (the UI asks for a repaint).
    pub fn on_progress(&mut self, notify: impl Fn() + Send + Sync + 'static) {
        self.notify = Some(Arc::new(notify));
    }

    /// Reads a frame with the reader (OCR included) on a worker thread;
    /// [`Session::poll`] takes the result (see [`Session::install`]). A frame
    /// that arrives while a read runs waits for it; only the newest waiting
    /// one is read next.
    pub fn read_in_background(&mut self, frame: Rgba) {
        if self.scan.is_some() {
            self.queued = Some(frame);
            return;
        }
        self.start_scan(frame);
    }

    fn start_scan(&mut self, frame: Rgba) {
        let (tx, rx) = channel();
        self.scan = Some(rx);
        let (notify, keep, hint) = (self.notify.clone(), self.log.is_some(), self.ui_scale);
        std::thread::spawn(move || {
            if tx.send(scan(frame, keep, hint)).is_ok()
                && let Some(notify) = &notify
            {
                notify();
            }
        });
    }

    /// Takes a read frame. When it gives the scene on show again (the same
    /// model with the current corrections, screen and days), only the read
    /// time and warnings change: the corrections and the running solve stay.
    /// Otherwise the corrections go and a solve starts. A game dialog, a
    /// frame without the board (see [`Panels::off_board`]) or a frame of a
    /// window narrower than 16:9 (see [`shape_line`]) keeps the last
    /// reading and its solve on show, with a line that says so. The riot
    /// screen ends the run, see [`Session::end_run`]. Until the next run
    /// starts (a days counter reading at least one under the run's length,
    /// or an instrument pick with the counter hidden), a read is kept with
    /// [`RUN_OVER_WAIT`]. On the run's last day a board read with the counter
    /// hidden that differs from the board on show in [`AFTER_RUN_CELLS`]
    /// cells or more is taken as the riot screen missed and ends the run.
    ///
    /// The reader can't name held instruments, so they follow the advice: a
    /// new run (see [`DaysClock`]) clears them, and the instrument advised on
    /// a pick is taken as the one picked: held from the next screen that is
    /// not a dialog, unknown or View Prison Status. View Prison Status over a
    /// pick gives the board under it and keeps the pick on show.
    ///
    /// The run theme scroll sets the theme and keeps the last reading on
    /// show. A new run clears the theme unless it was set after the old
    /// run's last board read; the riot screen clears it too.
    ///
    /// A Warden's Treasure opens on its pick screen and closes on the next
    /// decision screen. The board shown after a pick, with other cells than
    /// the board under the pick, is the pick's effect: while the treasure
    /// has picks left (Opulence gives more), it becomes the board the next
    /// pick covers and the pick stays on show.
    ///
    /// A low-confidence cell read warns until it has given the same read on
    /// [`STEADY_READS`] reads in a row; a new run warns again.
    ///
    /// The run log gets the reading and, when it shows another screen or
    /// board than the last frame saved, the frame. A read that keeps the last
    /// reading (or, with none on show, sets [`Session::missed`]) is logged as
    /// a miss with its frame, saved once per screen in a row and once a
    /// minute while that screen lasts; a miss off the board or between runs
    /// saves no frame.
    fn install(&mut self, (scene, panels, size, frame, u): Scanned) {
        self.ui_scale = Some(u);
        let screen = panels.screen;
        match self.take_read(scene, panels, size) {
            Taken::Installed => {
                self.missed = None;
                self.log_read(frame);
            }
            Taken::Kept(line) => {
                // Off the board or between runs the frame shows another game screen.
                let frame = frame.filter(|_| line != OFF_BOARD && line != RUN_OVER_WAIT);
                self.log_miss(screen, line, frame);
            }
            Taken::Skipped => {}
        }
    }

    /// [`Session::install`] without the log.
    fn take_read(&mut self, scene: Scene, mut panels: Panels, size: (usize, usize)) -> Taken {
        self.scan = None;
        self.size = size;
        let shape = shape_line(size);
        // The lobby's scroll names the coming run's theme and shows no days counter.
        if shape.is_none()
            && let Some(id) = panels.theme
        {
            self.tools_changed |= self.theme != Some(id);
            self.theme = Some(id);
            self.theme_fresh = true;
        }
        // The board fading in after a Consume target screen reads the
        // instrument panel as empty, and its cells unsure, for a frame. A run
        // loses no instrument, so such a read is kept once and the next taken.
        let fading = !self.fade_kept
            && self.reading.as_ref().is_some_and(|r| r.screen == Screen::ChainTarget)
            && !self.tools.is_empty()
            && panels.held.as_ref().is_some_and(Vec::is_empty);
        self.fade_kept = fading;
        let kept = shape.clone().or_else(|| match panels.screen {
            Screen::Theme => Some(format!("{THEME_READ}{}.", data().theme_name(self.theme).unwrap_or_default())),
            Screen::Popup => Some(POPUP.to_string()),
            _ if panels.off_board() => Some(OFF_BOARD.to_string()),
            _ if fading => Some(FADING.to_string()),
            _ => None,
        });
        if let Some(line) = kept {
            if let Some(r) = &mut self.reading {
                r.warnings.retain(|w| !kept_line(&w.text));
                r.warnings.insert(0, Warning { cell: None, kind: WarningKind::Read, text: line.clone() });
            } else {
                self.missed = Some(match panels.screen {
                    _ if shape.is_some() || panels.screen == Screen::Theme => line.clone(),
                    Screen::Popup => "A popup covered the game.".to_string(),
                    _ => "The last read found no board or pick on screen.".to_string(),
                });
            }
            return Taken::Kept(line);
        }
        if panels.screen == Screen::RunEnd {
            self.end_run();
            return Taken::Skipped;
        }
        // Between runs only a run start is taken: the screens players stay on
        // after the riot (chest rooms and such) read as junk boards.
        let starts_run = panels.days.is_some_and(|left| i64::from(left) >= RUN_DAYS - 1)
            || (panels.screen == Screen::InstrumentPick && panels.days.is_none());
        if self.run_over.is_some() && !starts_run {
            self.missed = Some(RUN_OVER_WAIT.to_string());
            return Taken::Kept(RUN_OVER_WAIT.to_string());
        }
        // The riot screen missed: a board with a hidden counter on the last
        // day, far from the board on show, is a screen after the run.
        if self.days.left == Some(0)
            && panels.days.is_none()
            && matches!(panels.screen, Screen::Patrol | Screen::Manage | Screen::ChainTarget | Screen::Unknown)
            && let Some(r) = &self.reading
        {
            let model = to_model(&scene, &panels, &self.tools, None, self.run_days()).0;
            let differ = model.cells.iter().zip(&r.model.cells).filter(|(a, b)| cell_sight(a) != cell_sight(b)).count();
            if differ >= AFTER_RUN_CELLS {
                self.end_run();
                return Taken::Skipped;
            }
        }
        self.run_over = None;
        if self.reading.as_ref().is_some_and(|r| r.screen == Screen::ChainTarget) {
            self.meal = self.meal_picked();
        }
        self.days_read = panels.days;
        // A pick over no board read is taken as the run start (see `to_model`),
        // and its hidden counter as the run start's too: a count kept from an
        // earlier read or a past session can be stale. A lower count kept is a
        // past run's, so the instruments and theme held with it go.
        let (days, new_run) = if run_start(panels.screen, self.board.as_ref()) && panels.days.is_none() {
            let start = self.run_days() as u32;
            let new_run = self.days.left.is_some_and(|left| left < start);
            if new_run {
                self.held_before_higher = self.tools.len();
            }
            self.days = DaysClock { left: Some(start), higher: None };
            (Some(start), new_run)
        } else {
            self.days.update_run(panels.days)
        };
        panels.days = days;
        if new_run && let Some(log) = &mut self.log {
            log.new_run();
        }
        if self.days.higher.is_some() {
            self.held_before_higher = self.tools.len();
        }
        let mut notes = Vec::new();
        let mut held_changed = false;
        let stale = if new_run { self.held_before_higher.min(self.tools.len()) } else { 0 };
        if stale > 0 {
            self.tools.drain(..stale);
            self.held_edit = None;
            notes.push("A new run: the held instruments were cleared.".to_string());
            held_changed = true;
        }
        if new_run && !self.theme_fresh && self.theme.take().is_some() {
            notes.push("A new run: the theme was cleared.".to_string());
            held_changed = true;
        }
        // A higher count waiting for its second read already belongs to the new run.
        if self.days.higher.is_none() {
            self.theme_fresh = false;
        }
        // Only Iron Law runs past 20 days, so a longer count names the theme
        // when the lobby's scroll was not read; after the new-run clear above,
        // which would drop it again.
        if days.is_some_and(|left| i64::from(left) > RUN_DAYS) && self.theme != Some(IRON_LAW) {
            self.tools_changed = true;
            self.theme = Some(IRON_LAW);
            self.theme_fresh = true;
        }
        if !matches!(panels.screen, Screen::InstrumentPick | Screen::PrisonStatus | Screen::Unknown) {
            self.pick_named = false;
            if let Some(t) = self.advised_tool.take()
                && !self.tools.contains(&t)
            {
                self.tools.push(t);
                self.held_edit = None;
                held_changed = true;
            }
        }
        // A sure read of the instrument panel names the held list, unless the
        // player set it while the panel read the same and the session has not
        // changed it since.
        if let Some(held) = &panels.held
            && self.held_edit.as_ref() != Some(&panels.held)
        {
            self.held_edit = None;
            if *held != self.tools {
                self.tools = held.clone();
                held_changed = true;
                notes.push(if held.is_empty() {
                    format!("The {} panel shows none.", data().instrument_name)
                } else {
                    let names: Vec<&str> = held.iter().map(|&t| data().tool_names[t as usize].as_str()).collect();
                    format!("The {} panel shows: {}.", data().instrument_name, names.join(", "))
                });
            }
        }
        // The board shown over an open treasure's pick: a flicker while View
        // Prison Status opens when its cells are the pick's, else the pick's
        // effect, the board the next pick covers while picks are left.
        let mut after_pick = false;
        if panels.screen == Screen::Unknown
            && self.days_read.is_some()
            && let Some(taken) = self.treasure
            && let (Some((_, over, _)), Some(r)) = (&self.scene, &self.reading)
            && over.screen == Screen::TreasurePick
        {
            if to_model(&scene, &panels, &self.tools, self.board.as_ref(), self.run_days()).0.cells == r.model.cells {
                return Taken::Skipped;
            }
            after_pick = taken + 1 < treasure_picks(self.theme);
            self.treasure = after_pick.then_some(taken + 1);
        }
        match panels.screen {
            Screen::TreasurePick => self.treasure = Some(self.treasure.unwrap_or(0)),
            Screen::Patrol | Screen::Manage | Screen::ChainTarget | Screen::CellPick | Screen::Interrogation | Screen::InstrumentPick | Screen::DayBanner => {
                self.treasure = None;
            }
            _ => {}
        }
        // View Prison Status shows the real board under a pick, and so does
        // the board after a treasure's pick with picks left: it becomes the
        // last board read, in place of any board inferred after the consume,
        // and the pick stays on show, solved on that board.
        let (scene, panels) = match (panels.screen, self.scene.take()) {
            (Screen::PrisonStatus | Screen::Unknown, Some((pick, mut over, _)))
                if after_pick || (panels.screen == Screen::PrisonStatus && matches!(over.screen, Screen::TreasurePick | Screen::InstrumentPick)) =>
            {
                self.board = Some(to_model(&scene, &panels, &self.tools, self.board.as_ref(), self.run_days()).0);
                self.consume = None;
                self.meal = None;
                self.chained = false;
                over.days = panels.days;
                (pick, over)
            }
            _ => (scene, panels),
        };
        self.scene = Some((scene, panels, SystemTime::now()));
        let mut before = std::mem::take(&mut self.steady);
        if new_run {
            before.clear();
        }
        // The stored read's warnings, none dropped yet (`steady` is empty).
        let keys: Vec<String> = self.build().map(|r| r.warnings.iter().filter_map(steady_key).collect()).unwrap_or_default();
        self.steady = keys.into_iter().map(|k| {
            let n = before.get(&k).map_or(1, |n| n + 1);
            (k, n)
        }).collect();
        if !held_changed
            && let (Some(old), Some(new)) = (&self.reading, self.build())
            && (&old.model, old.screen, old.days_left) == (&new.model, new.screen, new.days_left)
        {
            self.reading = Some(new);
            return Taken::Installed;
        }
        self.tools_changed |= held_changed;
        self.notes = notes;
        self.edits.clear();
        self.markers.clear();
        self.rebuild();
        Taken::Installed
    }

    /// Logs a read that installed nothing: its screen, the line it gave,
    /// the frame size and `frame` when its screen differs from the last
    /// frame saved.
    fn log_miss(&mut self, screen: Screen, line: String, frame: Option<Rgba>) {
        let Some(log) = &mut self.log else { return };
        let file = frame.and_then(|f| log.frame(screen, None, f));
        let fields = json!({
            "screen": format!("{screen:?}"),
            "line": line,
            "size": [self.size.0, self.size.1],
            "frame": file,
        });
        log.event("miss", SystemTime::now(), fields);
    }

    /// Logs the window auto read captures, as the watcher describes it, and
    /// the language Windows OCR reads in (null with none installed).
    pub fn log_window(&mut self, window: &str) {
        if let Some(log) = &mut self.log {
            log.window(json!({ "window": window, "ocr": crate::reader::ocr::language() }));
        }
    }

    /// Logs the reading on show, and `frame` when it shows another screen or
    /// board than the last frame saved.
    fn log_read(&mut self, frame: Option<Rgba>) {
        let (Some(log), Some(r)) = (&mut self.log, &self.reading) else { return };
        let file = frame.and_then(|f| log.frame(r.screen, Some(&r.model), f));
        let warnings: Vec<&str> = r.warnings.iter().map(|w| w.text.as_str()).collect();
        let fields = json!({
            "screen": format!("{:?}", r.screen),
            "days_left": r.days_left,
            "tools": tool_keys(&self.tools),
            "theme": data().theme_name(self.theme),
            "warnings": warnings,
            "frame": file,
            "model": r.model,
        });
        log.event("read", r.at, fields);
    }

    /// Logs the advice of a finished solve: the pick and the picks about as
    /// good.
    fn log_advice(&self) {
        let (Some(log), Some(a)) = (&self.log, self.advice()) else { return };
        let fields = json!({
            "read_at": self.reading.as_ref().map(|r| timestamp(r.at)),
            "pick": a.label,
            "also_good": also_good(&self.ranking, a.option).into_iter().map(|k| self.ranking[k].label.as_str()).collect::<Vec<_>>(),
            "rounds": self.rounds,
        });
        log.event("advice", SystemTime::now(), fields);
    }

    /// Corrects screen cell `cell` (2-16; 1 is the Interrogation Room) and re-solves.
    pub fn edit_cell(&mut self, cell: u8, edit: CellEdit) -> Result<(), String> {
        self.check_editable(cell)?;
        self.edits.insert(cell, edit);
        self.log_edit(cell, format!("{edit:?}"));
        self.rebuild();
        Ok(())
    }

    /// Adds or removes the incident marker on screen cell `cell` (2-16) and re-solves.
    pub fn set_marker(&mut self, cell: u8, on: bool) -> Result<(), String> {
        self.check_editable(cell)?;
        self.markers.insert(cell, on);
        self.log_edit(cell, format!("Marker {}", if on { "on" } else { "off" }));
        self.rebuild();
        Ok(())
    }

    fn check_editable(&self, cell: u8) -> Result<(), String> {
        let count = self.reading.as_ref().ok_or("no board read yet")?.model.cells.len();
        if !(2..=count).contains(&(cell as usize)) {
            return Err(format!("cell {cell} can't be edited"));
        }
        Ok(())
    }

    fn log_edit(&self, cell: u8, edit: String) {
        if let Some(log) = &self.log {
            log.event("edit", SystemTime::now(), json!({ "cell": cell, "edit": edit }));
        }
    }

    /// True once after the session changed the held instruments or the theme
    /// itself (the scroll, a new run, an advised pick, the run's end), so the
    /// UI saves them.
    pub fn take_tools_changed(&mut self) -> bool {
        std::mem::take(&mut self.tools_changed)
    }

    /// The days until Riot in use: None until a first read and after a run's end.
    pub fn days_left(&self) -> Option<u32> {
        self.days.left
    }

    /// Sets the held instruments (the panel shows only icons) and re-solves;
    /// the lines about the session's own changes to them go.
    pub fn set_tools(&mut self, tools: Vec<Tool>) {
        if let Some(log) = &self.log {
            log.event("edit", SystemTime::now(), json!({ "tools": tool_keys(&tools) }));
        }
        self.tools = tools;
        self.held_edit = Some(self.scene.as_ref().and_then(|(_, p, _)| p.held.clone()));
        self.notes.clear();
        if self.reading.as_ref().is_some_and(|r| r.screen == Screen::InstrumentPick) {
            self.advised_tool = None;
            self.pick_named = true;
        }
        if self.scene.is_some() {
            self.rebuild();
        }
    }

    /// Sets the run's theme (the scroll was missed or misread) and re-solves.
    pub fn set_theme(&mut self, theme: Option<u32>) {
        if let Some(log) = &self.log {
            log.event("edit", SystemTime::now(), json!({ "theme": data().theme_name(theme) }));
        }
        self.theme = theme;
        self.theme_fresh = true;
        if self.reading.is_some() {
            self.solve();
        }
    }

    /// The treasure pick the running solve plays: the gifts on offer and the
    /// picks left of the treasure, this one included.
    pub fn treasure_pick(&self) -> Option<(&[Gift], u32)> {
        match self.state.as_ref()?.pending.first()? {
            Pending::Gift { options, picks } => Some((options, *picks)),
            _ => None,
        }
    }

    /// The model again from the stored read, the tools and the edits; then a solve.
    fn rebuild(&mut self) {
        let Some(mut reading) = self.build() else { return };
        // The Hunt's target pick shows no cards, and its white glow on a
        // lone target spoils that cell's read: the cells and the day's used
        // Consume and Replace are the same day's manage screen's.
        let prey = reading.screen == Screen::CellPick && reading.model.choice.kind == ChoiceKind::Prey;
        if prey && let Some(m) = self.consume.as_ref().filter(|m| m.day == reading.model.day) {
            reading.model.cells = m.cells.clone();
            (reading.model.devour_used, reading.model.swap_used) = (m.devour_used, m.swap_used);
        }
        if matches!(reading.screen, Screen::Patrol | Screen::Manage | Screen::CellPick | Screen::PrisonStatus) {
            self.board = Some(reading.model.clone());
            self.chained = false;
            self.meal = None;
        }
        match reading.screen {
            Screen::CellPick if prey => {}
            Screen::Patrol | Screen::CellPick | Screen::PrisonStatus => self.consume = None,
            Screen::Manage => self.consume = Some(reading.model.clone()),
            Screen::ChainTarget => {
                // The screen hides the counter, the vault and the decision
                // buttons: only its cells and warden are taken. A cell bitten
                // just before may still show as open while its inmate fades:
                // it locks again, as every consumed cell does.
                // The red jaws over a target spoil its read: a cell read with
                // low confidence keeps the board's cell, and its warning goes,
                // unless the player corrected it.
                let before = self.consume.take().or_else(|| self.board.clone());
                let mut read = reading.model.clone();
                let unedited = |n: usize| !self.edits.contains_key(&(n as u8 + 1));
                if let Some(m) = &before {
                    let unsure = |w: &Warning| w.text.starts_with(CHECK_CELL) && w.cell.is_some_and(unedited);
                    for n in reading.warnings.iter().filter(|w| unsure(w)).filter_map(|w| w.cell) {
                        read.cells[n] = m.cells[n].clone();
                    }
                    reading.warnings.retain(|w| !unsure(w));
                }
                let mut m = before.unwrap_or_else(|| reading.model.clone());
                self.chained |= bitten(&m, &read);
                for (cell, read) in m.cells.iter_mut().zip(&read.cells) {
                    *cell = match (&*cell, read) {
                        (CellModel::Inmate { .. }, &CellModel::Empty { event }) => CellModel::Sealed { event },
                        _ => read.clone(),
                    };
                }
                // The board before may be the day's patrol.
                (m.warden, m.step) = (reading.model.warden, Step::Manage);
                if let Some((scene, _, _)) = &self.scene {
                    let cells: Vec<usize> = jawed(scene).into_iter().filter(|&n| unedited(n)).collect();
                    reading.warnings.extend(casket_meals(&mut m, &reading.model, &cells));
                }
                // Before the first bite the day's consume is still open.
                m.devour_used = self.chained;
                m.tools = reading.model.tools.clone();
                // The solve takes this board, not the bare read.
                reading.model = m.clone();
                self.consume = Some(m);
            }
            _ => {}
        }
        let (day, vault) = (reading.model.day, &reading.model.vault);
        if self.vault_top.as_ref().is_none_or(|(d, v)| day < *d || vault.len() > v.len()) {
            self.vault_top = Some((day, vault.clone()));
        }
        if matches!(reading.screen, Screen::Patrol | Screen::Manage | Screen::CellPick | Screen::PrisonStatus) {
            let seen = &self.prey_seen;
            self.prey_seen = prey_cells(&reading.model)
                .map(|x| seen.iter().copied().find(|&(c, d)| c == x && d <= day).unwrap_or((x, day)))
                .collect();
        }
        self.reading = Some(reading);
        self.solve();
    }

    /// The days the run lasts: Iron Law extends it.
    fn run_days(&self) -> i64 {
        match self.theme {
            Some(IRON_LAW) => iron_law_days(),
            _ => RUN_DAYS,
        }
    }

    /// The reading of the stored read with the tools and the edits.
    fn build(&self) -> Option<Reading> {
        let (scene, panels, at) = self.scene.as_ref()?;
        let chains = self.tools.contains(&Tool::Chains);
        let vaulted = self.consume.as_ref().filter(|_| panels.screen == Screen::TreasurePick).and_then(|m| vaulted(m, self.meal, chains));
        let under = vaulted.as_ref().map(|(m, _, _)| m).or(self.board.as_ref());
        let assumed_days = run_start(panels.screen, under) && self.days_read.is_none();
        let (mut model, mut warnings) = to_model(scene, panels, &self.tools, under, self.run_days());
        // Chromatics never leave the vault in a run: a read that shows fewer
        // (the panel did not read, or only some of its slots) keeps the last
        // board's. A day before the board's is a new run.
        if let Some(board) = under.filter(|b| b.day <= model.day && b.vault.len() > model.vault.len()) {
            model.vault = board.vault.clone();
        }
        // The board under a treasure pick can be View Prison Status's with
        // the vault panel unread: the pick keeps the run's longest vault.
        if panels.screen == Screen::TreasurePick
            && let Some((_, vault)) = self.vault_top.as_ref().filter(|(day, v)| *day <= model.day && v.len() > model.vault.len())
        {
            model.vault = vault.clone();
        }
        for cell in &mut model.cells {
            if let CellModel::Inmate { prey: Some(mark), .. } = cell
                && let Some(&(_, first)) = self.prey_seen.iter().find(|&&(c, d)| Some(c as i64) == mark.cell && d <= model.day)
            {
                mark.rounds = (mark.rounds - (model.day - first)).max(1);
            }
        }
        warnings.retain(|w| steady_key(w).and_then(|k| self.steady.get(&k)).is_none_or(|&n| n <= STEADY_READS));
        if let Some((_, n, removed)) = vaulted {
            let left = format!("The Warden's Treasure follows a Chromatic Prisoner: the inmate in cell {} left for the vault", cell_label(n));
            let removed: Vec<String> = removed.into_iter().map(cell_label).collect();
            let text = match self.meal {
                _ if !removed.is_empty() => format!(
                    "{left}, lifted by the {} at the start of the day, which removed {}: check them with View Prison Status.",
                    tool_name(Tool::Chains.key()),
                    match &removed[..] {
                        [one] => format!("the inmate in cell {one}"),
                        _ => format!("the inmates in cells {}", removed.join(" and ")),
                    },
                ),
                Some(Meal { cell, certain: true }) => format!("{left} after consuming cell {}.", cell_label(cell)),
                Some(Meal { cell, certain: false }) => format!("{left}. Cell {} is taken as its last meal, the target Warden advised: click the cell to correct it.", cell_label(cell)),
                None => format!("{left}. The last cell it consumed is not known and still shows its inmate: check it with View Prison Status."),
            };
            warnings.push(Warning { cell: None, kind: WarningKind::Read, text });
        }
        // The Consume target screen always hides the counter; the run start's
        // count is assumed, not kept.
        if panels.screen != Screen::ChainTarget && !assumed_days {
            warnings.extend(kept_days(self.days_read, panels.days));
        }
        warnings.extend(self.notes.iter().map(|text| Warning { cell: None, kind: WarningKind::Read, text: text.clone() }));
        for (&cell, edit) in &self.edits {
            let n = cell as usize - 1;
            // A correction keeps the cell's incident marker and Tusk.
            let (event, ivory) = (model.cells[n].event(), model.cells[n].ivory());
            model.cells[n] = match *edit {
                CellEdit::Locked => CellModel::Sealed { event },
                CellEdit::Empty => CellModel::Empty { event },
                CellEdit::Inmate { beast, tier } => CellModel::Inmate { beast, tier, ivory, prey: None, event },
            };
        }
        // The Office and a Nest carry no marker.
        for (&cell, &on) in &self.markers {
            if let CellModel::Sealed { event }
            | CellModel::Empty { event }
            | CellModel::Inmate { event, .. }
            | CellModel::Scouted { event, .. }
            | CellModel::Scratch { event, .. } = &mut model.cells[cell as usize - 1]
            {
                *event = on.then_some(EventMark::Unknown);
            }
        }
        Some(Reading { model, warnings, screen: panels.screen, days_left: panels.days, at: *at })
    }

    /// The meal picked on the Consume target screen on show: the only target
    /// the engine offers there, or else the advised one.
    fn meal_picked(&self) -> Option<Meal> {
        let st = self.state.as_ref()?;
        let chain = st.pending.iter().find_map(|p| match p {
            Pending::Chain { options, .. } => Some(options.clone()),
            _ => None,
        });
        let offered = chain.unwrap_or_else(|| start_options(st).iter().map(|o| o.cell).collect());
        if let [cell] = offered[..] {
            return Some(Meal { cell, certain: true });
        }
        match self.advice()?.action {
            Action::Devour { to } | Action::Chain { to } => Some(Meal { cell: to, certain: false }),
            _ => None,
        }
    }

    /// What the window recommends now; see [`advise`].
    pub fn advice(&self) -> Option<Advice> {
        advise(self.pick.as_ref(), &self.ranking, !self.solving || self.rounds >= LIVE_MIN)
    }

    /// How the window shows the advice `a`; see [`shown`].
    pub fn shown(&self, a: &Advice) -> Shown {
        match &self.state {
            Some(st) => shown(st, &self.ranking, a),
            None => Shown { headline: None, why: a.why.clone() },
        }
    }

    /// Keeps the instrument advised on an instrument pick.
    fn note_advised_tool(&mut self) {
        if !self.pick_named
            && self.reading.as_ref().is_some_and(|r| r.screen == Screen::InstrumentPick)
            && let Some(advice) = self.advice()
            && let Action::Choose { id: ChoiceId::Name(key) } = &advice.action
            && let Some(t) = Tool::from_key(key)
        {
            self.advised_tool = Some(t);
        }
    }

    /// Starts a solve of the current model on a worker thread, replacing any
    /// running one: rounds of [`ROUND`] playouts until `rollouts`, each
    /// reported to [`Session::poll`]. Dropping the old receiver stops the
    /// old worker at its next send.
    pub fn solve(&mut self) {
        self.stop();
        let Some(reading) = &self.reading else { return };
        let (mut st, problems) = build_state(&reading.model, SEED);
        if !problems.is_empty() {
            self.problems = problems;
            return;
        }
        st.theme = self.theme;
        // A treasure pick always offers all four gifts (build_state lists
        // them), for the picks the treasure has left.
        if let Some(Pending::Gift { picks, .. }) = st.pending.first_mut() {
            *picks = treasure_picks(self.theme).saturating_sub(self.treasure.unwrap_or(0)).max(1);
        }
        // A Consume target screen picks the day's first bite, or a chain's
        // next, among the cells the screen marks as targets.
        let target = reading.screen == Screen::ChainTarget;
        if target && self.chained {
            if let Some(eater) = st.cells[st.position].inmate {
                offer_chain(&mut st, eater.id);
            }
            // The game opens this screen only while the eater has a target, so
            // a board read that gives it none is wrong (the eater's rarity not
            // risen yet, a misread target) and gets no advice.
            if !st.pending.iter().any(|p| matches!(p, Pending::Chain { .. })) {
                self.problems = vec![format!(
                    "The board read gives the eater in cell {} nothing to Consume, but the game marks targets: check the eater and the marked cells.",
                    cell_label(st.position)
                )];
                return;
            }
        }
        let first = target && !self.chained;
        let mut warning = None;
        let mut bites = Vec::new();
        if target {
            let jawed = self.scene.as_ref().map_or(Vec::new(), |(scene, _, _)| jawed(scene));
            let (eater, chain) = (st.position, st.pending.iter_mut().find_map(|p| match p {
                Pending::Chain { options, .. } => Some(options),
                _ => None,
            }));
            if let Some(options) = chain {
                (*options, warning) = consume_targets(options, &jawed, eater);
            } else if first {
                let options: Vec<usize> = start_options(&st).iter().map(|o| o.cell).collect();
                (bites, warning) = consume_targets(&options, &jawed, eater);
            }
        }
        if let Some(r) = &mut self.reading {
            r.warnings.retain(|w| !w.text.starts_with(TARGETS_DIFFER));
            r.warnings.extend(warning);
        }
        let pick = if first { instant_bite(&st, &bites) } else { instant(&st) };
        self.pick = pick.map(|i| Pick { action: i.action, label: i.label, why: i.why });
        self.state = Some(st.clone());
        self.note_advised_tool();
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.solving = true;
        let (notify, rollouts) = (self.notify.clone(), self.rollouts);
        // The playouts use all cores but one, below normal priority so the
        // game keeps its frame rate.
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(1).max(1));
        std::thread::spawn(move || {
            #[cfg(windows)]
            crate::solver::solve::set_priority(windows::Win32::System::Threading::THREAD_PRIORITY_BELOW_NORMAL.0);
            let mut r = Rollouts::with(&st, SEED, &TUNED, first.then_some(&bites[..]));
            loop {
                let ran = r.run_on(ROUND.min(rollouts - r.rounds()), threads);
                let done = ran == 0 || r.rounds() >= rollouts;
                let progress = Progress { rounds: r.rounds(), done, options: r.ranking() };
                if tx.send(progress).is_err() {
                    return;
                }
                if let Some(notify) = &notify {
                    notify();
                }
                if done {
                    return;
                }
            }
        });
    }

    /// Drops the solve, its ranking and pick; the worker stops at its next send.
    fn stop(&mut self) {
        self.ranking.clear();
        self.rounds = 0;
        self.pick = None;
        self.state = None;
        self.problems.clear();
        self.solving = false;
        self.rx = None;
    }

    /// The riot screen: the run is over. The solve stops and the board goes
    /// with the run (nothing on it can be played or corrected any more); the
    /// line says how many Chromatic Prisoners the run vaulted, the most any
    /// of its readings showed.
    /// The held instruments and the days count go too. The riot screen read
    /// again keeps the line. The run log gets a run_end and the next run's
    /// folder only when the last reading was on the run's last day: the riot
    /// screen also shows right after a run ends, and a board read in between
    /// (a day 1 with an empty vault) is no run of its own.
    fn end_run(&mut self) {
        if self.run_over.is_some() {
            return;
        }
        let vaulted = self.vault_top.as_ref().map(|(_, v)| v.len());
        self.run_over = Some(match vaulted {
            Some(1) => "The run is over: 1 Chromatic Prisoner vaulted.".to_string(),
            Some(n) => format!("The run is over: {n} Chromatic Prisoners vaulted."),
            None => "The run is over.".to_string(),
        });
        if let Some(log) = &mut self.log
            && self.reading.as_ref().is_some_and(|r| r.model.day >= r.model.max_days)
        {
            log.event("run_end", SystemTime::now(), json!({ "vaulted": vaulted }));
            log.new_run();
        }
        self.stop();
        self.reading = None;
        self.missed = None;
        self.scene = None;
        self.board = None;
        self.consume = None;
        self.vault_top = None;
        self.prey_seen.clear();
        self.chained = false;
        self.meal = None;
        self.advised_tool = None;
        self.held_edit = None;
        self.pick_named = false;
        self.steady.clear();
        self.days = DaysClock::default();
        self.treasure = None;
        self.theme_fresh = false;
        if self.theme.take().is_some() {
            self.tools_changed = true;
        }
        if !self.tools.is_empty() {
            self.tools.clear();
            self.tools_changed = true;
        }
    }

    /// Takes the worker's latest ranking; true when something changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        if let Some(scan) = &self.scan {
            match scan.try_recv() {
                Ok(scanned) => {
                    self.install(scanned);
                    changed = true;
                    self.read_queued();
                }
                // The reader thread panicked: the spinner must not stick.
                Err(TryRecvError::Disconnected) => {
                    self.scan = None;
                    self.read_queued();
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        let Some(rx) = &self.rx else { return changed };
        let was_solving = self.solving;
        while let Ok(p) = rx.try_recv() {
            self.ranking = p.options;
            self.rounds = p.rounds;
            self.solving = !p.done;
            changed = true;
        }
        if changed {
            self.note_advised_tool();
        }
        if was_solving && !self.solving {
            self.log_advice();
        }
        changed
    }

    /// Starts a read of the frame that waited for the last one, if any.
    fn read_queued(&mut self) {
        if let Some(frame) = self.queued.take() {
            self.start_scan(frame);
        }
    }

    /// Blocks until the pending reads, the running solve and the run log's
    /// writes finish (for tests).
    pub fn wait(&mut self) {
        while let Some(scan) = self.scan.take() {
            if let Ok(scanned) = scan.recv() {
                self.install(scanned);
            }
            self.read_queued();
        }
        let was_solving = self.solving;
        while self.solving {
            let Some(rx) = &self.rx else { break };
            match rx.recv() {
                Ok(p) => {
                    self.ranking = p.options;
                    self.rounds = p.rounds;
                    self.solving = !p.done;
                }
                Err(_) => self.solving = false,
            }
        }
        self.note_advised_tool();
        if was_solving {
            self.log_advice();
        }
        if let Some(log) = &self.log {
            log.wait();
        }
    }
}
