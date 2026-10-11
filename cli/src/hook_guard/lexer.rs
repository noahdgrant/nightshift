//! Split a shell command into words and operators for the hook guard.

use std::iter::Peekable;
use std::str::Chars;

#[derive(Debug, PartialEq, Clone)]
pub(super) struct Word {
    pub(super) text: String,
    /// An expansion the guard can't resolve may stand where a flag or command name goes.
    pub(super) opaque: bool,
}

impl Word {
    pub(super) fn literal(text: &str) -> Self {
        Word {
            text: text.to_string(),
            opaque: false,
        }
    }
}

#[derive(Debug, PartialEq)]
pub(super) enum Tok {
    Word(Word),
    /// `>`, `>>`, `>|`, `&>`: the next word is a file written.
    RedirectOut,
    /// `<`: the next word is a file read.
    RedirectIn,
    /// `;`, `&&`, `|`, a newline or a brace: one command ends.
    CommandEnd,
    /// `(`: one command ends and a subshell begins, so a `cd` inside it is undone at `)`.
    SubshellOpen,
    /// `)`: the subshell ends.
    SubshellClose,
}

#[derive(Default)]
struct WordBuf {
    text: String,
    quoted: bool,
    /// Where in `text` the first expansion sits.
    expansion_at: Option<usize>,
    /// Where an unquoted `{` opened in `text`.
    brace_at: Option<usize>,
}

impl WordBuf {
    fn mark_expansion(&mut self) {
        self.expansion_at.get_or_insert(self.text.len());
    }

    fn finish(self) -> Word {
        let opaque = self.expansion_at.is_some_and(|at| {
            let before = &self.text[..at];
            at == 0 || before == "-" || (before.starts_with("--") && !before.contains('='))
        });
        Word {
            text: self.text,
            opaque,
        }
    }
}

struct Heredoc {
    delimiter: String,
    quoted: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Close {
    End,
    Paren,
    Backtick,
}

pub(super) struct Lexed {
    pub(super) main: Vec<Tok>,
    /// Each `$(...)`, backtick or `<(...)` body, flattened.
    pub(super) substitutions: Vec<Vec<Tok>>,
}

struct Lexer<'a> {
    chars: Peekable<Chars<'a>>,
    out: Vec<Tok>,
    substitutions: Vec<Vec<Tok>>,
    cur: Option<WordBuf>,
    pending_heredocs: Vec<Heredoc>,
    next_word_is_delimiter: bool,
}

/// Split a shell command into words and operators. Quotes join and are removed, so a bypass
/// mentioned inside a quoted string is one word, never a command. Heredoc bodies are skipped.
/// A command substitution is lexed apart, so it never splits the command around it.
pub(super) fn tokens(s: &str) -> Lexed {
    let mut lx = Lexer::new(s);
    lx.run(Close::End);
    lx.end_word();
    Lexed {
        main: lx.out,
        substitutions: lx.substitutions,
    }
}

impl<'a> Lexer<'a> {
    fn new(s: &'a str) -> Self {
        Lexer {
            chars: s.chars().peekable(),
            out: Vec::new(),
            substitutions: Vec::new(),
            cur: None,
            pending_heredocs: Vec::new(),
            next_word_is_delimiter: false,
        }
    }

    fn buf(&mut self) -> &mut WordBuf {
        self.cur.get_or_insert_with(WordBuf::default)
    }

    fn push(&mut self, c: char) {
        self.buf().text.push(c);
    }

    fn end_word(&mut self) {
        let Some(buf) = self.cur.take() else {
            return;
        };
        if std::mem::take(&mut self.next_word_is_delimiter) {
            self.pending_heredocs.push(Heredoc {
                delimiter: buf.text,
                quoted: buf.quoted,
            });
        } else {
            self.out.push(Tok::Word(buf.finish()));
        }
    }

    fn command_end(&mut self) {
        self.end_word();
        self.out.push(Tok::CommandEnd);
    }

    fn blank_follows(&mut self) -> bool {
        self.chars
            .peek()
            .is_none_or(|c| matches!(c, ' ' | '\t' | '\n' | ';'))
    }

    fn run(&mut self, close: Close) {
        let mut depth = 0usize;
        while let Some(c) = self.chars.next() {
            match c {
                '\'' => self.single_quoted(),
                '"' => self.double_quoted(),
                '\\' => self.escape(),
                '$' => self.dollar(false),
                '#' if self.cur.is_none() => self.comment(),
                ' ' | '\t' => self.end_word(),
                '\n' => self.newline(),
                '`' if close == Close::Backtick => return,
                '`' => self.expansion(Close::Backtick),
                '(' => {
                    depth += 1;
                    self.end_word();
                    self.out.push(Tok::SubshellOpen);
                }
                ')' if close == Close::Paren && depth == 0 => return,
                ')' => {
                    depth = depth.saturating_sub(1);
                    self.end_word();
                    self.out.push(Tok::SubshellClose);
                }
                ';' | '|' => self.command_end(),
                '&' => self.ampersand(),
                '>' => self.greater(),
                '<' => self.less(),
                '{' | '}' if self.cur.is_none() && self.blank_follows() => self.command_end(),
                '{' => {
                    let buf = self.buf();
                    buf.brace_at.get_or_insert(buf.text.len());
                    buf.text.push('{');
                }
                ',' => {
                    let buf = self.buf();
                    if let Some(at) = buf.brace_at {
                        buf.expansion_at.get_or_insert(at);
                    }
                    buf.text.push(',');
                }
                c => self.push(c),
            }
        }
    }

    fn single_quoted(&mut self) {
        self.buf().quoted = true;
        for c in self.chars.by_ref() {
            if c == '\'' {
                return;
            }
            self.cur.get_or_insert_with(WordBuf::default).text.push(c);
        }
    }

    fn double_quoted(&mut self) {
        self.buf().quoted = true;
        while let Some(c) = self.chars.next() {
            match c {
                '"' => return,
                '\\' => match self.chars.peek().copied() {
                    Some(escaped @ ('$' | '`' | '"' | '\\')) => {
                        self.chars.next();
                        self.push(escaped);
                    }
                    Some('\n') => {
                        self.chars.next();
                    }
                    _ => self.push('\\'),
                },
                '$' => self.dollar(true),
                '`' => self.expansion(Close::Backtick),
                c => self.push(c),
            }
        }
    }

    fn escape(&mut self) {
        self.buf().quoted = true;
        if let Some(c) = self.chars.next().filter(|c| *c != '\n') {
            self.push(c);
        }
    }

    fn comment(&mut self) {
        while self.chars.next_if(|c| *c != '\n').is_some() {}
    }

    fn dollar(&mut self, in_double_quotes: bool) {
        match self.chars.peek().copied() {
            Some('\'') if !in_double_quotes => {
                self.chars.next();
                self.ansi_c_quoted();
            }
            Some('"') if !in_double_quotes => {}
            Some('(') => {
                self.chars.next();
                self.expansion(Close::Paren);
            }
            Some('{') => {
                self.chars.next();
                self.buf().mark_expansion();
            }
            Some(c) if c.is_ascii_alphanumeric() || c == '_' => {
                while self
                    .chars
                    .next_if(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .is_some()
                {}
                self.buf().mark_expansion();
            }
            Some(c) if "@*#?!$-".contains(c) => {
                self.chars.next();
                self.buf().mark_expansion();
            }
            _ => self.push('$'),
        }
    }

    fn ansi_c_quoted(&mut self) {
        self.buf().quoted = true;
        while let Some(c) = self.chars.next() {
            match c {
                '\'' => return,
                '\\' => {
                    let decoded = self.ansi_c_escape();
                    self.buf().text.push_str(&decoded);
                }
                c => self.push(c),
            }
        }
    }

    fn ansi_c_escape(&mut self) -> String {
        let simple = |c: char| c.to_string();
        match self.chars.next() {
            None => "\\".into(),
            Some('n') => simple('\n'),
            Some('t') => simple('\t'),
            Some('r') => simple('\r'),
            Some('a') => simple('\x07'),
            Some('b') => simple('\x08'),
            Some('e' | 'E') => simple('\x1b'),
            Some('f') => simple('\x0c'),
            Some('v') => simple('\x0b'),
            Some(c @ ('\\' | '\'' | '"' | '?')) => simple(c),
            Some('x') => self.numeric_escape(16, 2, String::new()),
            Some('u') => self.numeric_escape(16, 4, String::new()),
            Some('U') => self.numeric_escape(16, 8, String::new()),
            Some(d @ '0'..='7') => self.numeric_escape(8, 3, d.to_string()),
            Some(c) => format!("\\{c}"),
        }
    }

    fn numeric_escape(&mut self, radix: u32, max: usize, mut digits: String) -> String {
        while digits.len() < max {
            match self.chars.next_if(|c| c.is_digit(radix)) {
                Some(c) => digits.push(c),
                None => break,
            }
        }
        let code = u32::from_str_radix(&digits, radix).unwrap_or(0xfffd);
        char::from_u32(code).unwrap_or('\u{fffd}').to_string()
    }

    /// Lex a command substitution apart from the command around it.
    fn substitution(&mut self, close: Close) {
        let outer_out = std::mem::take(&mut self.out);
        let outer_word = self.cur.take();
        let outer_delimiter = std::mem::take(&mut self.next_word_is_delimiter);
        self.run(close);
        self.end_word();
        let inner = std::mem::replace(&mut self.out, outer_out);
        self.substitutions.push(inner);
        self.cur = outer_word;
        self.next_word_is_delimiter = outer_delimiter;
    }

    fn expansion(&mut self, close: Close) {
        self.substitution(close);
        self.buf().mark_expansion();
    }

    fn process_substitution(&mut self) {
        self.substitution(Close::Paren);
        self.buf().text.push_str("/dev/fd/63");
    }

    fn fd_prefix(&mut self) {
        let is_fd = self.cur.as_ref().is_some_and(|b| {
            !b.quoted && !b.text.is_empty() && b.text.bytes().all(|c| c.is_ascii_digit())
        });
        if is_fd {
            self.cur = None;
        }
    }

    fn duplicated_descriptor(&mut self) {
        while self
            .chars
            .next_if(|c| c.is_ascii_digit() || *c == '-')
            .is_some()
        {}
    }

    fn ampersand(&mut self) {
        if self.chars.next_if_eq(&'>').is_some() {
            self.chars.next_if_eq(&'>');
            self.end_word();
            self.out.push(Tok::RedirectOut);
        } else {
            self.command_end();
        }
    }

    fn greater(&mut self) {
        self.fd_prefix();
        self.end_word();
        if self.chars.next_if_eq(&'(').is_some() {
            self.process_substitution();
            return;
        }
        self.chars.next_if(|c| *c == '>' || *c == '|');
        if self.chars.next_if_eq(&'&').is_some()
            && self
                .chars
                .peek()
                .is_some_and(|c| c.is_ascii_digit() || *c == '-')
        {
            self.duplicated_descriptor();
            return;
        }
        self.out.push(Tok::RedirectOut);
    }

    fn less(&mut self) {
        self.fd_prefix();
        self.end_word();
        if self.chars.next_if_eq(&'(').is_some() {
            self.process_substitution();
        } else if self.chars.next_if_eq(&'<').is_none() {
            if self.chars.next_if_eq(&'&').is_some() {
                self.duplicated_descriptor();
            } else {
                self.out.push(Tok::RedirectIn);
            }
        } else if self.chars.next_if_eq(&'<').is_some() {
            self.out.push(Tok::RedirectIn);
        } else {
            self.chars.next_if_eq(&'-');
            self.next_word_is_delimiter = true;
        }
    }

    fn newline(&mut self) {
        self.command_end();
        for heredoc in std::mem::take(&mut self.pending_heredocs) {
            self.skip_heredoc_body(&heredoc);
        }
    }

    fn skip_heredoc_body(&mut self, heredoc: &Heredoc) {
        let mut body = String::new();
        let mut line = String::new();
        for c in self.chars.by_ref() {
            if c != '\n' {
                line.push(c);
            } else if line.trim_start_matches('\t') == heredoc.delimiter {
                break;
            } else {
                body.push_str(&line);
                body.push('\n');
                line.clear();
            }
        }
        if !heredoc.quoted {
            let mut body_lexer = Lexer::new(&body);
            body_lexer.find_substitutions();
            self.substitutions.append(&mut body_lexer.substitutions);
        }
    }

    /// Collect the substitutions in text the shell expands but doesn't split, a heredoc body.
    fn find_substitutions(&mut self) {
        while let Some(c) = self.chars.next() {
            match c {
                '\\' => {
                    self.chars.next();
                }
                '$' if self.chars.next_if_eq(&'(').is_some() => self.substitution(Close::Paren),
                '`' => self.substitution(Close::Backtick),
                _ => {}
            }
        }
    }
}
