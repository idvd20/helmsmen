//! Helmsmen control-plane hooks (new module, task #15) — the imperative
//! shell for the M3 control plane.
//!
//! A per-Workspace, loopback-only HTTP endpoint receives Claude Code hook
//! events (PreToolUse / Notification / PostToolUse / Stop), authenticates
//! them against a per-session bearer token, caps their size, and parses the
//! hostile JSON body into the typed events the pure core
//! (`modules::core::control_plane`) reduces into Approval Inbox cards and
//! Session status. This is the M3 replacement SOURCE for the M2
//! agent-signal (`modules::harness::agent_signal`); both feed the same
//! pure-core status reducer during the swap.
//!
//! Split:
//! - [`wire`] — pure request handling (token check, size cap, typed parse).
//!   Zero I/O, so every rejection is unit-tested.
//! - [`server`] — the `std::net` loopback listener that reads sockets, calls
//!   [`wire::handle_request`], folds accepted events into the pure core, and
//!   answers with terse HTTP. The only place with the network.
//!
//! Security invariants (PRD, verbatim): the endpoint binds loopback only; a
//! per-session bearer token is required; payloads are typed-parsed and
//! size-capped; bad/missing tokens and oversized bodies are rejected; a hook
//! payload is DATA, never an instruction; an event may change state but
//! never executes anything. The HTTP server lives here, never in the pure
//! core — a guard test in `modules::registry` enforces that
//! `modules::core` imports no network, async, or process code.

pub mod server;
pub mod wire;

pub use server::ControlPlaneEndpoint;
pub use wire::{handle_request, parse_hook_event, Outcome, Rejection, RequestParts, MAX_BODY_BYTES};
