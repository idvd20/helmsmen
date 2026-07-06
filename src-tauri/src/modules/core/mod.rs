//! Helmsmen pure core (functional core, imperative shell).
//!
//! Everything in this module is `data in -> data out`: entities plus
//! `apply(state, event) -> state` transitions. RULES (enforced by a guard
//! test in `modules::registry`): no PTY, no async, no HTTP, no filesystem,
//! no process spawning, no Tauri imports. The imperative shell —
//! persistence, git detection, Tauri command glue — lives in
//! `modules::registry` and stays thin.

pub mod cut;
pub mod profile;
pub mod project;
pub mod session;
pub mod settings;
pub mod state;
pub mod workspace;
