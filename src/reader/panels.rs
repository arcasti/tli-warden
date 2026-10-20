//! The text panels of the S16 prison screen (docs/s16-screen.md): which
//! screen is up, the destination cards, the decision buttons, the days
//! counter and the instrument or treasure offers, read with the Windows OCR
//! engine from fixed regions of the 1918x1078 frame. The Chromatic Prisoner
//! slots and the instrument icons are read from pixels.

use super::instruments::read_held;
use super::ocr::{Line, ocr, ocr_all};
use super::scene::beast_at;
use super::{Rect, Rgba};
use crate::engine::data::{Beast, Tool, data};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    /// "Select a Destination" with the destination cards.
    Patrol,
    /// "Make Your Decision" with the Consume / Replace / End buttons.
    Manage,
    /// "Select an Inmate to Consume": a Consume target pick.
    ChainTarget,
    /// "Select a Torture Instrument".
    InstrumentPick,
    /// "Warden's Treasure obtained": the Chromatic reward pick.
    TreasurePick,
    /// View Prison Status over a pick: the board with the counter and the
    /// vault, a "Return to Selection" button and no decision on it.
    PrisonStatus,
    /// The Interrogation Room's Instigate and Bestow cards under the board.
    Interrogation,
    /// "Select an Objective" under the board: the Sewer Ratway's pick of the
    /// cell the warden moves to.
    CellPick,
    /// The "Day N" plaque over a dimmed board.
    DayBanner,
    /// The run theme scroll shown once in the lobby before a run, titled
    /// with the theme's name.
    Theme,
    /// A game dialog over the board: the network timeout or connecting
    /// notice, or a Prison Incident popup.
    Popup,
    /// "A Riot has broken out in Tartarus" with the Ignore and Suppress
    /// buttons: the run is over.
    RunEnd,
    /// None of the above, such as the board between moves.
    Unknown,
}

impl Screen {
    /// The screen name used by the labels in data/scan-truth-s16.json.
    pub fn label(self) -> &'static str {
        match self {
            Screen::Patrol | Screen::Manage | Screen::Unknown => "board",
            Screen::ChainTarget => "chainTarget",
            Screen::InstrumentPick => "instrumentPick",
            Screen::TreasurePick => "treasurePick",
            Screen::PrisonStatus => "prisonStatus",
            Screen::Interrogation => "interrogation",
            Screen::CellPick => "cellPick",
            Screen::DayBanner => "dayBanner",
            Screen::Theme => "theme",
            Screen::Popup => "popup",
            Screen::RunEnd => "runEnd",
        }
    }
}

/// A filled Chromatic Prisoner slot: the beast when its portrait matches a
/// board template, with the match confidence.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VaultSlot {
    pub beast: Option<Beast>,
    pub conf: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Panels {
    pub screen: Screen,
    /// "N day(s) until Riot".
    pub days: Option<u32>,
    /// Destination cards in order, as screen cell numbers (1 = Go to
    /// Interrogation Room); None for a card whose text did not read.
    pub doors: Vec<Option<u8>>,
    /// Buttons on screen: "Consume", "Replace", "End", or "Cancel" on a
    /// target pick.
    pub decision: Vec<&'static str>,
    /// The instruments on an instrument pick, in card order.
    pub instruments: Vec<Tool>,
    /// Filled Chromatic Prisoner slots.
    pub vault: Vec<VaultSlot>,
    /// The Torture Instrument panel shows at least one icon.
    pub tool_icons: bool,
    /// The theme the run theme scroll names; None on every other screen.
    pub theme: Option<u32>,
    /// The run objective the objective panel names, as (id, progress); see
    /// [`objective`].
    pub objective: Option<(u32, u32)>,
    /// The instruments the panel icons name; None off the board screens or
    /// when an icon is unsure.
    pub held: Option<Vec<Tool>>,
}

impl Panels {
    /// The prison board is not on screen to read: no decision, dialog or riot
    /// title was found and the days counter, which every board screen but the
    /// Consume target pick shows, did not read (a loading screen, the
    /// hideout), or a day banner hides the counter.
    pub fn off_board(&self) -> bool {
        matches!(self.screen, Screen::Unknown | Screen::DayBanner) && self.days.is_none()
    }
}

// Regions (x, y, w, h) at 1918x1078 and the upscale each reads best at.
const DAYS: (Rect, f32) = (Rect::new(800, 0, 320, 60), 2.0);
const TITLE: (Rect, f32) = (Rect::new(0, 70, 1918, 60), 1.5);
const HEADING: (Rect, f32) = (Rect::new(420, 680, 1100, 40), 2.0);
const CARDS: (Rect, f32) = (Rect::new(440, 740, 1040, 200), 1.5);
const TARGET: (Rect, f32) = (Rect::new(700, 800, 520, 50), 2.0);
const CANCEL: (Rect, f32) = (Rect::new(1600, 740, 318, 140), 2.0);
const BANNER: (Rect, f32) = (Rect::new(760, 400, 400, 130), 2.0);
/// The card names on the Select a Torture Instrument screen, one row across
/// the cards at y 480-515 (rec1-t031, a small font for "Ferocious Beast's
/// Ravenous Chains"); the hovered card is raised, its name at about y 435-470.
/// The descriptions start below about y 550.
const NAMES: (Rect, f32) = (Rect::new(0, 420, 1918, 125), 1.0);
// The game's dialogs: "Tip" over "Network request timeout", and the title
// of a Prison Incident popup.
const DIALOG: (Rect, f32) = (Rect::new(560, 340, 800, 260), 1.0);
const INCIDENT: (Rect, f32) = (Rect::new(1100, 190, 700, 70), 1.0);
/// The title band of the run theme scroll, inside the DIALOG region.
const THEME: (Rect, f32) = (Rect::new(660, 310, 600, 70), 1.0);
/// The objective panel at the top left of the board: the name, the task and
/// the progress ("0/2") or "Completed". A name on two lines starts higher.
const OBJECTIVE: (Rect, f32) = (Rect::new(30, 110, 370, 140), 1.5);
/// The "Return to Selection" button of View Prison Status, where a pick
/// shows "View Prison Status".
const STATUS: (Rect, f32) = (Rect::new(1540, 115, 300, 40), 2.0);
/// The title of the riot screen that ends a run.
const RIOT: (Rect, f32) = (Rect::new(400, 95, 1120, 80), 1.0);
/// Distance between the centres of two destination cards.
const CARD_PITCH: i32 = 345;

/// Chromatic Prisoner slots in fill order: two columns, top to bottom.
const SLOT_X: [i32; 2] = [1720, 1805];
const SLOT_Y: [i32; 5] = [240, 335, 430, 525, 620];
/// Mean luma over a slot's centre above which it holds a portrait.
const SLOT_FILLED: f32 = 60.0;
/// The first instrument icon of the Torture Instrument panel.
const ICON: (i32, i32) = (122, 390);
/// Luma standard deviation over the first icon spot above which an icon is there.
const ICON_SPREAD: f32 = 30.0;

/// Lowercase ASCII letters and digits only: spacing and punctuation are the
/// parts OCR gets wrong most.
pub fn squash(s: &str) -> String {
    s.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
}

/// Levenshtein distance over chars.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for j in 0..b.len() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != b[j])).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

/// 1 for the same text up to case, spacing and punctuation, falling with the
/// edit distance.
pub fn similarity(a: &str, b: &str) -> f32 {
    let (a, b) = (squash(a), squash(b));
    let n = a.chars().count().max(b.chars().count());
    if n == 0 {
        return 0.0;
    }
    1.0 - distance(&a, &b) as f32 / n as f32
}

/// The candidate closest to `text` and its similarity, when at least `min`.
pub fn best_match<S: AsRef<str>>(text: &str, candidates: &[S], min: f32) -> Option<(usize, f32)> {
    candidates
        .iter()
        .enumerate()
        .map(|(i, c)| (i, similarity(text, c.as_ref())))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .filter(|&(_, s)| s >= min)
}

fn has(lines: &[Line], target: &str, min: f32) -> bool {
    lines.iter().any(|l| similarity(&l.text, target) >= min || squash(&l.text).contains(&squash(target)))
}

/// A number as OCR writes it: letters that look like digits count as digits.
fn number(token: &str) -> Option<u32> {
    let digits: String = token
        .chars()
        .map(|c| match c {
            'O' | 'o' | 'D' | 'Q' => '0',
            'I' | 'l' | '|' | 'i' => '1',
            'Z' | 'z' => '2',
            'S' | 's' => '5',
            'B' => '8',
            c => c,
        })
        .collect();
    digits.parse().ok()
}

/// "N day(s) until Riot": the number is what comes before "day", or before
/// the first space when "day" misreads. OCR can drop the space ("0day(s)")
/// and reads the red 0 as "()". On a frame scaled up from a short window the
/// number can come as its own line after the text, so the lines go left to
/// right.
fn days(lines: &[Line]) -> Option<u32> {
    let mut sorted: Vec<&Line> = lines.iter().collect();
    sorted.sort_by_key(|l| l.rect.x);
    let text = joined(sorted);
    let text = text.trim();
    let lower = text.to_ascii_lowercase();
    let (head, rest) = text.split_at(lower.find("day").or_else(|| lower.find(' '))?);
    let head = head.replace(' ', "").replace("()", "0");
    (similarity(rest, "day(s) until Riot") >= 0.6 || squash(rest).ends_with("riot")).then(|| number(&head))?
}

/// A game dialog: the "Tip" box of a network timeout, the connecting notice,
/// or a Prison Incident popup titled with the incident's name.
fn popup(dialog: &[Line], incident: &[Line]) -> bool {
    let names: Vec<&str> = data().incidents.iter().map(|e| e.name.as_str()).collect();
    dialog.iter().any(|l| squash(&l.text) == "tip")
        || has(dialog, "Network request timeout", 0.75)
        || has(dialog, "Connecting to server", 0.75)
        || incident.iter().any(|l| best_match(&l.text, &names, 0.75).is_some())
}

/// The theme a run theme scroll's title names.
fn theme(title: &[Line]) -> Option<u32> {
    data().themes.iter().find(|t| has(title, &t.name, 0.75)).map(|t| t.id)
}

/// The objective the panel names, as (id, progress). The name can wrap onto
/// two lines, so the panel's text joined also counts. "Completed" is the
/// objective's target (1 for a rating objective). When the name reads but the
/// progress does not, the progress is 0: the objective still steers the
/// solve, and a later read corrects the count.
fn objective(lines: &[Line]) -> Option<(u32, u32)> {
    let mut sorted: Vec<&Line> = lines.iter().collect();
    sorted.sort_by_key(|l| l.rect.y);
    let text = squash(&joined(sorted.iter().copied()));
    let goal = data().objectives.iter().find(|o| has(lines, &o.name, 0.75) || text.contains(&squash(&o.name)))?;
    Some((goal.id, lines.iter().find_map(|l| progress(&l.text, goal.target)).unwrap_or(0)))
}

/// A progress line: "N/M" gives N, "Completed" gives `target`. "Completed"
/// ends the task's line ("Move 18 tiles Completed"), and OCR can glue it to
/// the word before ("UltimateCompleted"), so the last word or its tail counts.
/// OCR glues the count to the task too ("times0/2"), so the digits next to the
/// slash count first, then the whole word read as a number. The count after
/// the slash is the known target, so only a count above it is refused.
fn progress(text: &str, target: u32) -> Option<u32> {
    if similarity(text, "Completed") >= 0.75 || completed_tail(text) {
        return Some(target);
    }
    let (head, _) = text.trim().rsplit_once('/')?;
    let head = head.split_whitespace().last()?;
    let digits = head.len() - head.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let done = if digits > 0 { head[head.len() - digits..].parse().ok()? } else { number(head)? };
    (done <= target).then_some(done)
}

/// Whether the line's last word, or its last 8 to 10 letters, reads as
/// "Completed".
fn completed_tail(text: &str) -> bool {
    let Some(word) = text.split_whitespace().last() else { return false };
    let chars: Vec<char> = squash(word).chars().collect();
    (8..=10).filter(|&n| n <= chars.len()).any(|n| {
        let tail: String = chars[chars.len() - n..].iter().collect();
        similarity(&tail, "Completed") >= 0.75
    })
}

/// One destination card's text: "Go to Cell N" or "Go to Interrogation Room".
fn door(text: &str) -> Option<u8> {
    let s = squash(text);
    if s.contains("room") || similarity(text, "Go to Interrogation Room") >= 0.6 {
        return Some(1);
    }
    let tail: String = match s.rfind("cell") {
        // OCR reads the digits 1 and 0 as I, l and O ("Go to Cell IL").
        Some(at) => s[at + 4..].chars().map(|c| match c {
            'i' | 'l' => '1',
            'o' => '0',
            c => c,
        }).collect(),
        None => s.chars().rev().take_while(char::is_ascii_digit).collect::<Vec<_>>().into_iter().rev().collect(),
    };
    tail.parse().ok().filter(|n| (2..=16).contains(n))
}

/// The cards sit in a centred row, as many as today's doors (two with Burning
/// Stake): lines whose centres are within half a card pitch are one card. OCR
/// can drop "Go" or put junk before it and garble "Cell" ("eGo to Cell 11",
/// "to celgl"), so "to" and "cel" also mark card text. A card whose text
/// doesn't parse goes to `reread` with its centre.
fn doors(lines: &[Line], reread: impl Fn(i32) -> Option<u8>) -> Vec<Option<u8>> {
    let mut sorted: Vec<&Line> = lines
        .iter()
        .filter(|l| {
            let s = squash(&l.text);
            s.starts_with("go") || s.starts_with("to") || s.contains("cel") || door(&l.text).is_some()
        })
        .collect();
    sorted.sort_by_key(|l| l.rect.centre().0);
    let mut cards: Vec<Vec<&Line>> = Vec::new();
    for l in sorted {
        match cards.last_mut() {
            Some(card) if l.rect.centre().0 - card[0].rect.centre().0 < CARD_PITCH / 2 => card.push(l),
            _ => cards.push(vec![l]),
        }
    }
    cards
        .into_iter()
        .map(|mut card| {
            card.sort_by_key(|l| l.rect.y);
            door(&joined(card.iter().copied())).or_else(|| reread(card[0].rect.centre().0))
        })
        .collect()
}

fn joined<'a>(lines: impl IntoIterator<Item = &'a Line>) -> String {
    lines.into_iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join(" ")
}

/// The card centred near `x` read alone at larger scales: the card under the
/// mouse is raised and dimmed, and its "Cell 11" reads as "celV1" or "celgl"
/// at the row's 1.5x but as "Cell 11" at 2.5x or 3x.
fn reread(img: &Rgba, x: i32) -> Option<u8> {
    let rect = Rect::new(x - CARD_PITCH / 2, 840, CARD_PITCH - 15, 80);
    [2.5, 3.0].iter().find_map(|&scale| door(&joined(&ocr(img, rect, scale))))
}

/// The Interrogation Room's cards: both titles, Instigate left of Bestow.
fn interrogation(lines: &[Line]) -> bool {
    let at = |name: &str| lines.iter().find(|l| similarity(&l.text, name) >= 0.75).map(|l| l.rect.x);
    matches!((at("Instigate"), at("Bestow")), (Some(a), Some(b)) if a < b)
}

fn buttons(lines: &[Line]) -> Vec<&'static str> {
    let mut found: Vec<(i32, &'static str)> = Vec::new();
    for l in lines {
        if let Some((i, _)) = best_match(&l.text, &["Consume", "Replace", "End"], 0.65) {
            let name = ["Consume", "Replace", "End"][i];
            if !found.iter().any(|f| f.1 == name) {
                found.push((l.rect.x, name));
            }
        }
    }
    found.sort_by_key(|f| f.0);
    found.into_iter().map(|f| f.1).collect()
}

/// Card names on a pick screen: lines that match a name, left to right.
fn offers(lines: &[Line], names: &[String]) -> Vec<usize> {
    let mut found: Vec<(i32, usize)> = Vec::new();
    for l in lines {
        if let Some((i, _)) = best_match(&l.text, names, 0.75)
            && !found.iter().any(|f| f.1 == i)
        {
            found.push((l.rect.x, i));
        }
    }
    found.sort_by_key(|f| f.0);
    found.into_iter().map(|f| f.1).collect()
}

pub(crate) fn luma_stats(img: &Rgba, (cx, cy): (i32, i32), half: i32) -> (f32, f32) {
    let mut v = Vec::with_capacity(((2 * half) * (2 * half)) as usize);
    for y in cy - half..cy + half {
        for x in cx - half..cx + half {
            let [r, g, b] = img.rgb(x, y);
            v.push(0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32);
        }
    }
    let mean = v.iter().sum::<f32>() / v.len() as f32;
    let var = v.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / v.len() as f32;
    (mean, var.sqrt())
}

/// Mean luma over each Chromatic Prisoner slot, in fill order.
pub fn slot_scores(img: &Rgba) -> Vec<f32> {
    SLOT_Y.iter().flat_map(|&y| SLOT_X.map(|x| luma_stats(img, (x, y), 22).0)).collect()
}

/// Luma spread over the first instrument icon spot.
pub fn icon_score(img: &Rgba) -> f32 {
    luma_stats(img, ICON, 22).1
}

/// The panels of a frame whose two layouts are the same, as at interface
/// scale 1 (the tests' saved frames); see [`read_panels_titled`].
pub fn read_panels(img: &Rgba) -> Panels {
    read_panels_titled(img, img)
}

/// The panels of `img`, a frame in the board layout, with [`TITLE`],
/// [`INCIDENT`] and [`RIOT`] read from `title`, the same frame in the title layout (see
/// [`super::layout::lay_out_scaled`]).
pub fn read_panels_titled(img: &Rgba, title: &Rgba) -> Panels {
    let d = data();
    // Every fixed region is read at once, whether the screen needs it or not.
    const REGIONS: [(Rect, f32); 14] = [DAYS, TITLE, HEADING, CARDS, TARGET, CANCEL, BANNER, DIALOG, INCIDENT, THEME, OBJECTIVE, STATUS, RIOT, NAMES];
    let mut imgs = [img; 14];
    for i in [1, 8, 12] {
        imgs[i] = title;
    }
    let [days_lines, title, heading, cards, target, cancel, banner, dialog, incident, theme_lines, objective_lines, status, riot, name_lines] =
        ocr_all(imgs, &REGIONS);
    let mut p = Panels {
        screen: Screen::Unknown,
        days: days(&days_lines),
        doors: Vec::new(),
        decision: Vec::new(),
        instruments: Vec::new(),
        vault: Vec::new(),
        tool_icons: icon_score(img) >= ICON_SPREAD,
        theme: None,
        objective: None,
        held: None,
    };
    if has(&title, "Select a Torture Instrument", 0.75) {
        p.screen = Screen::InstrumentPick;
        let names: Vec<String> = d.tool_ids.iter().map(|&t| d.tool_names[t as usize].clone()).collect();
        p.instruments = offers(&name_lines, &names).into_iter().map(|i| d.tool_ids[i]).collect();
    } else if has(&title, "Warden's Treasure", 0.75) || title.iter().any(|l| squash(&l.text).contains("treasure")) {
        p.screen = Screen::TreasurePick;
    } else if has(&status, "Return to Selection", 0.75) {
        p.screen = Screen::PrisonStatus;
    } else if has(&target, "Select an Inmate to Consume", 0.75) || has(&cancel, "Cancel", 0.8) {
        p.screen = Screen::ChainTarget;
        p.decision = vec!["Cancel"];
    } else if let Some(id) = theme(&theme_lines) {
        p.screen = Screen::Theme;
        p.theme = Some(id);
    } else if popup(&dialog, &incident) {
        p.screen = Screen::Popup;
    } else {
        p.decision = buttons(&cards);
        let doors = doors(&cards, |x| reread(img, x));
        if has(&heading, "Select an Objective", 0.75) {
            p.screen = Screen::CellPick;
        } else if interrogation(&cards) {
            // Its heading is "Make Your Decision", as on the manage step.
            p.screen = Screen::Interrogation;
            p.decision.clear();
        } else if has(&heading, "Select a Destination", 0.75) || !doors.is_empty() {
            p.screen = Screen::Patrol;
            p.doors = doors;
            p.decision.clear();
        } else if has(&heading, "Make Your Decision", 0.75) || !p.decision.is_empty() {
            p.screen = Screen::Manage;
        } else if banner.iter().any(|l| squash(&l.text).starts_with("day")) {
            p.screen = Screen::DayBanner;
        } else if has(&riot, "A Riot has broken out in Tartarus", 0.75) {
            p.screen = Screen::RunEnd;
        }
    }
    // The Consume target pick covers the panel.
    if matches!(p.screen, Screen::Patrol | Screen::Manage | Screen::CellPick | Screen::Interrogation) {
        p.held = read_held(img);
    }
    let pick_or_end = matches!(p.screen, Screen::InstrumentPick | Screen::TreasurePick | Screen::RunEnd);
    if !pick_or_end && p.screen != Screen::Theme {
        p.objective = objective(&objective_lines);
    }
    // The riot screen shows no Chromatic Prisoner slots.
    if !pick_or_end {
        for (k, s) in slot_scores(img).into_iter().enumerate() {
            if s < SLOT_FILLED {
                break;
            }
            let (x, y) = (SLOT_X[k % 2], SLOT_Y[k / 2]);
            let (beast, conf) = beast_at(img, (x, y));
            p.vault.push(VaultSlot { beast, conf });
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_names() {
        let names = ["Ferocious Beast's Ravenous Chains", "Scapegoat's Guillotine", "Kin's Coin of Fate", "Forged Key"];
        assert_eq!(best_match("Ferocious Beast's Ravenous Chains", &names, 0.75), Some((0, 1.0)));
        assert_eq!(best_match("ferocious beasts ravenous chalns", &names, 0.75).map(|m| m.0), Some(0));
        assert_eq!(best_match("Scapegoat'sGuillotlne", &names, 0.75).map(|m| m.0), Some(1));
        assert_eq!(best_match("Kin 's Coin of Fale", &names, 0.75).map(|m| m.0), Some(2));
        assert_eq!(best_match("Randomly removes 2 other", &names, 0.75), None);
        assert_eq!(door("Go to Celi9."), Some(9));
        assert_eq!(door("Go to Imeycogation Room"), Some(1));
        assert_eq!(door("Go to Cell 16"), Some(16));
        assert_eq!(number("O"), Some(0));
    }

    #[test]
    fn objective_progress_parses() {
        assert_eq!(progress("0/2", 2), Some(0));
        assert_eq!(progress("times0/2", 2), Some(0));
        assert_eq!(progress("O/18", 18), Some(0));
        assert_eq!(progress("Trigger ! Prison Incident 2 times 1/2", 2), Some(1));
        assert_eq!(progress("Completed", 4), Some(4));
        assert_eq!(progress("Move 18 tiles Completed", 18), Some(18));
        assert_eq!(progress("UltimateCompleted", 1), Some(1));
        assert_eq!(progress("Move 18 tiles 7/18", 18), Some(7));
        assert_eq!(progress("Complete 2 objectives 1/2", 2), Some(1));
        assert_eq!(progress("Consume 4 times", 4), None);
        assert_eq!(progress("3/2", 2), None);
    }

    #[test]
    fn cards_by_position() {
        let line = |text: &str, x: i32, y: i32| Line { text: text.to_string(), rect: Rect::new(x, y, 160, 30) };
        // Three cards, the room on two lines.
        let three = [line("Go to Cell 7", 535, 860), line("Go to Interrogation", 880, 845), line("Room", 920, 875), line("Go to Cell 12", 1225, 860)];
        assert_eq!(doors(&three, |_| None), [Some(7), Some(1), Some(12)]);
        // Two cards centred between the three-card columns (Burning Stake).
        let two = [line("Go to Cell 5", 1060, 860), line("Go to Cell 2", 710, 860)];
        assert_eq!(doors(&two, |_| None), [Some(2), Some(5)]);
        // OCR of a live frame at the reader's scale: junk before "Go", 11 read as IL.
        let live = [line("so to Cell 6", 710, 869), line("eGo to Cell IL", 1057, 869)];
        assert_eq!(doors(&live, |_| None), [Some(6), Some(11)]);
        // The card under the mouse: "Go" dropped and "Cell 11" garbled; it is read again.
        let hovered = [line("to Cell 8", 588, 869), line("to celgl", 927, 869), line("Go to Cell 13", 1234, 869)];
        assert_eq!(doors(&hovered, |x| (x == 1007).then_some(11)), [Some(8), Some(11), Some(13)]);
    }

    #[test]
    fn interrogation_cards() {
        let line = |text: &str, x: i32| Line { text: text.to_string(), rect: Rect::new(x, 862, 110, 30) };
        assert!(interrogation(&[line("Instigate", 738), line("Bestow", 1090)]));
        assert!(interrogation(&[line("lnstigate", 738), line("Bestaw", 1090)]));
        assert!(!interrogation(&[line("Instigate", 738)]));
        assert!(!interrogation(&[line("Consume", 600), line("Replace", 900), line("End", 1200)]));
    }

    #[test]
    fn door_numbers() {
        assert_eq!(door("Go to Cell lO"), Some(10));
        assert_eq!(door("Go to Cell 16"), Some(16));
        // The "l" of "Cell" is not a 1.
        assert_eq!(door("Go to Cell"), None);
        assert_eq!(door("Go to Interrogation Room"), Some(1));
    }

    #[test]
    fn days_counter() {
        let read = |text: &str| days(&[Line { text: text.to_string(), rect: Rect::new(844, 11, 235, 33) }]);
        assert_eq!(read("12 day(s) until Riot"), Some(12));
        assert_eq!(read("1 day(s) until Riot"), Some(1));
        // The red 0 on the last day, as Windows OCR read it in a recording.
        assert_eq!(read("() day(S) untii Riot"), Some(0));
        assert_eq!(read("0day(s) until Riot"), Some(0));
        assert_eq!(read("Oday(s) until Riot"), Some(0));
        assert_eq!(read("O day(s) until Riot"), Some(0));
        assert_eq!(read("1 0 day(s) until Riot"), Some(10));
        assert_eq!(read("13 dav(s) until Riot"), Some(13));
        assert_eq!(read("day(s) until Riot"), None);
        assert_eq!(read("Tartarus Prison"), None);
        assert_eq!(days(&[]), None);
    }

    #[test]
    fn dialogs() {
        let lines = |texts: &[&str]| texts.iter().map(|t| Line { text: t.to_string(), rect: Rect::new(560, 561, 800, 34) }).collect::<Vec<_>>();
        assert!(popup(&lines(&["Tip", "Network request timeout. Please try again later.(5684)"]), &[]));
        assert!(popup(&lines(&["Connecting to server..."]), &[]));
        assert!(popup(&[], &lines(&["Behind Closed Doors"])));
        assert!(!popup(&lines(&["Multiple"]), &lines(&["Warden"])));
        assert!(!popup(&[], &[]));
        assert_eq!(theme(&lines(&["Opulence"])), Some(302));
        assert_eq!(theme(&lines(&["Cage Fiqht"])), Some(304));
        assert_eq!(theme(&lines(&["Warden"])), None);
    }
}
