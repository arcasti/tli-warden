//! Game data from data/warden-data.json and the tables js/engine.js derives
//! from it (lines 18-310). Parsed once.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::sync::OnceLock;

const RAW: &str = include_str!("../../data/warden-data.json");

/// Chromatic, the top tier; an inmate reaching it leaves for the vault.
pub const RAINBOW: i32 = 5;
/// The vault holds at most this many Chromatics (confirmed in-game).
pub const VAULT_CAP: u32 = 10;
/// What one inmate left at the Riot pays, in Chromatics, by tier (Normal,
/// Magic, Rare, Legendary, Ultimate, Chromatic). The game pays a Warden's
/// Cache per inmate left in Tartarus by rarity and one per vaulted
/// Chromatic; the worth of each cache against a Chromatic is set by the
/// player.
pub const LOOT: [f64; 6] = [0.0, 0.0, 0.0, 0.05, 0.2, 1.0];
/// Relative worth of one inmate per tier; the advisor ranks moves with it.
pub const WORTH: [f64; 6] = [1.0, 3.0, 9.0, 27.0, 81.0, 400.0];

/// In-game species in the order of the data's species list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Beast {
    Mammoth,
    Lion,
    Wolf,
    Rat,
}

impl Beast {
    pub const ALL: [Beast; 4] = [Beast::Mammoth, Beast::Lion, Beast::Wolf, Beast::Rat];

    pub fn index(self) -> usize {
        self as usize
    }
}

macro_rules! keyed_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident = $key:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn key(self) -> &'static str {
                match self {
                    $($name::$variant => $key),+
                }
            }

            pub fn from_key(key: &str) -> Option<$name> {
                match key {
                    $($key => Some($name::$variant),)+
                    _ => None,
                }
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.key())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let key = String::deserialize(d)?;
                $name::from_key(&key).ok_or_else(|| serde::de::Error::custom(format!("unknown {}: {key}", stringify!($name))))
            }
        }
    };
}

keyed_enum! {
    /// Torture Instruments by the engine's short ids (TOOL_KEYS).
    Tool {
        Muzzle = "muzzle", Chains = "chains", Collar = "collar", Revenge = "revenge", Beastcage = "beastcage",
        Whip = "whip", Saw = "saw", Blacklist = "blacklist", Keys = "keys", Coin = "coin", Fire = "fire",
        Dogtag = "dogtag", Forgedkey = "forgedkey", Hook = "hook", Scapegoat = "scapegoat", Axe = "axe",
        Maiden = "maiden", Scales = "scales", Rope = "rope", Stake = "stake", Pestle = "pestle",
        Shortchain = "shortchain", Longchain = "longchain", Tongs = "tongs", Maul = "maul", Trap = "trap",
        Crown = "crown", Claws = "claws", Guillotine = "guillotine", Ironbox = "ironbox", Skewer = "skewer",
    }
}

keyed_enum! {
    /// The Warden's Treasure rewards (GIFT_ROWS), in GIFT_IDS order.
    Gift { Ring = "ring", Horn = "horn", Wine = "wine", Brand = "brand" }
}

keyed_enum! {
    /// The Interrogation Room's two cards (INTERROGATION_IDS).
    Card { Instigate = "instigate", Bestow = "bestow" }
}

impl Tool {
    /// The data's instrument id (TOOL_KEYS).
    pub fn rl(self) -> u32 {
        const RL: [u32; 31] = [
            101, 102, 103, 104, 105, 106, 107, 111, 112, 113, 114, 115, 116, 117, 121, 122, 123, 124, 125, 126, 127, 131,
            132, 141, 142, 143, 144, 145, 146, 147, 148,
        ];
        RL[self as usize]
    }

    pub fn from_rl(rl: u32) -> Option<Tool> {
        Tool::ALL.iter().copied().find(|t| t.rl() == rl)
    }
}

/// One instrument: data values plus the numbers TOOL_RULES reads from its text.
#[derive(Clone, Debug, Default)]
pub struct ToolDef {
    pub weight: u32,
    pub exclude: Vec<Tool>,
    /// chancePercent; JS compares against null as 0.
    pub chance: f64,
    pub tiers: Option<i32>,
    pub fewer_dice: i32,
    pub count: Option<usize>,
    pub max_inmates: Option<usize>,
    pub max_sealed: Option<usize>,
    pub min_others: Option<usize>,
    pub max_steps: Option<usize>,
    pub min_steps: Option<usize>,
    pub above: Option<i32>,
    pub tier: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spawn {
    Any,
    Unopened,
    Opened,
}

#[derive(Clone, Debug)]
pub struct Incident {
    pub id: u32,
    pub name: String,
    pub weight: u32,
    pub spawn: Spawn,
    /// excludeRelic, as held instruments.
    pub exclude: Vec<Tool>,
    /// 0 for no limit.
    pub repeat_limit: u32,
    /// Counts, tiers, chance and rarity the effect text states (0 when it
    /// states none); `count` is usize::MAX for "all Inmates".
    pub count: usize,
    pub tiers: i32,
    pub chance: f64,
    pub tier: i32,
    /// The Usurpation stage the text adds to the pool.
    pub next: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectiveKind {
    ReachRating,
    DevourCount,
    OpenCellCount,
    EventTriggerCount,
    MoveDistance,
}

#[derive(Clone, Debug)]
pub struct Objective {
    pub id: u32,
    /// The objective panel's title: "Objective: Undercurrents".
    pub name: String,
    pub kind: ObjectiveKind,
    pub target: u32,
    /// The rarity reach_rating names; JS compares tiers against null as 0.
    pub rating: i32,
    pub weight: u32,
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub id: u32,
    pub name: String,
    pub weight: u32,
    pub value: Option<i64>,
}

pub const TYRANNY: u32 = 301;
pub const OPULENCE: u32 = 302;
pub const IRON_LAW: u32 = 303;
pub const CAGE_FIGHT: u32 = 304;

pub struct Data {
    pub cell_count: usize,
    pub room: usize,
    pub dice_options: u32,
    pub min_pips: u32,
    pub max_pips: u32,
    pub days: u32,
    pub open_rooms_at_start: usize,
    pub instrument_picks: usize,
    pub objectives_per_run: u32,
    pub min_markers: usize,
    pub theme_chance_per_10000: f64,
    pub theme_pity_runs: u32,
    /// Rarity draw weights for Normal to Ultimate.
    pub tier_weights: [u32; RAINBOW as usize],
    pub incident_spawn_chance: u32,
    pub oppress_per_tier: f64,
    /// Who each beast eats (FOOD_CHAIN), by Beast index.
    pub food_chain: [Vec<Beast>; 4],
    /// Instruments in data order (TOOL_IDS); index with `tool_ids` lookups.
    pub tool_ids: Vec<Tool>,
    tools: Vec<ToolDef>,
    pub incidents: Vec<Incident>,
    pub objectives: Vec<Objective>,
    pub themes: Vec<Theme>,
    /// In-game names: rarities Normal to Chromatic (TIERS), species by Beast
    /// index, instruments by Tool, gifts by Gift, the cards by Card, and Hunt.
    pub tier_names: Vec<String>,
    pub beast_names: [String; 4],
    pub tool_names: Vec<String>,
    pub gift_names: Vec<String>,
    pub card_names: Vec<String>,
    pub hunt_name: String,
    /// Glossary names: Prison Incident, Torture Instrument, Warden's Treasure.
    pub incident_term: String,
    pub instrument_name: String,
    pub treasure_name: String,
}

impl Data {
    pub fn tool(&self, t: Tool) -> &ToolDef {
        &self.tools[t as usize]
    }

    pub fn incident(&self, id: u32) -> &Incident {
        self.incidents.iter().find(|e| e.id == id).expect("incident id")
    }

    pub fn objective(&self, id: u32) -> Option<&Objective> {
        self.objectives.iter().find(|o| o.id == id)
    }

    pub fn theme(&self, id: u32) -> Option<&Theme> {
        self.themes.iter().find(|t| t.id == id)
    }

    /// The name of `theme`, an id of `themes`.
    pub fn theme_name(&self, theme: Option<u32>) -> Option<&str> {
        theme.and_then(|id| self.theme(id)).map(|t| t.name.as_str())
    }
}

pub fn data() -> &'static Data {
    static DATA: OnceLock<Data> = OnceLock::new();
    DATA.get_or_init(|| load(RAW))
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or_else(|| panic!("number expected, got {v}"))
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_else(|| panic!("string expected, got {v}")).to_string()
}

fn uint(v: &Value) -> u32 {
    num(v) as u32
}

/// The `{ value }` wrapper the data puts on text-read numbers.
fn wrapped(v: &Value) -> Option<f64> {
    v.get("value").and_then(Value::as_f64)
}

/// textNumber: the first `before<digits>after` in the instrument's texts.
fn text_number(r: &Value, before: &str, after: &str) -> i32 {
    let text = ["triggerPlain", "plain"].iter().filter_map(|k| r[*k].as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
    let mut from = 0;
    while from < text.len() {
        let Some(at) = text[from..].find(before) else { break };
        let start = from + at + before.len();
        let digits: String = text[start..].chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() && text[start + digits.len()..].starts_with(after) {
            return digits.parse().expect("digits");
        }
        from += at + text[from + at..].chars().next().map_or(1, char::len_utf8);
    }
    panic!("{}: no match for {before}(\\d+){after}", r["Id"]);
}

/// The rarity a text names after `before` (a `(\w+)` in the JS patterns).
fn rarity_after(r: &Value, before: &str, names: &[&str]) -> i32 {
    let plain = r["plain"].as_str().unwrap_or("");
    let at = plain.find(before).unwrap_or_else(|| panic!("{}: no {before:?}", r["Id"])) + before.len();
    let name: String = plain[at..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    names.iter().position(|x| *x == name).unwrap_or_else(|| panic!("{}: rarity {name}", r["Id"])) as i32
}

/// The numbers an incident's effect reads from its text (INCIDENT_EFFECTS,
/// USURPATION_STEPS); the caller fills the other fields.
fn incident_rules(e: &Value, all: &[Value], names: &[&str]) -> Incident {
    let n = |before: &str, after: &str| text_number(e, before, after);
    let plain = e["plain"].as_str().unwrap_or("");
    let mut x =
        Incident { id: 0, name: String::new(), weight: 0, spawn: Spawn::Any, exclude: Vec::new(), repeat_limit: 0, count: 0, tiers: 0, chance: 0.0, tier: 0, next: None };
    match uint(&e["Id"]) {
        201 => x.tier = rarity_after(e, "with a ", names),
        202 => x.count = n("open ", " cells") as usize,
        203 => x.tier = rarity_after(e, "rarity to ", names),
        211 => {
            x.count = n("make ", " consecutive attempts") as usize;
            x.chance = f64::from(n("", "% chance"));
        }
        212 => {
            x.count = n("up to ", " Inmates") as usize;
            x.tiers = n("by ", " tier");
        }
        213 => x.tiers = n("by ", " tier"),
        231 => x.count = n("next ", " unopened") as usize,
        232 => x.tier = rarity_after(e, "rarity below ", names),
        252..=255 => {
            x.count = if plain.contains("Increase all Inmates") { usize::MAX } else { n("increase ", " Inmate") as usize };
            x.tiers = n("by ", " tier");
            x.next = plain.find("Add [").map(|at| {
                let rest = &plain[at + "Add [".len()..];
                let name = &rest[..rest.find(']').expect("usurpation next")];
                uint(&all.iter().find(|o| o["name"] == name).expect("usurpation stage")["Id"])
            });
        }
        _ => {}
    }
    x
}

fn load(raw: &str) -> Data {
    let d: Value = serde_json::from_str(raw).expect("warden-data.json");
    let board = &d["board"];
    let rarities = d["rarities"].as_array().expect("rarities");
    let rarity_names: Vec<&str> = rarities.iter().map(|r| r["name"].as_str().expect("rarity name")).collect();
    let species = d["species"].as_array().expect("species");
    let glossary = |id: u32| text(&d["glossary"].as_array().expect("glossary").iter().find(|g| uint(&g["Id"]) == id).expect("glossary id")["name"]);
    let beast_of = |id: &Value| Beast::ALL[species.iter().position(|s| s["Id"] == *id).expect("species id")];
    let food_chain: [Vec<Beast>; 4] = std::array::from_fn(|i| species[i]["counters"].as_array().expect("counters").iter().map(beast_of).collect());

    let mut tools = vec![ToolDef::default(); Tool::ALL.len()];
    let mut tool_names = vec![String::new(); Tool::ALL.len()];
    let mut tool_ids = Vec::new();
    for r in d["instruments"].as_array().expect("instruments") {
        let Some(tool) = Tool::from_rl(uint(&r["Id"])) else { continue };
        let def = &mut tools[tool as usize];
        tool_names[tool as usize] = text(&r["name"]);
        tool_ids.push(tool);
        def.weight = uint(&r["Weight"]);
        def.exclude = r["exclude"].as_array().map(|a| a.iter().filter_map(|x| Tool::from_rl(uint(x))).collect()).unwrap_or_default();
        def.chance = wrapped(&r["chancePercent"]).unwrap_or(0.0);
        def.tiers = wrapped(&r["extraTiers"]).map(|x| x as i32);
        let n = |before: &str, after: &str| text_number(r, before, after);
        match tool {
            Tool::Muzzle => def.max_inmates = Some(n("no more than ", " Inmates") as usize),
            Tool::Chains | Tool::Dogtag => def.tiers = Some(n("+", " Rarity Tier")),
            Tool::Whip => def.fewer_dice = 1,
            Tool::Blacklist => {
                def.max_sealed = Some(n("no more than ", " unopened") as usize);
                def.count = Some(n(" Rarity Tier to ", " random") as usize);
                def.tiers = Some(n("+", " Rarity Tier to"));
            }
            Tool::Coin => def.tiers = Some(n("+", " Rarity Tiers")),
            Tool::Forgedkey => {
                def.fewer_dice = 1;
                def.count = Some(n("opens ", " additional") as usize);
            }
            Tool::Scapegoat => {
                def.min_others = Some(n("at least ", " other") as usize);
                def.count = Some(n("removes ", " other") as usize);
            }
            Tool::Axe => def.count = Some(n("opens ", " cells") as usize),
            Tool::Stake => {
                def.fewer_dice = 1;
                def.tier = Some(rarity_after(r, "rarity to ", &rarity_names));
            }
            Tool::Shortchain => {
                def.max_steps = Some(n("no more than ", " tiles") as usize);
                def.count = Some(n("opens ", " cell") as usize);
            }
            Tool::Longchain => {
                def.min_steps = Some(n("", " or more tiles") as usize);
                def.tiers = Some(n("+", " Rarity Tier"));
            }
            Tool::Maul => def.above = Some(n("+", " tiers above")),
            _ => {}
        }
    }

    let incident_list = d["incidents"].as_array().expect("incidents");
    let incidents = incident_list
        .iter()
        .map(|e| Incident {
            id: uint(&e["Id"]),
            name: text(&e["name"]),
            weight: uint(&e["weight"]),
            spawn: match e["spawnCellEnum"].as_str() {
                Some("any") => Spawn::Any,
                Some("unopened") => Spawn::Unopened,
                Some("opened") => Spawn::Opened,
                other => panic!("spawnCellEnum {other:?}"),
            },
            exclude: e["excludeRelic"].as_array().map(|a| a.iter().filter_map(|x| Tool::from_rl(uint(x))).collect()).unwrap_or_default(),
            // Number(repeatLimit): "" and missing read as no limit.
            repeat_limit: match &e["repeatLimit"] {
                Value::Number(x) => x.as_f64().unwrap_or(0.0).max(0.0) as u32,
                Value::String(s) => s.trim().parse::<f64>().unwrap_or(0.0).max(0.0) as u32,
                _ => 0,
            },
            ..incident_rules(e, incident_list, &rarity_names)
        })
        .collect();

    let objectives = d["objectives"]
        .as_array()
        .expect("objectives")
        .iter()
        .map(|o| {
            let kind = match o["taskTypeEnum"].as_str() {
                Some("reach_rating") => ObjectiveKind::ReachRating,
                Some("devour_count") => ObjectiveKind::DevourCount,
                Some("open_cell_count") => ObjectiveKind::OpenCellCount,
                Some("event_trigger_count") => ObjectiveKind::EventTriggerCount,
                Some("move_distance") => ObjectiveKind::MoveDistance,
                other => panic!("taskTypeEnum {other:?}"),
            };
            let (target, rating) = if kind == ObjectiveKind::ReachRating {
                let rating = rarities.iter().position(|r| r["Id"] == o["target"]).expect("objective rarity");
                (1, rating as i32)
            } else {
                (uint(&o["target"]), 0)
            };
            Objective { id: uint(&o["Id"]), name: text(&o["name"]), kind, target, rating, weight: uint(&o["weight"]) }
        })
        .collect();

    let themes = d["themes"]["relics"]
        .as_array()
        .expect("themes")
        .iter()
        .map(|t| Theme { id: uint(&t["Id"]), name: t["name"].as_str().expect("theme name").to_string(), weight: uint(&t["weight"]), value: wrapped(&t["value"]).map(|x| x as i64) })
        .collect();

    Data {
        cell_count: uint(&board["cells"]) as usize,
        room: uint(&board["interrogationRoom"]["cell"]) as usize,
        dice_options: uint(&board["dice"]["options"]),
        min_pips: uint(&board["dice"]["minPips"]),
        max_pips: uint(&board["dice"]["maxPips"]),
        days: uint(&board["days"]),
        open_rooms_at_start: uint(&board["openRoomsAtStart"]) as usize,
        instrument_picks: uint(&board["instrumentPicks"]) as usize,
        objectives_per_run: uint(&board["objectivesPerRun"]),
        min_markers: uint(&board["minIncidentMarkers"]) as usize,
        theme_chance_per_10000: num(&board["specialTheme"]["chancePer10000"]),
        theme_pity_runs: uint(&board["specialTheme"]["pityRuns"]),
        tier_weights: std::array::from_fn(|t| uint(&rarities[t]["weight"])),
        incident_spawn_chance: (num(&d["practice"]["incidentSpawnRate"]["value"]) * 10000.0).round() as u32,
        oppress_per_tier: num(&d["oppress"]["penaltyPerTierPercent"]["value"]),
        food_chain,
        tool_ids,
        tools,
        incidents,
        objectives,
        themes,
        tier_names: rarity_names.iter().map(|n| n.to_string()).collect(),
        beast_names: std::array::from_fn(|i| text(&species[i]["name"])),
        tool_names,
        gift_names: Gift::ALL
            .iter()
            .map(|&g| {
                let row = g as u32 + 1;
                text(&d["chromaticRewards"].as_array().expect("chromaticRewards").iter().find(|r| uint(&r["Id"]) == row).expect("gift row")["name"])
            })
            .collect(),
        card_names: Card::ALL.iter().map(|c| text(&board["interrogationRoom"]["cards"][c.key()]["name"])).collect(),
        hunt_name: text(&board["actions"]["hunt"]["name"]),
        incident_term: glossary(16000103),
        instrument_name: glossary(16000105),
        treasure_name: glossary(16000118),
    }
}
