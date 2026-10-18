//! The text helpers of js/engine.js (an, plural, tierName, cellLabel,
//! describe, toolName) with the in-game names from the data.

use super::data::{Card, Gift, RAINBOW, Tool, data};
use super::state::Inmate;

/// an: "an" before a vowel, else "a".
pub fn an(text: &str) -> String {
    let vowel = text.chars().next().is_some_and(|c| "AEIOUaeiou".contains(c));
    format!("{} {text}", if vowel { "an" } else { "a" })
}

pub fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

pub fn tier_name(tier: i32) -> &'static str {
    &data().tier_names[tier.clamp(0, RAINBOW) as usize]
}

/// cellLabel: cell 0 is "01".
pub fn cell_label(n: usize) -> String {
    format!("{:02}", n + 1)
}

pub fn describe(inmate: &Inmate) -> String {
    format!("{} {}", tier_name(inmate.tier), data().beast_names[inmate.beast.index()])
}

/// toolName: the name of whatever a pending choice offers by key (an
/// instrument, a gift or a card), or the key itself.
pub fn tool_name(key: &str) -> String {
    let d = data();
    if let Some(t) = Tool::from_key(key) {
        return d.tool_names[t as usize].clone();
    }
    if let Some(g) = Gift::from_key(key) {
        return d.gift_names[g as usize].clone();
    }
    if let Some(c) = Card::from_key(key) {
        return d.card_names[c as usize].clone();
    }
    key.to_string()
}

pub fn incident_name(id: u32) -> &'static str {
    &data().incident(id).name
}
