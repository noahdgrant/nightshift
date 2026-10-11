//! Commands that run another command (`sudo`, `env`, `xargs`, `sh -c`...) and how to find it.

use super::*;

pub(super) const RESERVED_WORDS: &[&str] = &[
    "if", "then", "else", "elif", "do", "while", "until", "!", "coproc",
];

/// A command that runs the command after its own options. It lists the options that take a
/// value, so the value is not mistaken for the program.
pub(super) struct Wrapper {
    pub(super) name: &'static str,
    /// Letters of the short options that take a value.
    pub(super) short_values: &'static str,
    pub(super) long_values: &'static [&'static str],
    /// Positional arguments before the command (the duration of `timeout`).
    pub(super) positional: usize,
}

pub(super) const fn wrapper(name: &'static str, short_values: &'static str) -> Wrapper {
    Wrapper {
        name,
        short_values,
        long_values: &[],
        positional: 0,
    }
}

pub(super) const WRAPPERS: &[Wrapper] = &[
    wrapper("export", ""),
    wrapper("declare", ""),
    wrapper("typeset", ""),
    wrapper("readonly", ""),
    wrapper("local", ""),
    wrapper("command", ""),
    wrapper("nohup", ""),
    wrapper("builtin", ""),
    wrapper("setsid", ""),
    wrapper("doas", "uC"),
    wrapper("exec", "a"),
    Wrapper {
        long_values: &["--unset", "--chdir"],
        ..wrapper("env", "uC")
    },
    Wrapper {
        long_values: &[
            "--user",
            "--group",
            "--host",
            "--prompt",
            "--close-from",
            "--chdir",
            "--role",
            "--type",
            "--other-user",
            "--command-timeout",
            "--chroot",
        ],
        ..wrapper("sudo", "ughpCrtTUDR")
    },
    Wrapper {
        long_values: &["--adjustment"],
        ..wrapper("nice", "n")
    },
    Wrapper {
        long_values: &["--format", "--output"],
        ..wrapper("time", "fo")
    },
    Wrapper {
        long_values: &["--input", "--output", "--error"],
        ..wrapper("stdbuf", "ioe")
    },
    Wrapper {
        long_values: &["--class", "--classdata", "--pid", "--pgid", "--uid"],
        ..wrapper("ionice", "cnpPu")
    },
    Wrapper {
        long_values: &[
            "--max-args",
            "--max-lines",
            "--max-procs",
            "--max-chars",
            "--delimiter",
            "--arg-file",
            "--process-slot-var",
        ],
        ..wrapper("xargs", "nLPIsdEa")
    },
    Wrapper {
        long_values: &["--kill-after", "--signal"],
        positional: 1,
        ..wrapper("timeout", "ks")
    },
];

/// The words after a command's leading options, skipping the value of each option that takes one.
pub(super) fn skip_options<'a>(mut words: &'a [Word], wrap: &Wrapper) -> &'a [Word] {
    while let Some(first) = words.first() {
        let arg = first.text.as_str();
        let width = if arg == "--" {
            return &words[1..];
        } else if arg.starts_with("--") {
            if !arg.contains('=') && wrap.long_values.contains(&arg) {
                2
            } else {
                1
            }
        } else if let Some(cluster) = arg.strip_prefix('-').filter(|c| !c.is_empty()) {
            let value_flag = cluster
                .char_indices()
                .find(|(_, c)| wrap.short_values.contains(*c));
            match value_flag {
                Some((i, c)) if i + c.len_utf8() == cluster.len() => 2,
                _ => 1,
            }
        } else {
            return words;
        };
        words = words.get(width..).unwrap_or(&[]);
    }
    words
}

/// The command string of `env -S`, which env splits and runs. Walks env's options with their
/// values, so `-iS`, `-u FOO -S` and `-C /x -S` all reach it.
pub(super) fn env_split_string(options: &[Word]) -> Option<&str> {
    let mut args = options.iter().map(|w| w.text.as_str());
    while let Some(arg) = args.next() {
        match arg {
            "--" => return None,
            "--split-string" => return args.next(),
            "--unset" | "--chdir" => {
                args.next();
            }
            _ if arg.starts_with("--") => {
                if let Some(v) = arg.strip_prefix("--split-string=") {
                    return Some(v);
                }
            }
            _ if arg.starts_with('-') && arg.len() > 1 => {
                for (i, c) in arg.char_indices().skip(1) {
                    let attached = &arg[i + c.len_utf8()..];
                    match c {
                        'S' if attached.is_empty() => return args.next(),
                        'S' => return Some(attached),
                        'u' | 'C' => {
                            if attached.is_empty() {
                                args.next();
                            }
                            break;
                        }
                        _ => {}
                    }
                }
            }
            _ => return None,
        }
    }
    None
}

/// The script word of `sh -c SCRIPT`: the word after the first option cluster with a `c`.
pub(super) fn shell_dash_c_script(args: &[Word]) -> Option<&Word> {
    let at = args.iter().position(|a| {
        a.text.starts_with('-') && !a.text.starts_with("--") && a.text.contains('c')
    })?;
    args.get(at + 1)
}
