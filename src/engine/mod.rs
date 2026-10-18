//! The rules engine, a port of js/engine.js. Every random roll draws from the
//! state's seeded stream in the same order as the JS, so a seed replays the
//! same run. Lab editing and the Lab-only scenarios are not ported.

pub mod actions;
pub mod data;
pub mod devour;
pub mod gifts;
pub mod incidents;
pub mod rng;
pub mod rules;
pub mod state;
pub mod text;
pub mod tools;

pub use actions::{Action, ChoiceId, Outcome, apply};
pub use rules::{new_game, new_game_with};
pub use state::State;
