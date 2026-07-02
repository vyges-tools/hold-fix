//! Emit a structural Verilog netlist (the hold-fixed design) — round-trips through the
//! `vyges-sta-si` / `vyges-loom` netlist parser.

use std::collections::HashSet;
use std::fmt::Write;

use vyges_sta_si::netlist::Netlist;

/// Render an identifier for output. A Verilog **escaped identifier** (`\name[0]`,
/// `\a.b[1]`) is terminated by whitespace — so it must be followed by a space before any
/// punctuation (`,`, `;`, `)`), else the parser folds that punctuation into the name.
/// Plain identifiers pass through unchanged.
fn id(name: &str) -> String {
    if name.starts_with('\\') {
        format!("{name} ")
    } else {
        name.to_string()
    }
}

fn join_ids<'a, I: IntoIterator<Item = &'a str>>(it: I) -> String {
    it.into_iter().map(id).collect::<Vec<_>>().join(", ")
}

/// Render `nl` as structural Verilog: module header, `input`/`output`/`wire` declarations,
/// then one instance line per cell (`<cell> <inst> ( .<pin>(<net>), … );`).
pub fn to_verilog(nl: &Netlist) -> String {
    let mut s = String::new();
    let ports: Vec<&str> = nl.inputs.iter().chain(&nl.outputs).map(String::as_str).collect();
    let _ = writeln!(s, "module {} ( {} );", nl.module, join_ids(ports.iter().copied()));
    if !nl.inputs.is_empty() {
        let _ = writeln!(s, "  input {};", join_ids(nl.inputs.iter().map(String::as_str)));
    }
    if !nl.outputs.is_empty() {
        let _ = writeln!(s, "  output {};", join_ids(nl.outputs.iter().map(String::as_str)));
    }

    // wires = nets referenced by instance connections that aren't primary ports, in
    // first-seen order (deduped).
    let portset: HashSet<&str> = ports.iter().copied().collect();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut wires: Vec<&str> = Vec::new();
    for inst in &nl.insts {
        for (_, net) in &inst.conns {
            let net = net.as_str();
            if !portset.contains(net) && seen.insert(net) {
                wires.push(net);
            }
        }
    }
    if !wires.is_empty() {
        let _ = writeln!(s, "  wire {};", join_ids(wires.iter().copied()));
    }

    for inst in &nl.insts {
        let conns: Vec<String> =
            inst.conns.iter().map(|(p, n)| format!(".{p}({})", id(n))).collect();
        let _ = writeln!(s, "  {} {} ( {} );", inst.cell, id(&inst.name), conns.join(", "));
    }
    let _ = writeln!(s, "endmodule");
    s
}
