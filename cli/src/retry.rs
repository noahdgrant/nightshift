//! Repeating a call that failed for a reason that can pass, as `docs/CODING_STANDARDS.md` says
//! under "Retry what can pass, where it is safe". The caller decides which calls are safe to
//! repeat and which failures can pass.

use std::time::Duration;

use anyhow::Result;

/// How many times a call is made before its last result is returned.
pub const ATTEMPTS: u32 = 3;

/// How long to wait before the second attempt, when nothing says otherwise.
pub const DELAY: Duration = Duration::from_secs(2);

/// Makes `call` until `transient` finds nothing to wait out in its result, or `ATTEMPTS` calls
/// have been made. `transient` returns why the result is worth another attempt. The wait doubles
/// after each attempt, and each reason is printed.
pub fn retry<T>(
    delay: Duration,
    mut call: impl FnMut() -> Result<T>,
    transient: impl Fn(&Result<T>) -> Option<String>,
) -> Result<T> {
    let mut delay = delay;
    let mut attempt = 1;
    loop {
        let result = call();
        match transient(&result) {
            Some(why) if attempt < ATTEMPTS => {
                eprintln!("ns: {why}; trying again in {}s", delay.as_secs());
                std::thread::sleep(delay);
                delay *= 2;
                attempt += 1;
            }
            _ => return result,
        }
    }
}

/// The error of `result`, when its text holds any of `known`, ignoring case.
pub fn error_says<T>(result: &Result<T>, known: &[&str]) -> Option<String> {
    let said = format!("{:#}", result.as_ref().err()?);
    says(&said, known).then_some(said)
}

/// Whether `text` holds any of `known`, ignoring case.
pub fn says(text: &str, known: &[&str]) -> bool {
    let text = text.to_lowercase();
    known.iter().any(|k| text.contains(k))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_is_repeated_until_it_passes_or_the_attempts_run_out() {
        let mut calls = 0;
        let result = retry(
            Duration::ZERO,
            || {
                calls += 1;
                if calls < 3 {
                    anyhow::bail!("down")
                }
                Ok(calls)
            },
            |r| error_says(r, &["down"]),
        );
        assert_eq!(result.unwrap(), 3);

        let mut calls = 0;
        let result: Result<()> = retry(
            Duration::ZERO,
            || {
                calls += 1;
                anyhow::bail!("down")
            },
            |r| error_says(r, &["down"]),
        );
        assert!(result.is_err());
        assert_eq!(calls, ATTEMPTS);
    }

    #[test]
    fn an_error_that_cannot_pass_is_returned_at_once() {
        let mut calls = 0;
        let result: Result<()> = retry(
            Duration::ZERO,
            || {
                calls += 1;
                anyhow::bail!("not found")
            },
            |r| error_says(r, &["down"]),
        );
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }
}
