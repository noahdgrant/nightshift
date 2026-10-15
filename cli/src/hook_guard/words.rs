//! A Bash command split into simple commands and their words, as far as the guard reads it:
//! quotes are removed and remembered, comments and heredoc bodies are skipped, and each
//! output redirect's target is set apart. Expansions stay as written; nothing is resolved.

/// One shell word: its text with the quotes removed, and whether any of it was quoted.
#[derive(Debug, Default, Clone, PartialEq)]
pub(super) struct Word {
    pub text: String,
    pub quoted: bool,
}

/// One simple command: its words, and the files its output redirects write.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Segment {
    pub words: Vec<Word>,
    pub redirects: Vec<Word>,
}

/// Where the next word goes.
#[derive(Clone, Copy, PartialEq)]
enum Next {
    Arg,
    /// An output redirect's target.
    Out,
    /// An input redirect's source, which the guard drops.
    In,
    /// A heredoc's delimiter; `true` when `<<-` strips leading tabs.
    Delimiter(bool),
}

struct Splitter {
    done: Vec<Segment>,
    seg: Segment,
    word: Option<Word>,
    next: Next,
    /// Heredocs whose bodies start at the next newline: the delimiter, and whether tabs go.
    heredocs: Vec<(String, bool)>,
}

impl Splitter {
    fn word(&mut self) -> &mut Word {
        self.word.get_or_insert_with(Word::default)
    }

    fn end_word(&mut self) {
        let Some(w) = self.word.take() else {
            return;
        };
        match std::mem::replace(&mut self.next, Next::Arg) {
            Next::Arg => self.seg.words.push(w),
            Next::Out => self.seg.redirects.push(w),
            Next::In => {}
            Next::Delimiter(tabs) => self.heredocs.push((w.text, tabs)),
        }
    }

    fn end_segment(&mut self) {
        self.end_word();
        let seg = std::mem::take(&mut self.seg);
        if !seg.words.is_empty() || !seg.redirects.is_empty() {
            self.done.push(seg);
        }
    }
}

/// The simple commands in `command`.
pub(super) fn segments(command: &str) -> Vec<Segment> {
    let mut s = Splitter {
        done: Vec::new(),
        seg: Segment::default(),
        word: None,
        next: Next::Arg,
        heredocs: Vec::new(),
    };
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => s.end_word(),
            '\n' => {
                s.end_segment();
                for (delim, tabs) in std::mem::take(&mut s.heredocs) {
                    skip_body(&mut chars, &delim, tabs);
                }
            }
            ';' | '|' | '&' | '(' | ')' if c != '&' || chars.peek() != Some(&'>') => {
                s.end_segment();
            }
            '#' if s.word.is_none() => while chars.next_if(|c| *c != '\n').is_some() {},
            '\'' => {
                let w = s.word();
                w.quoted = true;
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    w.text.push(c);
                }
            }
            '"' => {
                let w = s.word();
                w.quoted = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' if chars.peek().is_some_and(|n| "\"\\$`".contains(*n)) => {
                            w.text.extend(chars.next());
                        }
                        c => w.text.push(c),
                    }
                }
            }
            '\\' => match chars.next() {
                Some('\n') | None => {}
                Some(n) => s.word().text.push(n),
            },
            '>' | '&' => {
                // A word of digits before `>` is the file descriptor it redirects.
                if s.word
                    .as_ref()
                    .is_some_and(|w| !w.quoted && w.text.bytes().all(|b| b.is_ascii_digit()))
                {
                    s.word = None;
                }
                s.end_word();
                if c == '&' {
                    chars.next();
                }
                while chars.next_if(|n| matches!(n, '>' | '|' | '&')).is_some() {}
                s.next = Next::Out;
            }
            '<' => {
                s.end_word();
                if chars.next_if_eq(&'<').is_some() {
                    if chars.next_if_eq(&'<').is_some() {
                        s.next = Next::In;
                    } else {
                        s.next = Next::Delimiter(chars.next_if_eq(&'-').is_some());
                    }
                } else {
                    chars.next_if_eq(&'&');
                    s.next = Next::In;
                }
            }
            c => s.word().text.push(c),
        }
    }
    s.end_segment();
    s.done
}

/// Skip a heredoc's body: every line up to and including the one that is `delim`.
fn skip_body(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, delim: &str, tabs: bool) {
    loop {
        let mut line = String::new();
        let mut ended = false;
        for c in chars.by_ref() {
            if c == '\n' {
                ended = true;
                break;
            }
            line.push(c);
        }
        let line = if tabs {
            line.trim_start_matches('\t')
        } else {
            &line
        };
        if line == delim || !ended {
            return;
        }
    }
}
