//! The event stream a headless `claude` session prints, one JSON event per line.

use serde_json::Value;

/// The longest a tool call's input or a tool result is shown.
const SHORT: usize = 200;

/// The text of the stream's final `result` event. `None` when the stream has none: the session
/// died before it finished.
pub fn result_find(raw: &str) -> Option<String> {
    raw.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .rfind(|event| event["type"] == "result")
        .and_then(|event| event["result"].as_str().map(String::from))
}

/// The stream as plain text, in order: what the agent said, one line per tool call and the start
/// of each tool result. A line that is not an event, such as the cut-off end of a killed session,
/// is skipped.
pub fn render(raw: &str) -> String {
    let mut out = String::new();
    for event in raw
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
    {
        let Some(blocks) = event["message"]["content"].as_array() else {
            continue;
        };
        for block in blocks {
            match block["type"].as_str() {
                Some("text") => {
                    let text = block["text"].as_str().unwrap_or_default().trim();
                    if !text.is_empty() {
                        out.push_str(text);
                        out.push('\n');
                    }
                }
                Some("tool_use") => {
                    let name = block["name"].as_str().unwrap_or("?");
                    out.push_str(&format!("> {name}: {}\n", input_short(&block["input"])));
                }
                Some("tool_result") => {
                    out.push_str(&format!("  = {}\n", short(&result_text(&block["content"]))));
                }
                _ => {}
            }
        }
    }
    out
}

/// What a tool call is about: the field that names it when there is one, else the whole input.
fn input_short(input: &Value) -> String {
    [
        "command",
        "file_path",
        "pattern",
        "path",
        "url",
        "description",
    ]
    .iter()
    .find_map(|key| input[key].as_str())
    .map(short)
    .unwrap_or_else(|| short(&input.to_string()))
}

/// A tool result is a string, or a list of blocks of which the text ones count.
fn result_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

/// `text` on one line, cut to [`SHORT`] characters.
fn short(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(SHORT) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = r#"{"type":"system","subtype":"init"}
{"type":"assistant","message":{"content":[{"type":"text","text":"Let me look."},{"type":"tool_use","name":"Bash","input":{"command":"ls   -la\nsrc"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"a","content":"main.rs\nlib.rs"}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Task","input":{"n":1}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"b","content":[{"type":"text","text":"one"},{"type":"text","text":"two"}]}]}}
{"type":"assistant","message":{"content":[{"type":"text","text":"Done."}]}}
{"type":"result","subtype":"success","result":"Done."}
"#;

    #[test]
    fn the_result_is_the_text_of_the_last_result_event() {
        let raw = format!("{SESSION}{}\n", r#"{"type":"result","result":"Later."}"#);
        assert_eq!(result_find(&raw).as_deref(), Some("Later."));
        assert_eq!(result_find(SESSION).as_deref(), Some("Done."));
    }

    #[test]
    fn a_stream_with_no_result_has_none() {
        assert_eq!(result_find("plain text\n"), None);
        assert_eq!(result_find(r#"{"type":"result"}"#), None);
    }

    #[test]
    fn a_session_renders_its_text_calls_and_results_in_order() {
        assert_eq!(
            render(SESSION),
            "Let me look.\n> Bash: ls -la src\n  = main.rs lib.rs\n> Task: {\"n\":1}\n  = one two\nDone.\n"
        );
    }

    #[test]
    fn a_long_call_and_a_long_result_are_cut() {
        let long = "x".repeat(300);
        let raw = format!(
            r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","content":"{long}"}}]}}}}"#
        );
        assert_eq!(render(&raw), format!("  = {}…\n", "x".repeat(SHORT)));
    }

    #[test]
    fn thinking_and_blank_text_leave_no_line() {
        let raw = r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hm"},{"type":"text","text":"  "}]}}"#;
        assert_eq!(render(raw), "");
    }

    #[test]
    fn a_cut_off_last_line_is_skipped() {
        let raw = format!("{SESSION}{{\"type\":\"assis");
        assert_eq!(render(&raw), render(SESSION));
    }
}
