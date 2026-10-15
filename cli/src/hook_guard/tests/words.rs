//! How the guard splits a command into words.

use crate::hook_guard::words::{segments, Segment, Word};

fn w(text: &str) -> Word {
    Word {
        text: text.into(),
        quoted: false,
    }
}

fn q(text: &str) -> Word {
    Word {
        text: text.into(),
        quoted: true,
    }
}

fn seg(words: Vec<Word>, redirects: Vec<Word>) -> Segment {
    Segment { words, redirects }
}

#[test]
fn operators_end_a_command_and_quotes_join_a_word() {
    assert_eq!(
        segments("a 'b c' \"d\\\"e\"f; g && h | i\nj"),
        [
            seg(vec![w("a"), q("b c"), q("d\"ef")], vec![]),
            seg(vec![w("g")], vec![]),
            seg(vec![w("h")], vec![]),
            seg(vec![w("i")], vec![]),
            seg(vec![w("j")], vec![]),
        ]
    );
}

#[test]
fn redirects_are_set_apart_and_inputs_dropped() {
    assert_eq!(
        segments("a 2>err >>out < in 2>&1 &>both"),
        [seg(
            vec![w("a")],
            vec![w("err"), w("out"), w("1"), w("both")]
        )]
    );
}

#[test]
fn comments_heredoc_bodies_and_line_continuations_are_skipped() {
    assert_eq!(
        segments("a # b c\nd <<'E' x\nbody\nE\ne \\\n f a#b"),
        [
            seg(vec![w("a")], vec![]),
            seg(vec![w("d"), w("x")], vec![]),
            seg(vec![w("e"), w("f"), w("a#b")], vec![]),
        ]
    );
    assert_eq!(
        segments("a <<-E\n\tbody\n\tE\nb <<< here"),
        [seg(vec![w("a")], vec![]), seg(vec![w("b")], vec![])]
    );
}

#[test]
fn an_unterminated_heredoc_or_quote_ends_the_command() {
    assert_eq!(segments("a <<E\nbody"), [seg(vec![w("a")], vec![])]);
    assert_eq!(segments("a 'b c"), [seg(vec![w("a"), q("b c")], vec![])]);
}
