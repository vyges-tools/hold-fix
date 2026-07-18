//! Post-route hold-fix ECO.
//!
//! Each round: rank the hold-violating capture endpoints (from sta-si's per-endpoint
//! hold slacks), insert a delay buffer **in series** on the net feeding each violating
//! pin — the pin is re-driven by a fresh buffer whose input is the pin's original net,
//! so one buffer's min-path delay is added to the data arriving at the capture, lifting
//! its hold slack. Rebuild the timer, and keep the ECO iff the worst hold slack improved
//! without pushing setup negative. Repeat until hold is met (within `hold_margin`) or the
//! round budget is exhausted; a still-violating endpoint simply gets another buffer next
//! round, so a delay chain grows only where it is needed.
//!
//! A slow clock usually leaves ample setup slack, so trading a little of it for hold
//! closure is safe — the accept test refuses any move that would make setup negative.

use vyges_sta_si::job::StaJob;
use vyges_sta_si::liberty::Lib;
use vyges_sta_si::netlist::{self, Inst, Netlist};
use vyges_sta_si::spef::Spef;
use vyges_sta_si::sta::Timer;

use crate::emit;
use crate::job::{glob_match, HoldJob};

/// One applied delay insertion — everything a physical ECO applier needs to re-create it on
/// a routed database: make the buffer instance, place it, and rewire the capture pin. The
/// capture pin's original net (`in_net`) now feeds the buffer input; the buffer output
/// (`out_net`) drives the capture pin.
#[derive(Debug, Clone)]
pub struct Insertion {
    pub buffer: String,   // new delay-buffer instance name
    pub cell: String,     // delay cell master
    pub in_pin: String,   // buffer input pin name
    pub out_pin: String,  // buffer output pin name
    pub in_net: String,   // net feeding the buffer input (the pin's original driver net)
    pub out_net: String,  // new net the buffer drives, into the capture pin
    pub cap_inst: String, // capture instance whose pin is re-driven
    pub cap_pin: String,  // capture pin (e.g. D)
}

/// Outcome of a hold-fix run.
#[derive(Debug, Clone)]
pub struct HoldResult {
    pub before_whs: f64,
    pub after_whs: f64,
    pub before_wns: f64,
    pub after_wns: f64,
    pub hold_margin: f64,
    /// Every applied delay insertion, in order — the machine-readable ECO manifest.
    pub inserted: Vec<Insertion>,
    /// The hold-fixed netlist as structural Verilog.
    pub netlist_v: String,
    pub eco: bool,
}

/// The (single input, single output) pin names of the delay-buffer cell.
fn buffer_pins(lib: &Lib, buf: &str) -> Result<(String, String), String> {
    let cell = lib
        .cell(buf)
        .ok_or_else(|| format!("buffer cell {buf:?} not in any .lib"))?;
    let inp = cell
        .pins
        .iter()
        .find(|(_, p)| format!("{:?}", p.direction).contains("In"))
        .map(|(n, _)| n.clone())
        .ok_or_else(|| format!("buffer {buf:?} has no input pin"))?;
    let out = cell
        .pins
        .iter()
        .find(|(_, p)| format!("{:?}", p.direction).contains("Out"))
        .map(|(n, _)| n.clone())
        .ok_or_else(|| format!("buffer {buf:?} has no output pin"))?;
    Ok((inp, out))
}

/// Insert a delay buffer in series on the net feeding `inst_name/pin`. The pin is re-driven
/// by a fresh buffer whose input is the pin's original net; returns the new buffer's name,
/// or `None` if the instance/pin can't be located.
fn insert_series_delay(
    nl: &mut Netlist,
    inst_name: &str,
    pin: &str,
    buf_cell: &str,
    bin: &str,
    bout: &str,
    k: usize,
) -> Option<Insertion> {
    let ii = nl.insts.iter().position(|i| i.name == inst_name)?;
    let ci = nl.insts[ii].conns.iter().position(|(p, _)| p == pin)?;
    let old_net = nl.insts[ii].conns[ci].1.clone();
    let new_net = format!("__hold_n{k}");
    nl.insts[ii].conns[ci].1 = new_net.clone();
    let bufname = format!("__hold_buf{k}");
    nl.insts.push(Inst {
        cell: buf_cell.to_string(),
        name: bufname.clone(),
        conns: vec![
            (bin.to_string(), old_net.clone()),
            (bout.to_string(), new_net.clone()),
        ],
    });
    Some(Insertion {
        buffer: bufname,
        cell: buf_cell.to_string(),
        in_pin: bin.to_string(),
        out_pin: bout.to_string(),
        in_net: old_net,
        out_net: new_net,
        cap_inst: inst_name.to_string(),
        cap_pin: pin.to_string(),
    })
}

/// Run a hold-fix job loaded from disk.
pub fn run(job: &HoldJob) -> Result<HoldResult, String> {
    let sta = &job.sta;
    let nl = netlist::load(&sta.resolve(&sta.netlist)).map_err(|e| e.to_string())?;
    let mut lib = Lib::default();
    for l in &sta.libs {
        let one = Lib::load(&sta.resolve(l)).map_err(|e| e.to_string())?;
        lib.cells.extend(one.cells);
    }
    if lib.cells.is_empty() {
        return Err("no cells in any .lib".into());
    }
    let spef = match &sta.spef {
        Some(p) => Some(Spef::load(&sta.resolve(p)).map_err(|e| e.to_string())?),
        None => None,
    };
    optimize(nl, &lib, sta, spef, job)
}

/// Run on already-parsed inputs (the `demo` path; ideal interconnect, no SPEF).
pub fn run_inputs(nl_text: &str, lib_text: &str, job: &HoldJob) -> Result<HoldResult, String> {
    let nl = netlist::parse(nl_text).map_err(|e| e.to_string())?;
    let lib = Lib::parse(lib_text).map_err(|e| e.to_string())?;
    optimize(nl, &lib, &job.sta, None, job)
}

/// The optimizer: greedily add series delay at every violating hold endpoint, round by
/// round, until hold is met or the round budget runs out.
pub fn optimize(
    mut nl: Netlist,
    lib: &Lib,
    sta: &StaJob,
    spef: Option<Spef>,
    job: &HoldJob,
) -> Result<HoldResult, String> {
    let cfg = &job.cfg;
    let (bin, bout) = buffer_pins(lib, &cfg.buffer)?;
    let build = |nl: &Netlist| Timer::build(nl, lib, sta, spef.as_ref()).map_err(|e| e.to_string());

    let mut timer = build(&nl)?;
    let before_whs = timer.whs();
    let before_wns = timer.wns();
    let margin = cfg.hold_margin;

    let mut inserted: Vec<Insertion> = Vec::new();
    let mut counter = 0usize;

    for _round in 0..cfg.rounds {
        if timer.whs() >= -margin {
            break; // hold met (within margin)
        }
        // rank violating hold endpoints, worst first
        let viol: Vec<usize> = timer
            .hold_endpoint_slacks()
            .into_iter()
            .filter(|(_, s)| *s < -margin)
            .map(|(p, _)| p)
            .collect();
        if viol.is_empty() {
            break;
        }

        // one delay buffer per still-violating endpoint (a chain grows over rounds)
        let mut trial = nl.clone();
        let mut round_bufs: Vec<Insertion> = Vec::new();
        for p in &viol {
            let label = timer.pin_label(*p).to_string();
            // "inst/pin" — a primary-output port (no '/') is skipped (rare for hold)
            let Some((inst_name, pin)) = label.rsplit_once('/') else {
                continue;
            };
            if cfg.dont_touch.iter().any(|g| glob_match(g, inst_name)) {
                continue;
            }
            if let Some(ins) = insert_series_delay(
                &mut trial,
                inst_name,
                pin,
                &cfg.buffer,
                &bin,
                &bout,
                counter,
            ) {
                round_bufs.push(ins);
                counter += 1;
            }
        }
        if round_bufs.is_empty() {
            break;
        }

        let ttimer = build(&trial)?;
        // accept iff worst hold improved and setup stays acceptable (never push it negative;
        // on an already-violating setup, at least don't worsen it).
        let setup_ok = if timer.wns() >= 0.0 {
            ttimer.wns() >= -1e-9
        } else {
            ttimer.wns() >= timer.wns() - 1e-9
        };
        if ttimer.whs() > timer.whs() + 1e-9 && setup_ok {
            inserted.extend(round_bufs);
            nl = trial;
            timer = ttimer;
        } else {
            break; // no further progress (delay not helping, or setup would break)
        }
    }

    Ok(HoldResult {
        before_whs,
        after_whs: timer.whs(),
        before_wns,
        after_wns: timer.wns(),
        hold_margin: margin,
        inserted,
        netlist_v: emit::to_verilog(&nl),
        eco: spef.is_some(),
    })
}
