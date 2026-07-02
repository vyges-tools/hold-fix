//! The `.holdfix` job: the timing setup (reused from `vyges-sta-si`'s job parser) plus the
//! hold-fix knobs. A `.holdfix` file is a superset of a `.sta` file — same
//! `design`/`netlist`/`lib`/`clock`/`spef`/`sdc`/… keys (read by [`StaJob`]) and adds:
//!
//! ```text
//! buffer:      sky130_fd_sc_hd__dlygate4sd3_1   # the delay cell to insert in series
//! hold_margin: 0.05                             # target hold slack (ns); fix endpoints below -margin
//! rounds:      medium                           # low | medium | high  (max ECO rounds)
//! dont_touch:  clk_* *scan*                     # capture-instance globs left alone
//! ```

use vyges_sta_si::job::StaJob;

/// The hold-fix configuration (everything beyond the timing setup).
#[derive(Debug, Clone)]
pub struct HoldCfg {
    /// The delay cell to insert in series (must have exactly one input and one output pin).
    pub buffer: String,
    /// Target hold slack in ns; endpoints with slack below `-hold_margin` are fixed.
    pub hold_margin: f64,
    /// Max ECO rounds (derived from `rounds:`); each round adds one delay per still-violating
    /// endpoint, so a chain grows only where hold remains negative.
    pub rounds: usize,
    /// Capture-instance globs (leading/trailing `*`) whose endpoints are never delayed.
    pub dont_touch: Vec<String>,
}

/// A loaded hold-fix job: the timing job + the config.
#[derive(Debug, Clone)]
pub struct HoldJob {
    pub sta: StaJob,
    pub cfg: HoldCfg,
}

impl HoldJob {
    pub fn load(path: &str) -> Result<HoldJob, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let sta = StaJob::load(path).map_err(|e| e.to_string())?;
        let cfg = parse_cfg(&text)?;
        Ok(HoldJob { sta, cfg })
    }
}

/// Parse the hold-fix keys out of the job text.
pub fn parse_cfg(text: &str) -> Result<HoldCfg, String> {
    let mut buffer = String::new();
    let mut hold_margin = 0.0f64;
    let mut rounds_word = "medium".to_string();
    let mut dont_touch = Vec::new();
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        let Some((k, v)) = line.split_once(':') else { continue };
        let (k, v) = (k.trim().to_lowercase(), v.trim());
        match k.as_str() {
            "buffer" => buffer = v.to_string(),
            "hold_margin" => {
                hold_margin =
                    v.parse().map_err(|_| format!("hold_margin must be a number, got {v:?}"))?
            }
            "rounds" | "effort" => rounds_word = v.to_lowercase(),
            "dont_touch" => {
                dont_touch.extend(
                    v.split([',', ' ']).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string),
                );
            }
            _ => {}
        }
    }
    if buffer.is_empty() {
        return Err("a `buffer:` (delay cell) is required".into());
    }
    let rounds = match rounds_word.as_str() {
        "low" => 10,
        "medium" => 40,
        "high" => 200,
        other => return Err(format!("rounds must be low|medium|high, got {other:?}")),
    };
    Ok(HoldCfg { buffer, hold_margin, rounds, dont_touch })
}

/// A tiny glob matcher: supports a single leading and/or trailing `*` (e.g. `clk_*`,
/// `*scan*`, `*_reg`). Exact match otherwise.
pub fn glob_match(pat: &str, s: &str) -> bool {
    match (pat.strip_prefix('*'), pat.strip_suffix('*')) {
        (Some(_), Some(_)) => s.contains(pat.trim_matches('*')),
        (Some(suf), None) => s.ends_with(suf),
        (None, Some(pre)) => s.starts_with(pre),
        (None, None) => s == pat,
    }
}
