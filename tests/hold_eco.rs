//! What the hold ECO must never get wrong.
//!
//! Hold fixing is a topology mutation on a signed-off netlist, so the failure modes are not
//! "a slightly worse number" — they are a design that reports clean and is broken. The
//! properties pinned here are, in order of how much damage getting them wrong would do:
//!
//!   * setup is never traded away to buy hold (the guard in `optimize`)
//!   * a design that already meets hold is left completely alone
//!   * every inserted buffer is actually wired in series — original driver into the buffer,
//!     buffer output into the capture pin — because a mis-wired ECO still emits valid Verilog
//!   * `dont_touch` is honoured
//!   * a bad request fails with a message instead of a panic
//!
//! The fixture is the two-flop case the engine was built for: a near-zero data path plus a
//! fast CK->Q, so the second flop captures its own launch data on the same edge.

use vyges_hold_fix::{engine, job};

/// Two flops, `q1` running straight from f1.Q into f2.D — a hold violation by construction.
const NL: &str = "module top ( clk, d, q ); input clk, d; output q; wire q1;\n\
                  DFF f1 ( .CK(clk), .D(d),  .Q(q1) );\n\
                  DFF f2 ( .CK(clk), .D(q1), .Q(q)  );\n\
                  endmodule";

/// A flop with a 0.2 ns hold requirement, a 0.02 ns CK->Q, and a 0.15 ns delay cell.
const LIB: &str = r#"
library (d) {
  cell (DFF) {
    ff (IQ, IQN) { clocked_on : "CK"; next_state : "D"; }
    pin (CK) { direction : input; clock : true; capacitance : 0.002; }
    pin (D)  { direction : input; capacitance : 0.002;
      timing () { related_pin : "CK"; timing_type : hold_rising;
        rise_constraint (t) { index_1 ("0.01,0.1"); index_2 ("0.01,0.1"); values ("0.20,0.20","0.20,0.20"); }
        fall_constraint (t) { index_1 ("0.01,0.1"); index_2 ("0.01,0.1"); values ("0.20,0.20","0.20,0.20"); } } }
    pin (Q)  { direction : output;
      timing () { related_pin : "CK"; timing_type : rising_edge;
        cell_rise (t) { index_1 ("0.01,0.1"); index_2 ("0.001,0.016"); values ("0.02,0.03","0.02,0.03"); }
        cell_fall (t) { index_1 ("0.01,0.1"); index_2 ("0.001,0.016"); values ("0.02,0.03","0.02,0.03"); }
        rise_transition (t) { index_1 ("0.01,0.1"); index_2 ("0.001,0.016"); values ("0.02,0.03","0.02,0.03"); }
        fall_transition (t) { index_1 ("0.01,0.1"); index_2 ("0.001,0.016"); values ("0.02,0.03","0.02,0.03"); } } }
  }
  cell (DLY) {
    pin (A) { direction : input; capacitance : 0.002; }
    pin (X) { direction : output;
      timing () { related_pin : "A"; timing_sense : positive_unate;
        cell_rise (t) { index_1 ("0.01,0.1"); index_2 ("0.001,0.016"); values ("0.15,0.16","0.15,0.16"); }
        cell_fall (t) { index_1 ("0.01,0.1"); index_2 ("0.001,0.016"); values ("0.15,0.16","0.15,0.16"); }
        rise_transition (t) { index_1 ("0.01,0.1"); index_2 ("0.001,0.016"); values ("0.02,0.03","0.02,0.03"); }
        fall_transition (t) { index_1 ("0.01,0.1"); index_2 ("0.001,0.016"); values ("0.02,0.03","0.02,0.03"); } } }
  }
}
"#;

const JOB: &str =
    "design: top\nnetlist: x\nlib: x\nclock: clk 5.0\ninput_slew: 0.02\noutput_load: 0.003\n";

fn run(cfg_text: &str) -> Result<engine::HoldResult, String> {
    let sta = vyges_sta_si::job::StaJob::parse(JOB, "").map_err(|e| e.to_string())?;
    let cfg = job::parse_cfg(cfg_text)?;
    engine::run_inputs(NL, LIB, &job::HoldJob { sta, cfg })
}

fn fix() -> engine::HoldResult {
    run("buffer: DLY\nhold_margin: 0.0\nrounds: high\n").expect("hold fix should run")
}

#[test]
fn the_fixture_really_does_violate_hold() {
    // If this ever stops being true the rest of the file is testing nothing, so assert it
    // rather than assume it.
    let r = fix();
    assert!(
        r.before_whs < 0.0,
        "fixture must start with negative hold slack, got {}",
        r.before_whs
    );
}

#[test]
fn inserting_delay_fixes_hold() {
    let r = fix();
    assert!(
        r.after_whs > r.before_whs,
        "WHS must improve: {} -> {}",
        r.before_whs,
        r.after_whs
    );
    assert!(
        r.after_whs >= -r.hold_margin,
        "hold should be met within margin {}, got WHS {}",
        r.hold_margin,
        r.after_whs
    );
    assert!(
        !r.inserted.is_empty(),
        "fixing hold requires inserting delay"
    );
}

/// The property that matters most. Buying hold with setup turns a hold violation into a
/// setup violation, which is not a fix — it is a different broken chip. `optimize` refuses a
/// round that pushes setup negative (or, on an already-violating setup, that worsens it).
#[test]
fn setup_is_never_traded_away_for_hold() {
    let r = fix();
    if r.before_wns >= 0.0 {
        assert!(
            r.after_wns >= -1e-9,
            "setup was positive ({}) and must not go negative to buy hold (got {})",
            r.before_wns,
            r.after_wns
        );
    } else {
        assert!(
            r.after_wns >= r.before_wns - 1e-9,
            "setup already violated ({}) and must not be made worse (got {})",
            r.before_wns,
            r.after_wns
        );
    }
}

/// A mis-wired ECO still emits syntactically valid Verilog, so the netlist parsing clean is
/// not evidence of anything. Check the actual series topology: the capture pin must be driven
/// by the buffer's output net, and the buffer's input must be the pin's original driver net.
#[test]
fn every_insertion_is_wired_in_series_on_the_capture_pin() {
    let r = fix();
    let v = &r.netlist_v;
    for ins in &r.inserted {
        assert_ne!(
            ins.in_net, ins.out_net,
            "a buffer whose input and output are the same net is a short, not a delay"
        );
        // the buffer instance exists, taking in_net and driving out_net
        let decl = format!(
            "{} {} ( .{}({}), .{}({}) );",
            ins.cell, ins.buffer, ins.in_pin, ins.in_net, ins.out_pin, ins.out_net
        );
        assert!(
            v.contains(&decl),
            "emitted netlist is missing the inserted buffer:\n  want: {decl}\n  got:\n{v}"
        );
    }
    // Each fixed capture pin is driven by the *last* buffer of its chain, not by every buffer
    // inserted for it — later rounds insert ahead of earlier ones. So check the terminus: for
    // every capture pin touched, some inserted buffer drives it.
    let mut caps: Vec<(&str, &str)> = r
        .inserted
        .iter()
        .map(|i| (i.cap_inst.as_str(), i.cap_pin.as_str()))
        .collect();
    caps.sort_unstable();
    caps.dedup();
    for (inst, pin) in caps {
        let driver = r
            .inserted
            .iter()
            .filter(|i| i.cap_inst == inst && i.cap_pin == pin)
            .find(|i| v.contains(&format!(".{}({})", pin, i.out_net)));
        assert!(
            driver.is_some(),
            "capture pin {inst}/{pin} was delayed but no inserted buffer drives it:\n{v}"
        );
    }
}

/// Delay accumulates by chaining, so one endpoint fixed over several rounds must form a real
/// series chain rather than several buffers all driving the same pin.
#[test]
fn repeated_rounds_chain_in_series_rather_than_stacking() {
    let r = fix();
    for ins in &r.inserted {
        let others: Vec<_> = r
            .inserted
            .iter()
            .filter(|o| o.buffer != ins.buffer && o.out_net == ins.out_net)
            .collect();
        assert!(
            others.is_empty(),
            "net {} is driven by more than one inserted buffer",
            ins.out_net
        );
    }
}

/// An endpoint whose capture instance is excluded must never be delayed, however bad its
/// slack. A dont_touch that is quietly ignored is worse than one that is unsupported.
#[test]
fn dont_touch_instances_are_never_delayed() {
    let r = run("buffer: DLY\nhold_margin: 0.0\nrounds: high\ndont_touch: f2\n")
        .expect("run with dont_touch");
    assert!(
        r.inserted.iter().all(|i| i.cap_inst != "f2"),
        "f2 is dont_touch but was delayed: {:?}",
        r.inserted.iter().map(|i| &i.cap_inst).collect::<Vec<_>>()
    );
}

#[test]
fn a_dont_touch_glob_matches_a_prefix() {
    let r = run("buffer: DLY\nhold_margin: 0.0\nrounds: high\ndont_touch: f*\n")
        .expect("run with glob dont_touch");
    assert!(
        r.inserted.is_empty(),
        "every capture instance matches f*, so nothing may be delayed: {:?}",
        r.inserted.iter().map(|i| &i.cap_inst).collect::<Vec<_>>()
    );
}

/// `rounds:` is an effort word, not a count. A number is a plausible thing to write, so the
/// rejection has to say what the accepted values are rather than silently defaulting — a
/// silent default would give a different ECO than the one that was asked for.
#[test]
fn rounds_takes_an_effort_word_and_says_so_when_it_does_not() {
    let e = run("buffer: DLY\nhold_margin: 0.0\nrounds: 0\n")
        .expect_err("a numeric rounds value is not accepted");
    for want in ["low", "medium", "high"] {
        assert!(
            e.contains(want),
            "the error should list the accepted efforts, got: {e}"
        );
    }
    // and each accepted word really is accepted
    for effort in ["low", "medium", "high"] {
        run(&format!(
            "buffer: DLY\nhold_margin: 0.0\nrounds: {effort}\n"
        ))
        .unwrap_or_else(|e| panic!("rounds: {effort} should be accepted, got {e}"));
    }
}

/// More effort may fix more, but must never fix less — a monotonicity the greedy loop is
/// supposed to give and which would break silently if the budget were mis-wired.
#[test]
fn more_effort_never_does_worse_on_hold() {
    let low = run("buffer: DLY\nhold_margin: 0.0\nrounds: low\n").expect("low");
    let high = run("buffer: DLY\nhold_margin: 0.0\nrounds: high\n").expect("high");
    assert!(
        high.after_whs >= low.after_whs - 1e-9,
        "high effort ({}) must not end worse than low effort ({})",
        high.after_whs,
        low.after_whs
    );
}

/// A margin far below the starting slack means hold is already "met", so a correct engine
/// does nothing. An engine that inserts anyway is doing damage for no reason.
#[test]
fn a_design_already_meeting_hold_is_left_alone() {
    let r = run("buffer: DLY\nhold_margin: 10.0\nrounds: high\n").expect("run with wide margin");
    assert!(
        r.inserted.is_empty(),
        "hold is met within the 10 ns margin, so nothing should be inserted: {:?}",
        r.inserted.len()
    );
    assert_eq!(r.before_whs, r.after_whs);
}

#[test]
fn an_unknown_buffer_cell_is_an_error_not_a_panic() {
    let e = run("buffer: NOT_IN_LIB\nhold_margin: 0.0\nrounds: high\n")
        .expect_err("a buffer cell absent from the .lib must fail");
    assert!(
        e.contains("NOT_IN_LIB"),
        "the error should name the missing cell, got: {e}"
    );
}

/// Naming a flop as the "delay cell" must be refused.
///
/// This found a real defect: `buffer_pins` took the *first* input and *first* output pin, so a
/// DFF resolved to (CK, Q) and the ECO chained 19 flip-flops as delay buffers, emitting them
/// with no data pin connected at all. The netlist was structurally broken and still parsed as
/// Verilog, so nothing downstream would have caught it.
#[test]
fn a_cell_that_is_not_a_single_input_buffer_is_rejected() {
    let e = run("buffer: DFF\nhold_margin: 0.0\nrounds: high\n")
        .expect_err("a flop is not a delay buffer and must be refused, not silently chained");
    assert!(e.contains("DFF"), "the error should name the cell: {e}");
    assert!(
        e.contains("input") && e.contains("output"),
        "the error should explain the one-in one-out requirement: {e}"
    );
}

/// The result is the report someone signs off on, so its two halves have to agree: claiming
/// insertions while emitting an unchanged netlist would be a silent lie.
#[test]
fn the_report_and_the_emitted_netlist_agree() {
    let r = fix();
    let buffers_in_netlist = r
        .netlist_v
        .lines()
        .filter(|l| l.trim_start().starts_with("DLY "))
        .count();
    assert_eq!(
        buffers_in_netlist,
        r.inserted.len(),
        "reported {} insertion(s) but the netlist carries {}:\n{}",
        r.inserted.len(),
        buffers_in_netlist,
        r.netlist_v
    );
}

/// Ideal-interconnect runs are not ECO runs. `eco` is what tells a consumer whether the
/// numbers came from extracted parasitics, so it must not read true without a SPEF.
#[test]
fn a_run_without_spef_is_not_flagged_as_eco() {
    assert!(
        !fix().eco,
        "run_inputs has no SPEF, so the result must not claim to be a post-route ECO"
    );
}
