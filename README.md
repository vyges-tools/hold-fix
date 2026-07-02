# vyges-hold-fix

**Post-route hold-fix ECO**: a gate-level netlist in, a **hold-fixed netlist** out — delay
cells inserted on the data paths that reach their capture flops too early, scored by a real
static-timing engine so setup is never sacrificed to fix hold.

> **Vyges open EDA tools.** The hold counterpart to the "close-timing" engines
> [`vyges-resize`](https://github.com/vyges-tools/resize) (drive strength),
> [`vyges-vt-swap`](https://github.com/vyges-tools/vt-swap) (threshold voltage), and
> [`vyges-buffer-insert`](https://github.com/vyges-tools/buffer-insert) (transition/fanout).
> Those all fix the **late / setup** corner. Hold violations are the opposite problem —
> data arrives at a capture flop **too early** on the min-delay path — and open place-and-route
> flows often leave a residue of them after detailed routing. `vyges-hold-fix` **adds delay**:
> it inserts a delay cell in series on the net feeding each hold-violating capture pin, lifting
> that pin's earliest arrival until its hold constraint is met.

## What it does

Given a netlist + Liberty (+ optional SPEF) and a clock, it runs the shared
[`vyges-sta-si`](https://github.com/vyges-tools/sta-si) timer to find every hold-violating
capture endpoint, then, round by round:

1. rank the endpoints whose **hold slack** is below `-hold_margin`;
2. insert one delay cell **in series** on the net feeding each — the pin is re-driven by a
   fresh buffer whose input is the pin's original net, adding one cell's min-path delay;
3. rebuild the timer and **keep the ECO iff worst hold improved and setup stays met**;
4. repeat — a still-violating endpoint simply gets another delay next round, so a delay
   **chain grows only where hold is still negative**.

A slow clock usually leaves ample setup slack, so trading a little of it for hold closure is
safe; the accept test refuses any move that would push setup negative.

## Install

```bash
# via the Vyges installer (whole Loom suite)
vyges install loom
# or build from source
cargo build --release   # -> target/release/vyges-hold-fix
```

## CLI

```
vyges-hold-fix run   JOB  [-o OUT] [--json] [--fail-on-violation]   hold-fix -> delayed netlist
vyges-hold-fix check JOB                                            validate the job
vyges-hold-fix demo                                                 hold-fix a built-in example (no files)

flags:
  -o FILE              write the hold-fixed netlist to FILE (default: stdout)
  --json               emit the before/after report as JSON
  --fail-on-violation  exit 3 if the result still has negative hold slack (CI gate)
  -q, --quiet          suppress non-essential output
  -v, --verbose        extra detail on stderr
  -h, --help           show this help
  -V, --version        show version
```

Exit codes: `0` ok · `1` runtime error · `2` usage/validation · `3` still-violating
(with `--fail-on-violation`).

### The job file (`.holdfix`)

A `.holdfix` file is a **superset of a `vyges-sta-si` `.sta` job** — the same timing setup
keys (read by the shared timer) plus the hold-fix knobs:

```text
# --- timing setup (same as a .sta job) ---
design:      soc_top
netlist:     soc_top.pnl.v
lib:         sky130_fd_sc_hd__tt_025C_1v80.lib, macro_a.lib, macro_b.lib   # comma-separated
spef:        soc_top.nom.spef                     # optional parasitics
sdc:         soc_top.sdc                          # supplies the clock(s)
# (or an explicit `clock: <port> <period>` instead of an sdc)

# --- hold-fix knobs ---
buffer:      sky130_fd_sc_hd__clkdlybuf4s15_1   # the delay cell to insert (1 input, 1 output)
hold_margin: 0.02                               # fix endpoints with hold slack below -margin (ns)
rounds:      high                               # low(10) | medium(40) | high(200)  max ECO rounds
dont_touch:  clk_* *scan*                       # capture-instance globs left alone
```

### Example

```bash
cd examples
vyges-hold-fix run two_flop.holdfix -o two_flop.fixed.v
#   vyges-hold-fix — hold-fix ECO
#     hold:    WHS -0.2000 -> 0.1013 ns [MET]  (margin 0.0000)
#     setup:   WNS 4.9787 -> 4.9787 ns [MET]
#     delays:  4 inserted
```

The report prints **before → after** worst hold slack (WHS) and worst setup slack (WNS), plus
the delay cells inserted and the capture pin each one delays. `--json` emits the same as a
machine-readable record for CI.

## Notes

- **Post-route ECO**: with `spef:` present the timer uses extracted interconnect, so the fix
  is scored against real routed delays. Without SPEF it runs on ideal interconnect (pre-route).
- Because it changes netlist topology, each round is scored by rebuilding the timer on the
  mutated netlist (incremental topology update is future work).
- Emits structural Verilog that round-trips through the `vyges-sta-si` / `vyges-loom` reader,
  including Verilog escaped identifiers (`\clkbuf_0_gpio_in[0]`).

---

© 2026 Vyges. Apache-2.0. https://vyges.com
