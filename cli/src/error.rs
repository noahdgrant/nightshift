//! Error type that carries a process exit code and a correct invocation hint.

use std::fmt;

/// Exit code for usage errors (bad arguments, invalid unit id).
pub const EXIT_USAGE: i32 = 2;
/// Exit code when `ns ask` finds no role or default in the config.
pub const EXIT_NOT_CONFIGURED: i32 = 3;
/// Exit code when the harness binary is not on PATH.
pub const EXIT_HARNESS_MISSING: i32 = 4;
/// Exit code when `ns ask --write` targets a harness with no `command_write`.
pub const EXIT_NO_WRITE_COMMAND: i32 = 5;

#[derive(Debug, thiserror::Error)]
pub struct SfError {
    pub code: i32,
    pub message: String,
    /// Printed under the message: usually a correct invocation or config snippet.
    pub hint: Option<String>,
}

impl fmt::Display for SfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl SfError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: None,
        }
    }

    pub fn general(message: impl Into<String>) -> Self {
        Self::new(1, message)
    }

    pub fn usage(message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self::new(EXIT_USAGE, message).hint(hint)
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}
