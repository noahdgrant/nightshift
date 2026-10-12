//! The `## Passes` table of a `review.md`: which reviewers each pass ran and what each cost
//! (format in skills/ns-review/references/review-md.md).

use std::collections::BTreeMap;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::{axes_in, re};

/// The reviewers a pass ran: `full` or `reduced` for pass 0 (skills/ns-review step 2),
/// `cycle` for a later full cycle's pass, `fix-only` for a fix-only cycle's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Panel {
    Full,
    Reduced,
    Cycle,
    FixOnly,
}

impl Panel {
    fn from_cell(v: &str) -> Option<Panel> {
        match v.to_lowercase().replace([' ', '_'], "-").as_str() {
            "full" => Some(Panel::Full),
            "reduced" => Some(Panel::Reduced),
            "cycle" => Some(Panel::Cycle),
            "fix-only" => Some(Panel::FixOnly),
            _ => None,
        }
    }
}

/// One reviewer worker's run in a pass: a row of the `## Passes` table. A cost the harness
/// didn't report is `None`, never zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Worker {
    /// An axis from `AXES`.
    pub reviewer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

/// One review pass: its panel and the workers it ran, the denominator of every reviewer metric.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pass {
    pub pass: u32,
    pub panel: Option<Panel>,
    pub workers: Vec<Worker>,
}

/// A table cell's number (`48,210`, `$0.41`, `95.5 s`), or `None` for `-`, `n/a` or blank.
fn number(v: &str) -> Option<f64> {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"\d[\d,]*(?:\.\d+)?")
        .find(v)?
        .as_str()
        .replace(',', "")
        .parse()
        .ok()
}

/// A role or provider name as one token (`review.tests`, `claude`), else `None`: free text
/// stays out of the records.
fn name(v: &str) -> Option<String> {
    let v = v.replace('`', "");
    let v = v.trim();
    let ok = v.bytes().any(|b| b.is_ascii_alphanumeric())
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    ok.then(|| v.to_string())
}

/// The cells of a `| a | b |` table row, trimmed.
fn cells(line: &str) -> Vec<&str> {
    let l = line.trim();
    let l = l.strip_prefix('|').unwrap_or(l);
    let l = l.strip_suffix('|').unwrap_or(l);
    l.split('|').map(str::trim).collect()
}

/// The `## Passes` table: one pass per pass number, lowest first, each with the first panel
/// its rows name. Columns are found by header name; a row with no pass number or no axis as
/// its reviewer is skipped. A review from before the table has no passes.
pub fn passes(text: &str) -> Vec<Pass> {
    let mut by_pass: BTreeMap<u32, Pass> = Default::default();
    let mut in_section = false;
    let mut header: Option<Vec<String>> = None;
    let mut fence = false;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
            continue;
        }
        if fence {
            continue;
        }
        if line.starts_with('#') {
            in_section =
                line.starts_with("## ") && line[3..].trim().to_lowercase().starts_with("passes");
            header = None;
            continue;
        }
        if !in_section || !t.starts_with('|') {
            continue;
        }
        let row = cells(t);
        let Some(head) = &header else {
            header = Some(row.iter().map(|c| c.to_lowercase()).collect());
            continue;
        };
        if row.iter().all(|c| c.chars().all(|ch| "-: ".contains(ch))) {
            continue;
        }
        let col = |want: &dyn Fn(&str) -> bool| {
            head.iter()
                .position(|h| want(h))
                .and_then(|i| row.get(i).copied())
        };
        let Some(pass) = col(&|h| h == "pass").and_then(|c| c.parse::<u32>().ok()) else {
            continue;
        };
        let Some(reviewer) =
            col(&|h| h.starts_with("reviewer")).and_then(|c| axes_in(c).into_iter().next())
        else {
            continue;
        };
        let p = by_pass.entry(pass).or_insert(Pass {
            pass,
            panel: None,
            workers: Vec::new(),
        });
        p.panel = p
            .panel
            .or_else(|| col(&|h| h == "panel").and_then(Panel::from_cell));
        p.workers.push(Worker {
            reviewer,
            role: col(&|h| h == "role").and_then(name),
            provider: col(&|h| h == "provider").and_then(name),
            tokens: col(&|h| h.starts_with("tokens"))
                .and_then(number)
                .map(|n| n as u64),
            wall_s: col(&|h| h.contains("second") || h.starts_with("wall")).and_then(number),
            cost_usd: col(&|h| h.starts_with("cost")).and_then(number),
        });
    }
    by_pass.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review_md::AXES;

    const REDUCED: &str = "\
## Passes
| Pass | Panel | Reviewer | Role | Provider | Tokens | Seconds | Cost (USD) |
|---|---|---|---|---|---|---|---|
| 0 | reduced | correctness | `review.correctness` | claude | 48,210 | 192 | $0.41 |
| 0 | reduced | spec | review.spec | codex | 30110 | 95.5 | - |
| 0 | reduced | tests | review | claude | n/a | | |
| 1 | cycle | correctness | review.correctness | claude | 22000 | 80 | 0.2 |

## Critical
";

    #[test]
    fn passes_record_a_reduced_panel_and_each_workers_cost() {
        let ps = passes(REDUCED);
        assert_eq!(ps.len(), 2);
        assert_eq!((ps[0].pass, ps[0].panel), (0, Some(Panel::Reduced)));
        let who: Vec<_> = ps[0].workers.iter().map(|w| w.reviewer.as_str()).collect();
        assert_eq!(who, ["correctness", "spec", "tests"]);
        assert_eq!(
            ps[0].workers[0],
            Worker {
                reviewer: "correctness".into(),
                role: Some("review.correctness".into()),
                provider: Some("claude".into()),
                tokens: Some(48_210),
                wall_s: Some(192.0),
                cost_usd: Some(0.41),
            }
        );
        assert_eq!(
            (ps[0].workers[1].wall_s, ps[0].workers[1].cost_usd),
            (Some(95.5), None)
        );
        let t = &ps[0].workers[2];
        assert_eq!((t.tokens, t.wall_s, t.cost_usd), (None, None, None));
        assert_eq!((ps[1].pass, ps[1].panel), (1, Some(Panel::Cycle)));
        assert_eq!(ps[1].workers.len(), 1);
    }

    #[test]
    fn passes_record_a_full_panel_whatever_the_column_order() {
        let rows: String = AXES
            .iter()
            .map(|a| format!("| {a} | 1000 | 0 | Full | 10 | 0.01 |\n"))
            .collect();
        let text = format!(
            "## Passes\n| Reviewer | Tokens | Pass | Panel | Wall time (s) | Cost |\n|--|--|--|--|--|--|\n{rows}| | 5 | 0 | full | 1 | 1 |\n| tests | 5 | none | full | 1 | 1 |\n| fix-only | 1 | 4 | fix-only | 1 | 1 |\n"
        );
        let ps = passes(&text);
        assert_eq!(ps.len(), 1, "{ps:?}");
        assert_eq!(ps[0].panel, Some(Panel::Full));
        let who: Vec<_> = ps[0].workers.iter().map(|w| w.reviewer.as_str()).collect();
        assert_eq!(who, AXES);
        assert_eq!(
            (
                ps[0].workers[0].role.as_deref(),
                ps[0].workers[0].provider.as_deref()
            ),
            (None, None)
        );
        assert_eq!(ps[0].workers[7].cost_usd, Some(0.01));
    }

    #[test]
    fn a_pass_takes_the_first_panel_its_rows_name_and_free_text_stays_out() {
        let ps = passes(
            "## Passes\n| Pass | Panel | Reviewer | Role | Provider |\n|---|---|---|---|---|\n| 2 | ? | correctness | see /srv/x | claude opus |\n| 2 | fix-only | tests | review.tests | codex |\n",
        );
        assert_eq!(ps[0].panel, Some(Panel::FixOnly));
        assert_eq!(
            (
                ps[0].workers[0].role.as_deref(),
                ps[0].workers[0].provider.as_deref()
            ),
            (None, None)
        );
        assert_eq!(ps[0].workers[1].provider.as_deref(), Some("codex"));
    }

    #[test]
    fn a_review_without_a_passes_table_has_no_passes() {
        assert!(
            passes("## Reviewers\n| Reviewer | Role |\n|---|---|\n| tests | review |\n").is_empty()
        );
        assert!(
            passes("```\n## Passes\n| Pass | Reviewer |\n|-|-|\n| 0 | tests |\n```\n").is_empty()
        );
        assert!(passes("## Passes\nNone recorded.\n").is_empty());
        let after =
            passes("## Passes\n| Pass | Reviewer |\n|-|-|\n| 0 | tests |\n## Gaps\n| 1 | spec |\n");
        assert_eq!(after.len(), 1);
    }
}
