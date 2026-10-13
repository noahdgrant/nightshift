//! Split a shell command into words and operators for the hook guard.

use std::iter::Peekable;
use std::str::Chars;

#[derive(Debug, PartialEq, Clone)]
pub(super) struct Word {
    pub(super) text: String,
    /// An expansion the guard can't resolve may stand where a flag or command name goes.
    pub(super) opaque: bool,
    /// The whole word is one double-quoted variable, `"$name"` or `"${name}"`.
    pub(super) quoted_var: bool,
    /// Each expansion the guard can't resolve, in order. `text` holds nothing in its place.
    pub(super) expansions: Vec<Expansion>,
    /// An expansion outside double quotes, one that lists words, or a brace can split the word
    /// or add words.
    pub(super) splits: bool,
    /// An unquoted `{` opened in the word.
    pub(super) braced: bool,
}

#[derive(Debug, PartialEq, Clone)]
pub(super) struct Expansion {
    /// Where in the word's `text` it sat.
    pub(super) at: usize,
    /// The name of a plain parameter, `$name` or `${name}`, in a word with no brace: one
    /// string. `None` for any other expansion.
    pub(super) name: Option<String>,
}

impl Word {
    pub(super) fn literal(text: &str) -> Self {
        Word {
            text: text.to_string(),
            opaque: false,
            quoted_var: false,
            expansions: Vec::new(),
            splits: false,
            braced: false,
        }
    }

    /// The word with each expansion `value` gives text for written in as that text. The word
    /// stays opaque only if an expansion left in it still hides a flag or a command, or a
    /// brace does.
    pub(super) fn resolved<'v>(&self, value: impl Fn(&Expansion) -> Option<&'v str>) -> Word {
        let mut text = String::new();
        let mut expansions = Vec::new();
        let mut from = 0;
        for e in &self.expansions {
            text.push_str(&self.text[from..e.at]);
            from = e.at;
            match value(e) {
                Some(v) => text.push_str(v),
                None => expansions.push(Expansion {
                    at: text.len(),
                    name: e.name.clone(),
                }),
            }
        }
        if expansions.len() == self.expansions.len() {
            return self.clone();
        }
        text.push_str(&self.text[from..]);
        let left = !expansions.is_empty();
        let opaque = if self.braced {
            self.opaque
        } else {
            expansions
                .first()
                .is_some_and(|e| hides_flag_or_command(&text[..e.at]))
        };
        Word {
            opaque,
            quoted_var: self.quoted_var && left,
            splits: self.braced || (self.splits && left),
            braced: self.braced,
            text,
            expansions,
        }
    }

    /// The text with each expansion as `fill(i)` for the `i`th one.
    pub(super) fn filled(&self, fill: impl Fn(usize) -> char) -> String {
        let mut out = String::new();
        let mut from = 0;
        for (i, e) in self.expansions.iter().enumerate() {
            out.push_str(&self.text[from..e.at]);
            out.push(fill(i));
            from = e.at;
        }
        out.push_str(&self.text[from..]);
        out
    }
}

#[derive(Debug, PartialEq)]
pub(super) enum Tok {
    Word(Word),
    /// `>`, `>>`, `>|`, `&>`: the next word is a file written.
    RedirectOut,
    /// `<`: the next word is a file read.
    RedirectIn,
    /// `<<<`: the next word is text on the command's stdin.
    Herestring,
    /// `<<` or `<<-`: a heredoc body is the command's stdin.
    Heredoc,
    /// `;`, `&&`, `|`, a newline or a brace: one command ends.
    CommandEnd(Sep),
    /// `(`: one command ends and a subshell begins, so a `cd` inside it is undone at `)`.
    SubshellOpen,
    /// `)`: the subshell ends.
    SubshellClose,
}

/// What ends a command, as far as it decides whether the next one runs in the same shell.
#[derive(Debug, PartialEq, Clone, Copy)]
pub(super) enum Sep {
    /// `;` or a newline: the next command runs whatever this one did.
    Seq,
    /// `&&`: the next command runs only after this one ran.
    And,
    /// `||`, `|` or `&`: the next command may run without this one, or either may run in a
    /// subshell.
    Branch,
    /// A brace or an arithmetic command: a compound command begins or ends.
    Group,
}

#[derive(Default)]
struct WordBuf {
    text: String,
    quoted: bool,
    /// Where in `text` the first expansion sits.
    expansion_at: Option<usize>,
    /// Where an unquoted `{` opened in `text`.
    brace_at: Option<usize>,
    /// The word is one double-quoted variable and nothing else.
    quoted_var: bool,
    in_double_quotes: bool,
    expansions: Vec<Expansion>,
    /// An expansion outside double quotes can split the word or add words.
    splits: bool,
}

impl WordBuf {
    fn mark_expansion(&mut self, name: Option<String>) {
        let at = self.text.len();
        self.expansion_at.get_or_insert(at);
        self.expansions.push(Expansion { at, name });
        self.splits |= !self.in_double_quotes;
    }

    fn finish(self) -> Word {
        let opaque = self
            .expansion_at
            .is_some_and(|at| hides_flag_or_command(&self.text[..at]));
        let braced = self.brace_at.is_some();
        let expansions = self
            .expansions
            .into_iter()
            .map(|e| Expansion {
                name: e.name.filter(|_| !braced),
                ..e
            })
            .collect();
        Word {
            text: self.text,
            opaque,
            quoted_var: self.quoted_var,
            expansions,
            splits: self.splits || braced,
            braced,
        }
    }
}

/// A braced body is one safe word only when it is a plain parameter name, which this returns.
/// Any operator, subscript, indirection or quote may list words, so the body is not trusted.
fn plain_name(body: &str) -> Option<String> {
    let name = body.strip_suffix('}')?;
    let mut chars = name.chars();
    let plain = match chars.next() {
        Some(c) if c.is_ascii_digit() => chars.all(|c| c.is_ascii_digit()),
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        _ => false,
    };
    plain.then(|| name.to_string())
}

/// The opacity rule: an expansion hides what the word is unless the text ahead of it ends an
/// `--option=` or an assignment, whose value is data. `before` is the text ahead of the
/// expansion.
fn hides_flag_or_command(before: &str) -> bool {
    !before.contains('=')
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
    /// Arithmetic (`((...))`, `$((...))`, `$[...]`) or a `${name=value}` form, which can set a
    /// variable in the shell without naming it in a word, is somewhere in the command.
    pub(super) assigns: bool,
}

struct Lexer<'a> {
    chars: Peekable<Chars<'a>>,
    out: Vec<Tok>,
    substitutions: Vec<Vec<Tok>>,
    cur: Option<WordBuf>,
    pending_heredocs: Vec<Heredoc>,
    next_word_is_delimiter: bool,
    in_arithmetic: bool,
    arithmetic_next: bool,
    assigns: bool,
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
        assigns: lx.assigns,
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
            in_arithmetic: false,
            arithmetic_next: false,
            assigns: false,
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

    fn command_end(&mut self, sep: Sep) {
        self.end_word();
        self.out.push(Tok::CommandEnd(sep));
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
                '(' if self.cur.is_none() && self.chars.peek() == Some(&'(') => {
                    self.arithmetic_command();
                }
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
                ';' => self.command_end(Sep::Seq),
                '|' => self.command_end(Sep::Branch),
                '&' => self.ampersand(),
                '>' => self.greater(),
                '<' => self.less(),
                '{' | '}' if self.cur.is_none() && self.blank_follows() => {
                    self.command_end(Sep::Group);
                }
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

    /// The chars ahead are `$name"` or `${name}"` and then the word ends.
    fn lone_variable_ahead(&self) -> bool {
        let mut ahead = self.chars.clone();
        if ahead.next() != Some('$') {
            return false;
        }
        if ahead.next_if_eq(&'{').is_some() {
            let name: String = ahead.by_ref().take_while(|c| *c != '}').collect();
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return false;
            }
        } else {
            let mut named = false;
            while ahead
                .next_if(|c| c.is_ascii_alphanumeric() || *c == '_')
                .is_some()
            {
                named = true;
            }
            if !named {
                return false;
            }
        }
        ahead.next_if_eq(&'"').is_some()
            && ahead
                .next()
                .is_none_or(|c| matches!(c, ' ' | '\t' | '\n' | ';'))
    }

    fn double_quoted(&mut self) {
        let lone = self.cur.is_none() && self.lone_variable_ahead();
        self.buf().quoted = true;
        self.buf().quoted_var = lone;
        self.buf().in_double_quotes = true;
        self.double_quoted_body();
        self.buf().in_double_quotes = false;
    }

    fn double_quoted_body(&mut self) {
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
        if let Some(c) = self.chars.next().filter(|c| *c != '\n') {
            self.buf().quoted = true;
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
                self.arithmetic_next = self.chars.peek() == Some(&'(');
                self.assigns |= self.arithmetic_next;
                self.expansion(Close::Paren);
            }
            Some('{') => {
                self.chars.next();
                let name = self.braced_body();
                let buf = self.buf();
                buf.splits |= name.is_none();
                buf.mark_expansion(name);
            }
            Some(c) if c.is_ascii_alphanumeric() || c == '_' => {
                let mut name = String::new();
                while let Some(c) = self
                    .chars
                    .next_if(|c| c.is_ascii_alphanumeric() || *c == '_')
                {
                    name.push(c);
                }
                self.buf()
                    .mark_expansion(Some(name).filter(|_| !c.is_ascii_digit()));
            }
            Some(c) if "@*#?!$-".contains(c) => {
                self.chars.next();
                let buf = self.buf();
                buf.mark_expansion(None);
                buf.splits |= c == '@';
            }
            Some('[') => {
                self.assigns = true;
                self.push('$');
            }
            _ => self.push('$'),
        }
    }

    /// Consume a `${...}` body up to its matching `}`, so a `;`, `&`, `#` or quote inside it
    /// neither splits the command nor hides the words after it. Nested braces, quotes and
    /// substitutions are tracked. A body with no end swallows the rest of the command: the
    /// shell rejects it, and the word it sits in is already opaque. Returns the parameter's
    /// name when the body is a plain one (see `plain_name`); any other body lists words even in
    /// double quotes. The body keeps its quotes and its closing `}`; an unterminated body
    /// lists words.
    fn braced_body(&mut self) -> Option<String> {
        let mut depth = 1usize;
        let mut body = String::new();
        while let Some(c) = self.chars.next() {
            body.push(c);
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return plain_name(&body);
                    }
                }
                '\\' => {
                    self.chars.next();
                }
                '\'' => {
                    for c in self.chars.by_ref() {
                        body.push(c);
                        if c == '\'' {
                            break;
                        }
                    }
                }
                '"' => self.quoted_in_braces(&mut body),
                '$' if self.chars.next_if_eq(&'(').is_some() => self.expansion(Close::Paren),
                '`' => self.expansion(Close::Backtick),
                '=' => self.assigns = true,
                _ => {}
            }
        }
        None
    }

    /// A double-quoted string inside a `${...}` body, up to its closing quote.
    fn quoted_in_braces(&mut self, body: &mut String) {
        while let Some(c) = self.chars.next() {
            body.push(c);
            match c {
                '"' => return,
                '\\' => {
                    self.chars.next();
                }
                '$' if self.chars.next_if_eq(&'(').is_some() => self.expansion(Close::Paren),
                '`' => self.expansion(Close::Backtick),
                _ => {}
            }
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
        let outer_arithmetic = std::mem::replace(
            &mut self.in_arithmetic,
            std::mem::take(&mut self.arithmetic_next),
        );
        self.run(close);
        self.in_arithmetic = outer_arithmetic;
        self.end_word();
        let inner = std::mem::replace(&mut self.out, outer_out);
        self.substitutions.push(inner);
        self.cur = outer_word;
        self.next_word_is_delimiter = outer_delimiter;
    }

    fn arithmetic_command(&mut self) {
        self.arithmetic_next = true;
        self.assigns = true;
        self.substitution(Close::Paren);
        self.out.push(Tok::CommandEnd(Sep::Group));
    }

    fn expansion(&mut self, close: Close) {
        self.substitution(close);
        self.buf().mark_expansion(None);
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
        } else if self.chars.next_if_eq(&'&').is_some() {
            self.command_end(Sep::And);
        } else {
            self.command_end(Sep::Branch);
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
        } else if self.chars.next_if_eq(&'<').is_some() {
            if self.chars.next_if_eq(&'<').is_some() {
                self.herestring();
            } else if self.in_arithmetic || self.shift_operand_follows() {
                self.buf().text.push_str("<<");
            } else {
                self.heredoc_operator();
            }
        } else if self.chars.next_if_eq(&'&').is_some() {
            self.duplicated_descriptor();
        } else {
            self.out.push(Tok::RedirectIn);
        }
    }

    /// `<<` before a number is a shift, as in `$((1<<2))`, not a heredoc whose delimiter is a digit.
    fn shift_operand_follows(&self) -> bool {
        self.chars
            .clone()
            .find(|c| !matches!(c, ' ' | '\t'))
            .is_some_and(|c| c.is_ascii_digit())
    }

    /// `<<<`: the next word is input text.
    fn herestring(&mut self) {
        self.out.push(Tok::Herestring);
    }

    /// `<<` or `<<-`: the next word is the heredoc delimiter, not a command word.
    fn heredoc_operator(&mut self) {
        self.chars.next_if_eq(&'-');
        self.out.push(Tok::Heredoc);
        self.next_word_is_delimiter = true;
    }

    fn newline(&mut self) {
        self.command_end(Sep::Seq);
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
