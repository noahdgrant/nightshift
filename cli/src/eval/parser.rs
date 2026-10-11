//! Harness transcript parsers. Each `output` format in `[eval.harnesses.<name>]` maps to one.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

/// What a transcript tells us about one run.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Transcript {
    /// The agent's final answer.
    pub result: Option<String>,
    pub cost_usd: Option<f64>,
    /// Input tokens including cache reads and writes.
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub duration_ms: Option<u64>,
    pub num_turns: Option<u64>,
    /// Skills the agent loaded, by directory name.
    pub skills_loaded: BTreeSet<String>,
}

impl Transcript {
    pub fn loaded(&self, skill: &str) -> bool {
        self.skills_loaded.contains(skill)
    }
}

pub trait OutputParser {
    fn parse(&self, stdout: &str) -> Transcript;
}

/// The parser for an `output` name; `None` for `none` or an unknown format.
pub fn for_output(name: &str) -> Option<Box<dyn OutputParser>> {
    match name {
        "claude-stream-json" => Some(Box::new(ClaudeStreamJson)),
        "codex-jsonl" => Some(Box::new(CodexJsonl)),
        _ => None,
    }
}

/// `claude -p --output-format stream-json --verbose`: one JSON event per line.
pub struct ClaudeStreamJson;

impl OutputParser for ClaudeStreamJson {
    fn parse(&self, stdout: &str) -> Transcript {
        let mut t = Transcript::default();
        for line in stdout.lines() {
            let Ok(ev) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            match ev.get("type").and_then(Value::as_str) {
                Some("assistant") => {
                    let content = ev
                        .pointer("/message/content")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    for block in content {
                        if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                            tool_use_skill(&block, &mut t.skills_loaded);
                        }
                    }
                }
                Some("result") => {
                    t.result = ev.get("result").and_then(Value::as_str).map(String::from);
                    t.cost_usd = ev.get("total_cost_usd").and_then(Value::as_f64);
                    t.duration_ms = ev.get("duration_ms").and_then(Value::as_u64);
                    t.num_turns = ev.get("num_turns").and_then(Value::as_u64);
                    if let Some(u) = ev.get("usage") {
                        let n = |k: &str| u.get(k).and_then(Value::as_u64);
                        let parts = [
                            n("input_tokens"),
                            n("cache_creation_input_tokens"),
                            n("cache_read_input_tokens"),
                        ];
                        if parts.iter().any(Option::is_some) {
                            t.input_tokens = Some(parts.iter().flatten().sum());
                        }
                        t.output_tokens = n("output_tokens");
                    }
                }
                _ => {}
            }
        }
        t
    }
}

/// A `Skill` tool call naming the skill (optionally `plugin:skill`), or a `Read` of `<skill>/SKILL.md`.
fn tool_use_skill(block: &Value, loaded: &mut BTreeSet<String>) {
    let input = block.get("input").cloned().unwrap_or(Value::Null);
    match block.get("name").and_then(Value::as_str) {
        Some("Skill") => {
            if let Some(obj) = input.as_object() {
                for v in obj.values().filter_map(Value::as_str) {
                    let name = v.rsplit(':').next().unwrap_or(v).trim();
                    if !name.is_empty() && !name.contains(char::is_whitespace) {
                        loaded.insert(name.to_string());
                    }
                }
            }
        }
        Some("Read") => {
            if let Some(dir) = input
                .get("file_path")
                .and_then(Value::as_str)
                .and_then(skill_md_dir)
            {
                loaded.insert(dir.to_string());
            }
        }
        _ => {}
    }
}

/// The directory name in a `<dir>/SKILL.md` path.
fn skill_md_dir(path: &str) -> Option<&str> {
    let mut parts = path.rsplit('/');
    if parts.next() != Some("SKILL.md") {
        return None;
    }
    parts.next().filter(|d| !d.is_empty())
}

/// `codex exec --json`: one JSON event per line. Event and item shapes are from
/// https://developers.openai.com/codex/noninteractive#make-output-machine-readable and
/// `ThreadEvent` in https://github.com/openai/codex/blob/main/codex-rs/exec/src/exec_events.rs.
/// The stream reports no cost, duration or turn count.
pub struct CodexJsonl;

impl OutputParser for CodexJsonl {
    fn parse(&self, stdout: &str) -> Transcript {
        let mut t = Transcript::default();
        for line in stdout.lines() {
            let Ok(ev) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            match ev.get("type").and_then(Value::as_str) {
                Some("item.completed") => {
                    let item = &ev["item"];
                    let text = |k: &str| item.get(k).and_then(Value::as_str);
                    match text("type") {
                        Some("agent_message") => {
                            t.result = text("text").map(String::from);
                        }
                        Some("command_execution") => {
                            command_skills(text("command").unwrap_or(""), &mut t.skills_loaded);
                        }
                        _ => {}
                    }
                }
                // `input_tokens` already counts `cached_input_tokens`.
                Some("turn.completed") => {
                    let n = |k: &str| ev.pointer(&format!("/usage/{k}")).and_then(Value::as_u64);
                    for (sum, key) in [
                        (&mut t.input_tokens, "input_tokens"),
                        (&mut t.output_tokens, "output_tokens"),
                    ] {
                        if let Some(v) = n(key) {
                            *sum.get_or_insert(0) += v;
                        }
                    }
                }
                _ => {}
            }
        }
        t
    }
}

/// Every `<skill>/SKILL.md` path in a shell command line.
fn command_skills(command: &str, loaded: &mut BTreeSet<String>) {
    let words = command.split(|c: char| c.is_whitespace() || "'\"\\;|&>".contains(c));
    for dir in words.filter_map(skill_md_dir) {
        loaded.insert(dir.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STREAM: &str = r#"{"type":"system","subtype":"init","session_id":"s"}
{"type":"assistant","message":{"content":[{"type":"text","text":"hi"},{"type":"tool_use","id":"t1","name":"Skill","input":{"skill":"ns-tdd"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"Read","input":{"file_path":"/tmp/h/.claude/skills/ns-contract/SKILL.md"}}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t3","name":"Read","input":{"file_path":"/repo/src/SKILL.md.bak"}}]}}
not json
{"type":"result","subtype":"success","result":"done","total_cost_usd":0.0123,"duration_ms":4567,"num_turns":3,"usage":{"input_tokens":10,"cache_creation_input_tokens":100,"cache_read_input_tokens":1000,"output_tokens":42}}
"#;

    #[test]
    fn parses_result_and_usage() {
        let t = ClaudeStreamJson.parse(STREAM);
        assert_eq!(t.result.as_deref(), Some("done"));
        assert_eq!(t.cost_usd, Some(0.0123));
        assert_eq!(t.input_tokens, Some(1110));
        assert_eq!(t.output_tokens, Some(42));
        assert_eq!(t.duration_ms, Some(4567));
        assert_eq!(t.num_turns, Some(3));
    }

    #[test]
    fn detects_skill_loads() {
        let t = ClaudeStreamJson.parse(STREAM);
        assert!(t.loaded("ns-tdd"));
        assert!(t.loaded("ns-contract"));
        assert!(!t.loaded("src"));
        assert_eq!(t.skills_loaded.len(), 2);

        let plugin = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Skill","input":{"command":"nightshift:ns-review"}}]}}"#;
        assert!(ClaudeStreamJson.parse(plugin).loaded("ns-review"));
    }

    #[test]
    fn empty_or_garbage_yields_nothing() {
        assert_eq!(ClaudeStreamJson.parse(""), Transcript::default());
        assert_eq!(ClaudeStreamJson.parse("{]\n"), Transcript::default());
        assert!(for_output("none").is_none());
        assert!(for_output("claude-stream-json").is_some());
    }

    const CODEX_NORMAL: &str = include_str!("../../tests/fixtures/codex-jsonl/normal.jsonl");
    const CODEX_ERROR: &str = include_str!("../../tests/fixtures/codex-jsonl/error.jsonl");
    const CODEX_SKILLS: &str = include_str!("../../tests/fixtures/codex-jsonl/skills.jsonl");

    #[test]
    fn codex_parses_last_message_and_usage() {
        assert!(for_output("codex-jsonl").is_some());
        let t = CodexJsonl.parse(CODEX_NORMAL);
        assert_eq!(
            t.result.as_deref(),
            Some("The repo has a README and a src directory.")
        );
        assert_eq!(t.input_tokens, Some(24763));
        assert_eq!(t.output_tokens, Some(122));
        assert_eq!(t.cost_usd, None);
        assert!(t.skills_loaded.is_empty());
    }

    #[test]
    fn codex_error_run_has_no_result_or_usage() {
        assert_eq!(CodexJsonl.parse(CODEX_ERROR), Transcript::default());
        assert_eq!(CodexJsonl.parse(""), Transcript::default());
    }

    #[test]
    fn codex_detects_skill_md_reads_and_sums_turns() {
        let t = CodexJsonl.parse(CODEX_SKILLS);
        assert_eq!(
            t.skills_loaded
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["ns-contract", "ns-tdd"]
        );
        assert_eq!(t.input_tokens, Some(1200));
        assert_eq!(t.output_tokens, Some(57));
        assert!(!t.loaded("ns-review"), "an agent message is not a read");
    }

    #[test]
    fn codex_splits_commands_on_shell_punctuation() {
        let mut loaded = BTreeSet::new();
        command_skills(
            r#"cat /SKILL.md /s/a/SKILL.md /s/b/SKILL.md;wc /s/c/SKILL.md|x /s/d/SKILL.md>o /s/e/SKILL.md&&x "/s/f/SKILL.md""#,
            &mut loaded,
        );
        assert_eq!(
            loaded.iter().map(String::as_str).collect::<Vec<_>>(),
            ["a", "b", "c", "d", "e", "f"]
        );
    }

    #[test]
    fn codex_usage_without_a_count_keeps_the_sum() {
        let s = r#"{"type":"turn.completed","usage":{"input_tokens":5,"output_tokens":2}}
{"type":"turn.completed","usage":{}}
"#;
        let t = CodexJsonl.parse(s);
        assert_eq!((t.input_tokens, t.output_tokens), (Some(5), Some(2)));
        let empty = r#"{"type":"turn.completed","usage":{}}"#;
        assert_eq!(CodexJsonl.parse(empty), Transcript::default());
    }
}
