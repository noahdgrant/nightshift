//! The numbered files a checking state keeps, `<kind>-<n>.md` with `n` counting from 1, one per
//! run of the check. The next run reads the last one back when it failed or was inconclusive.

use std::path::Path;

use super::verdict::{self, Verdict};
use crate::artifact;

/// The numbered files of one kind (`review`, `verify`) for one unit.
pub struct Rounds<'a> {
    dir: &'a Path,
    unit: &'a str,
    kind: &'a str,
}

impl<'a> Rounds<'a> {
    pub fn new(dir: &'a Path, unit: &'a str, kind: &'a str) -> Self {
        Rounds { dir, unit, kind }
    }

    fn file(&self, n: usize) -> String {
        format!("{}-{n}.md", self.kind)
    }

    /// How many runs the unit has had: the highest `n` with a file.
    pub fn so_far(&self) -> usize {
        let mut n = 0;
        while artifact::read(self.dir, self.unit, &self.file(n + 1)).is_ok() {
            n += 1;
        }
        n
    }

    /// The text of the last run, when it ended in a failing or inconclusive verdict.
    pub fn last_unmet(&self) -> Option<String> {
        let n = self.so_far();
        if n == 0 {
            return None;
        }
        artifact::read(self.dir, self.unit, &self.file(n))
            .ok()
            .filter(|text| {
                matches!(
                    verdict::parse(text),
                    Some(Verdict::Fail | Verdict::Inconclusive)
                )
            })
    }

    /// Keeps `text` as the next run.
    pub fn keep(&self, text: &str) -> anyhow::Result<()> {
        artifact::write(self.dir, self.unit, &self.file(self.so_far() + 1), text)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_unmet_last_run_is_carried_forward() {
        let tmp = tempfile::tempdir().unwrap();
        let rounds = Rounds::new(tmp.path(), "7", "review");
        assert_eq!(rounds.so_far(), 0);
        assert_eq!(rounds.last_unmet(), None);
        rounds.keep("bad\nVERDICT: FAIL\n").unwrap();
        assert_eq!(rounds.so_far(), 1);
        assert_eq!(rounds.last_unmet().as_deref(), Some("bad\nVERDICT: FAIL\n"));
        rounds.keep("hm\nVERDICT: INCONCLUSIVE\n").unwrap();
        assert_eq!(
            rounds.last_unmet().as_deref(),
            Some("hm\nVERDICT: INCONCLUSIVE\n")
        );
        rounds.keep("ok\nVERDICT: PASS\n").unwrap();
        assert_eq!(rounds.so_far(), 3);
        assert_eq!(rounds.last_unmet(), None);
    }

    #[test]
    fn kinds_are_counted_apart() {
        let tmp = tempfile::tempdir().unwrap();
        Rounds::new(tmp.path(), "7", "review").keep("x").unwrap();
        assert_eq!(Rounds::new(tmp.path(), "7", "verify").so_far(), 0);
        Rounds::new(tmp.path(), "7", "verify").keep("y").unwrap();
        assert!(artifact::read(tmp.path(), "7", "verify-1.md").is_ok());
    }
}
