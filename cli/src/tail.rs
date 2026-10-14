//! Read the end of a file.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// The last `n` bytes of `path`, from a line start unless the whole file fits or they hold no
/// line break.
pub fn tail(path: &Path, n: u64) -> std::io::Result<Vec<u8>> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let start = len.saturating_sub(n);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    if start > 0 {
        if let Some(i) = buf.iter().position(|b| *b == b'\n') {
            buf.drain(..=i);
        }
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::tail;

    fn file(body: &[u8]) -> tempfile::NamedTempFile {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), body).unwrap();
        f
    }

    #[test]
    fn missing_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let e = tail(&dir.path().join("absent"), 10).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn short_file_is_returned_whole() {
        let f = file(b"one\ntwo\n");
        assert_eq!(tail(f.path(), 100).unwrap(), b"one\ntwo\n");
    }

    #[test]
    fn exact_fit_is_returned_whole() {
        let f = file(b"one\ntwo\n");
        assert_eq!(tail(f.path(), 8).unwrap(), b"one\ntwo\n");
    }

    #[test]
    fn window_starts_after_the_first_line_break() {
        let f = file(b"one\ntwo\nthree\n");
        assert_eq!(tail(f.path(), 9).unwrap(), b"three\n");
    }

    #[test]
    fn window_without_a_line_break_is_returned_as_is() {
        let f = file(b"one\nlongline");
        assert_eq!(tail(f.path(), 5).unwrap(), b"gline");
    }
}
