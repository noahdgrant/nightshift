//! The text of a unit once it has been defined and planned. It is the unit's description on the
//! forge, so a human can read it and correct it.

use anyhow::{bail, Context, Result};

use crate::forge::DISCLOSURE;

/// Opens the block that keeps the description a unit had before it was defined.
const ORIGINAL: &str = "<details><summary>Original description</summary>";
const SLICES: &str = "## Slices";
const DIAGNOSIS: &str = "## Diagnosis";

/// The description of a unit that was defined: the spec first, the text it came from below.
pub fn body_defined(spec: &str, original: &str) -> String {
    let mut body = format!("{DISCLOSURE}\n\n{}\n", spec.trim());
    if !original.trim().is_empty() {
        body.push_str(&format!(
            "\n{ORIGINAL}\n\n{}\n\n</details>\n",
            original.trim()
        ));
    }
    body
}

/// Whether `body` already has slices.
pub fn is_planned(body: &str) -> bool {
    body.lines().any(|l| l.trim() == SLICES)
}

/// `body` with `slices` added above the original description, or at the end when it has none.
/// A description a human wrote becomes the spec, and the disclosure line goes on top of it.
pub fn body_planned(body: &str, slices: &str) -> Result<String> {
    if is_planned(body) {
        bail!("the unit already has slices");
    }
    let body = with_disclosure(body);
    Ok(match body.find(ORIGINAL) {
        Some(at) => format!(
            "{}\n\n{}\n\n{}\n",
            body[..at].trim_end(),
            slices,
            body[at..].trim_end()
        ),
        None => format!("{body}\n\n{slices}\n"),
    })
}

/// `body` with the disclosure line on top, unless it has one.
fn with_disclosure(body: &str) -> String {
    if body.trim_start().starts_with('🤖') {
        body.trim().to_string()
    } else {
        format!("{DISCLOSURE}\n\n{}", body.trim())
    }
}

/// Whether `body` already has a diagnosis.
pub fn is_diagnosed(body: &str) -> bool {
    body.lines().any(|l| l.trim() == DIAGNOSIS)
}

/// `body` with the `diagnosis` of a troubleshooting session added at the end.
pub fn body_diagnosed(body: &str, diagnosis: &str) -> Result<String> {
    if is_diagnosed(body) {
        bail!("the unit already has a diagnosis");
    }
    Ok(format!(
        "{}\n\n{DIAGNOSIS}\n\n{}\n",
        with_disclosure(body),
        diagnosis.trim()
    ))
}

/// The slices a planning session answered with: its `## Slices` section.
pub fn slices_parse(reply: &str) -> Result<String> {
    let lines: Vec<&str> = reply.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.trim() == SLICES)
        .context("the answer has no ## Slices section")?;
    let slices = lines[start..].join("\n").trim().to_string();
    let numbered = slices.lines().any(|l| {
        let digits = l.chars().take_while(char::is_ascii_digit).count();
        digits > 0 && l[digits..].starts_with(". ")
    });
    if !numbered {
        bail!("the ## Slices section lists no slice");
    }
    Ok(slices)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLICES_TEXT: &str =
        "## Slices\n\n1. **A**: does a. Proof: test a.\n2. **B**: does b. Proof: test b.";

    #[test]
    fn a_defined_unit_keeps_what_it_was_below_the_spec() {
        let body = body_defined("Spec.", "Vague.");
        assert!(body.starts_with(DISCLOSURE));
        assert!(body.contains("Spec.\n"));
        assert!(body.ends_with("Vague.\n\n</details>\n"));
        assert!(!body_defined("Spec.", "  ").contains("<details>"));
    }

    #[test]
    fn slices_go_above_the_original_description() {
        let body = body_planned(&body_defined("Spec.", "Vague."), SLICES_TEXT).unwrap();
        let (slices, original) = (
            body.find("## Slices").unwrap(),
            body.find("<details>").unwrap(),
        );
        assert!(body.find("Spec.").unwrap() < slices && slices < original);
        assert!(is_planned(&body));
    }

    #[test]
    fn a_description_a_human_wrote_becomes_the_spec() {
        let body = body_planned("Make the thing.", SLICES_TEXT).unwrap();
        assert!(body.starts_with(DISCLOSURE));
        assert!(body.contains("Make the thing.\n\n## Slices"));
        assert!(!body.contains("<details>"));
    }

    #[test]
    fn a_unit_with_slices_is_not_planned_again() {
        let body = body_planned("Spec.", SLICES_TEXT).unwrap();
        assert!(body_planned(&body, SLICES_TEXT).is_err());
    }

    #[test]
    fn a_diagnosis_goes_at_the_end_once() {
        let body = body_diagnosed("It crashes.", "### Repro\nrun it").unwrap();
        assert!(body.starts_with(DISCLOSURE));
        assert!(body.ends_with("It crashes.\n\n## Diagnosis\n\n### Repro\nrun it\n"));
        assert!(body_diagnosed(&body, "again").is_err());
    }

    #[test]
    fn the_slices_are_the_section_of_the_answer() {
        let reply = format!("Here you go.\n\n{SLICES_TEXT}\n");
        assert_eq!(slices_parse(&reply).unwrap(), SLICES_TEXT);
        assert!(slices_parse("1. **A**: x.").is_err());
        assert!(slices_parse("## Slices\n\nnone yet").is_err());
    }
}
