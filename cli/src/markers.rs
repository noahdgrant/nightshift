//! The human-review markers a file can carry: a start marker with an optional reason after a
//! colon, and an end marker, each on a line of a comment in any syntax. The lines between a start
//! and its end are a region a person must review. This is the one place that reads them. The
//! marker text is built from parts so that this repository's own files hold no marker.

use std::fmt;

pub const START: &str = concat!("ns:human-", "review start");
pub const END: &str = concat!("ns:human-", "review end");

/// A region a person must review: the lines from its `start` marker to its `end` marker.
#[derive(Debug, PartialEq, Eq)]
pub struct Region {
    /// The 1-based line of the `start` marker.
    pub start: usize,
    /// The 1-based line of the `end` marker.
    pub end: usize,
    /// The text after `start:`, empty when there is none.
    pub reason: String,
}

/// A marker that cannot be read, with the 1-based line it is on.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    StartWithoutEnd { line: usize },
    EndWithoutStart { line: usize },
    StartInsideRegion { line: usize, open: usize },
}

impl Error {
    pub fn line(&self) -> usize {
        match self {
            Error::StartWithoutEnd { line }
            | Error::EndWithoutStart { line }
            | Error::StartInsideRegion { line, .. } => *line,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::StartWithoutEnd { .. } => write!(f, "`{START}` has no `{END}`"),
            Error::EndWithoutStart { .. } => write!(f, "`{END}` has no `{START}`"),
            Error::StartInsideRegion { open, .. } => write!(
                f,
                "`{START}` inside the region started on line {open}; regions do not nest"
            ),
        }
    }
}

/// The regions in `text`, in order, or the first marker that cannot be read.
pub fn regions_read(text: &str) -> Result<Vec<Region>, Error> {
    let mut regions = Vec::new();
    let mut open: Option<(usize, String)> = None;
    for (index, line) in text.lines().enumerate() {
        let line_no = index + 1;
        if let Some(at) = line.find(START) {
            if let Some((started, _)) = open {
                return Err(Error::StartInsideRegion {
                    line: line_no,
                    open: started,
                });
            }
            open = Some((line_no, reason_read(&line[at + START.len()..])));
        } else if line.contains(END) {
            let Some((start, reason)) = open.take() else {
                return Err(Error::EndWithoutStart { line: line_no });
            };
            regions.push(Region {
                start,
                end: line_no,
                reason,
            });
        }
    }
    match open {
        Some((line, _)) => Err(Error::StartWithoutEnd { line }),
        None => Ok(regions),
    }
}

/// The reason after `start`: the text after a colon, without a comment closer such as `-->`.
fn reason_read(rest: &str) -> String {
    let Some(reason) = rest.trim_start().strip_prefix(':') else {
        return String::new();
    };
    let reason = reason.trim();
    let reason = ["-->", "*/"]
        .iter()
        .find_map(|closer| reason.strip_suffix(closer))
        .unwrap_or(reason);
    reason.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `text` with `@start` and `@end` written as the markers.
    fn read(text: &str) -> Result<Vec<Region>, Error> {
        regions_read(&text.replace("@start", START).replace("@end", END))
    }

    fn region(start: usize, end: usize, reason: &str) -> Region {
        Region {
            start,
            end,
            reason: reason.to_string(),
        }
    }

    #[test]
    fn a_file_without_markers_has_no_regions() {
        assert_eq!(read("fn main() {}\n\nlet x = 1;\n"), Ok(vec![]));
        assert_eq!(read(""), Ok(vec![]));
    }

    #[test]
    fn one_region_is_read_with_its_lines() {
        let text = "a\n// @start\nb\n// @end\nc\n";
        assert_eq!(read(text), Ok(vec![region(2, 4, "")]));
    }

    #[test]
    fn two_regions_are_read_in_order() {
        let text = "# @start: one\nx\n# @end\n\n# @start: two\n# @end\n";
        assert_eq!(
            read(text),
            Ok(vec![region(1, 3, "one"), region(5, 6, "two")])
        );
    }

    #[test]
    fn a_reason_is_the_text_after_the_colon_and_may_be_empty() {
        let text =
            "// @start: touches billing \n// @end\n// @start:\n// @end\n// @start\n// @end\n";
        assert_eq!(
            read(text),
            Ok(vec![
                region(1, 2, "touches billing"),
                region(3, 4, ""),
                region(5, 6, "")
            ])
        );
    }

    #[test]
    fn markers_are_read_in_any_comment_syntax() {
        let text = "<!-- @start: markup -->\n<p>x</p>\n<!-- @end -->\n/* @start: block */\n/* @end */\n-- @start\n-- @end\n";
        assert_eq!(
            read(text),
            Ok(vec![
                region(1, 3, "markup"),
                region(4, 5, "block"),
                region(6, 7, "")
            ])
        );
    }

    #[test]
    fn a_start_without_an_end_names_the_start() {
        let err = read("a\nb\n// @start\nc\n").unwrap_err();
        assert_eq!(err, Error::StartWithoutEnd { line: 3 });
        assert_eq!(err.line(), 3);
    }

    #[test]
    fn an_end_without_a_start_names_the_end() {
        let err = read("a\n# @end\n").unwrap_err();
        assert_eq!(err, Error::EndWithoutStart { line: 2 });
        let after = read("# @start\n# @end\n# @end\n");
        assert_eq!(after, Err(Error::EndWithoutStart { line: 3 }));
    }

    #[test]
    fn a_start_inside_a_region_names_the_inner_start() {
        let text = "// @start\nx\n// @start: inner\n// @end\n";
        assert_eq!(
            read(text),
            Err(Error::StartInsideRegion { line: 3, open: 1 })
        );
    }
}
