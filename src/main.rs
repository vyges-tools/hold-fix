//! vyges-hold-fix CLI.
//!
//!   vyges-hold-fix run   JOB  [-o OUT] [--json] [--fail-on-violation]
//!   vyges-hold-fix check JOB
//!   vyges-hold-fix demo
//!
//! Common flags: -h/--help, -V/--version, -q/--quiet, -v/--verbose.
//! Exit codes: 0 ok · 1 runtime error · 2 usage/validation · 3 still-violating (--fail-on-violation).

use std::process::exit;

use vyges_hold_fix::engine::{self, HoldResult, Insertion};
use vyges_hold_fix::job::{parse_cfg, HoldJob};
use vyges_sta_si::job::StaJob;

const USAGE: &str = "\
vyges-hold-fix — post-route hold-fix ECO (insert series delay on hold-violating capture pins)

usage:
  vyges-hold-fix run   JOB  [-o OUT] [--json] [--fail-on-violation]   hold-fix -> delayed netlist
  vyges-hold-fix check JOB                                            validate the job
  vyges-hold-fix demo                                                 hold-fix a built-in example (no files)

flags:
  -o FILE              write the hold-fixed netlist to FILE (default: stdout)
  --json               emit the before/after report as JSON
  --eco FILE           write the ECO manifest (insertions) as JSON — for a physical applier
  --fail-on-violation  exit 3 if the result still has negative hold slack (CI gate)
  -q, --quiet          suppress non-essential output
  -v, --verbose        extra detail on stderr
  -h, --help           show this help
  -V, --version        show version
";

#[derive(Default)]
struct Cli {
    positionals: Vec<String>,
    out: Option<String>,
    eco: Option<String>,
    json: bool,
    fail_on_violation: bool,
    quiet: bool,
    verbose: bool,
    help: bool,
    version: bool,
}

fn parse_cli(args: &[String]) -> Cli {
    let mut c = Cli::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                c.out = args.get(i + 1).cloned();
                i += 1;
            }
            "--eco" => {
                c.eco = args.get(i + 1).cloned();
                i += 1;
            }
            "--json" => c.json = true,
            "--fail-on-violation" => c.fail_on_violation = true,
            "-q" | "--quiet" => c.quiet = true,
            "-v" | "--verbose" => c.verbose = true,
            "-h" | "--help" => c.help = true,
            "-V" | "--version" => c.version = true,
            other => c.positionals.push(other.to_string()),
        }
        i += 1;
    }
    c
}

fn render_report(r: &HoldResult) -> String {
    let met = |w: f64, m: f64| if w >= -m { "MET" } else { "VIOLATED" };
    let mut s = String::new();
    s.push_str("vyges-hold-fix — hold-fix ECO\n");
    s.push_str(&format!(
        "  mode:    {}\n",
        if r.eco { "post-route ECO (SPEF interconnect)" } else { "pre-route (ideal interconnect)" }
    ));
    s.push_str(&format!(
        "  hold:    WHS {:.4} -> {:.4} ns [{}]  (margin {:.4})\n",
        r.before_whs, r.after_whs, met(r.after_whs, r.hold_margin), r.hold_margin
    ));
    s.push_str(&format!(
        "  setup:   WNS {:.4} -> {:.4} ns [{}]\n",
        r.before_wns,
        r.after_wns,
        if r.after_wns >= 0.0 { "MET" } else { "VIOLATED" }
    ));
    s.push_str(&format!("  delays:  {} inserted\n", r.inserted.len()));
    for ins in r.inserted.iter().take(20) {
        s.push_str(&format!("    {} delays {}/{}\n", ins.buffer, ins.cap_inst, ins.cap_pin));
    }
    if r.inserted.len() > 20 {
        s.push_str(&format!("    … and {} more\n", r.inserted.len() - 20));
    }
    s
}

fn report_json(r: &HoldResult) -> String {
    format!(
        "{{\"eco\":{},\"before_whs\":{},\"after_whs\":{},\"before_wns\":{},\"after_wns\":{},\"hold_margin\":{},\"inserted\":{}}}",
        r.eco, r.before_whs, r.after_whs, r.before_wns, r.after_wns, r.hold_margin, r.inserted.len()
    )
}

// ---- built-in demo: two flops with a near-zero data path (q1 -> f2.D) plus a fast CK->Q, so
// f2 captures its own launch data on the same edge — a hold violation the ECO relieves. ----
const DEMO_NL: &str = "module top ( clk, d, q ); input clk, d; output q; wire q1;\n\
                       DFF f1 ( .CK(clk), .D(d),  .Q(q1) );\n\
                       DFF f2 ( .CK(clk), .D(q1), .Q(q)  );\n\
                       endmodule";
const DEMO_LIB: &str = r#"
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
const DEMO_JOB: &str = "design: top\nnetlist: x\nlib: x\nclock: clk 5.0\ninput_slew: 0.02\noutput_load: 0.003\n";

fn run_demo() -> Result<HoldResult, String> {
    let sta = StaJob::parse(DEMO_JOB, "").map_err(|e| e.to_string())?;
    let cfg = parse_cfg("buffer: DLY\nhold_margin: 0.0\nrounds: high\n")?;
    engine::run_inputs(DEMO_NL, DEMO_LIB, &HoldJob { sta, cfg })
}

fn write_netlist(text: &str, out: &Option<String>, quiet: bool) {
    match out {
        Some(path) => match std::fs::write(path, text) {
            Ok(_) => {
                if !quiet {
                    eprintln!("wrote {path}");
                }
            }
            Err(e) => {
                eprintln!("error: {path}: {e}");
                exit(1);
            }
        },
        None => print!("{text}"),
    }
}

fn json_str(s: &str) -> String {
    // minimal JSON string escaping (identifiers incl. Verilog escaped names / bus brackets)
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn eco_manifest_json(r: &HoldResult) -> String {
    let items: Vec<String> = r
        .inserted
        .iter()
        .map(|i: &Insertion| {
            format!(
                "{{\"buffer\":{},\"cell\":{},\"in_pin\":{},\"out_pin\":{},\"in_net\":{},\"out_net\":{},\"cap_inst\":{},\"cap_pin\":{}}}",
                json_str(&i.buffer), json_str(&i.cell), json_str(&i.in_pin), json_str(&i.out_pin),
                json_str(&i.in_net), json_str(&i.out_net), json_str(&i.cap_inst), json_str(&i.cap_pin)
            )
        })
        .collect();
    format!(
        "{{\"hold_before_ns\":{},\"hold_after_ns\":{},\"count\":{},\"insertions\":[{}]}}",
        r.before_whs, r.after_whs, r.inserted.len(), items.join(",")
    )
}

fn finish(r: HoldResult, cli: &Cli) {
    if cli.json {
        println!("{}", report_json(&r));
        if cli.out.is_some() {
            write_netlist(&r.netlist_v, &cli.out, cli.quiet);
        }
    } else {
        write_netlist(&r.netlist_v, &cli.out, cli.quiet);
        if !cli.quiet {
            eprint!("{}", render_report(&r));
        }
    }
    if let Some(path) = &cli.eco {
        match std::fs::write(path, eco_manifest_json(&r)) {
            Ok(_) => { if !cli.quiet { eprintln!("eco manifest ({} insertions) -> {path}", r.inserted.len()); } }
            Err(e) => { eprintln!("error: {path}: {e}"); exit(1); }
        }
    }
    if cli.fail_on_violation && r.after_whs < -r.hold_margin {
        exit(3);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cli = parse_cli(&args);

    if cli.version {
        println!("vyges-hold-fix {} ({})", vyges_hold_fix::VERSION, env!("VYGES_GIT_SHA"));
        println!("{}", vyges_hold_fix::COPYRIGHT);
        return;
    }
    let cmd = cli.positionals.first().cloned().unwrap_or_default();
    if cli.help || cmd.is_empty() {
        print!("{USAGE}");
        exit(if cmd.is_empty() && !cli.help { 2 } else { 0 });
    }

    match cmd.as_str() {
        "demo" => match run_demo() {
            Ok(r) => finish(r, &cli),
            Err(e) => {
                eprintln!("error: {e}");
                exit(1);
            }
        },
        "check" => {
            let Some(path) = cli.positionals.get(1) else {
                eprintln!("usage: vyges-hold-fix check JOB");
                exit(2);
            };
            match HoldJob::load(path) {
                Ok(j) => println!(
                    "OK  design={} buffer={} hold_margin={} rounds={} dont_touch={}",
                    j.sta.design, j.cfg.buffer, j.cfg.hold_margin, j.cfg.rounds, j.cfg.dont_touch.len()
                ),
                Err(e) => {
                    eprintln!("error: {e}");
                    exit(2);
                }
            }
        }
        "run" => {
            let Some(path) = cli.positionals.get(1) else {
                eprintln!("usage: vyges-hold-fix run JOB [-o OUT]");
                exit(2);
            };
            let job = match HoldJob::load(path) {
                Ok(j) => j,
                Err(e) => {
                    eprintln!("error: {e}");
                    exit(2);
                }
            };
            if cli.verbose {
                eprintln!("hold-fixing {} (delay {}, rounds {})", job.sta.design, job.cfg.buffer, job.cfg.rounds);
            }
            match engine::run(&job) {
                Ok(r) => finish(r, &cli),
                Err(e) => {
                    eprintln!("error: {e}");
                    exit(1);
                }
            }
        }
        other => {
            eprintln!("vyges-hold-fix: unknown command {other:?}\n");
            print!("{USAGE}");
            exit(2);
        }
    }
}
