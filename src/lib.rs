//! vyges-hold-fix — post-route hold-fix ECO.
//!
//! The hold counterpart to the Vyges "close-timing" engines (`vyges-resize` drive strength,
//! `vyges-vt-swap` threshold voltage, `vyges-buffer-insert` transition/fanout — all of which
//! target the *late* / setup corner). Hold violations are the opposite problem: data reaches
//! a capture flop *too early* on the min-delay path. This engine **adds delay** — it inserts
//! a delay cell in series on the net feeding each hold-violating capture pin, lifting that
//! pin's earliest arrival until its hold constraint is met.
//!
//! It reads the same per-endpoint timing the shared `vyges-sta-si` engine computes (now with
//! per-endpoint hold slacks), so it closes the gap left by the setup-side ECOs: a real
//! post-route netlist + Liberty [+ SPEF] in, a hold-fixed netlist + before/after WHS/WNS
//! report out. Because it changes topology, each round is scored by rebuilding the timer on
//! the mutated netlist (the incremental topology update is future work).

pub mod emit;
pub mod engine;
pub mod job;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const COPYRIGHT: &str = "© 2026 Vyges. All Rights Reserved.  https://vyges.com";
