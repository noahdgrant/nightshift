//! The last line of a check's evidence says how the check ended.

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The work meets the unit.
    Pass,
    /// The work does not meet the unit.
    Fail,
    /// No requirement failed, but at least one could not be proven from the workspace.
    Inconclusive,
    /// The check itself went wrong, so it says nothing about the work.
    Error,
}

pub const PASS_LINE: &str = "VERDICT: PASS";
pub const FAIL_LINE: &str = "VERDICT: FAIL";
pub const INCONCLUSIVE_LINE: &str = "VERDICT: INCONCLUSIVE";
pub const ERROR_LINE: &str = "VERDICT: ERROR";

/// The verdict on the last non-empty line of `evidence`, if that line is one.
pub fn parse(evidence: &str) -> Option<Verdict> {
    match evidence
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())?
        .trim()
    {
        PASS_LINE => Some(Verdict::Pass),
        FAIL_LINE => Some(Verdict::Fail),
        INCONCLUSIVE_LINE => Some(Verdict::Inconclusive),
        ERROR_LINE => Some(Verdict::Error),
        _ => None,
    }
}

/// The evidence the unit has so far, if it ends in `verdict`.
pub fn evidence_ending_in(
    workspace: &std::path::Path,
    unit: &str,
    verdict: Verdict,
) -> Option<String> {
    let text = crate::artifact::read(workspace, unit, "evidence.md").ok()?;
    (parse(&text) == Some(verdict)).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_last_line_counts() {
        assert_eq!(parse("a\nVERDICT: PASS\n\n"), Some(Verdict::Pass));
        assert_eq!(
            parse("VERDICT: PASS\nbut then\nVERDICT: FAIL"),
            Some(Verdict::Fail)
        );
        assert_eq!(
            parse("VERDICT: FAIL\nthen\nVERDICT: ERROR"),
            Some(Verdict::Error)
        );
        assert_eq!(parse("VERDICT: PASS\nmore"), None);
        assert_eq!(
            parse("a\nVERDICT: INCONCLUSIVE\n"),
            Some(Verdict::Inconclusive)
        );
        assert_eq!(parse("VERDICT: INCONCLUSIVE\nmore"), None);
        assert_eq!(
            parse("VERDICT: INCONCLUSIVE\nVERDICT: FAIL"),
            Some(Verdict::Fail)
        );
        assert_eq!(parse(""), None);
    }
}
