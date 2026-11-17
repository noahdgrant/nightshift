use anyhow::{bail, Result};
use clap::Args as ClapArgs;

use serde_json::json;

use crate::harness::Message;
use crate::state::State;
use crate::transcripts::{self, Transcripts};
use crate::{harness, vcs};

#[derive(ClapArgs)]
pub struct Args {
    /// The unit whose sessions to show.
    unit: u64,
    /// Show only the sessions of this state.
    #[arg(long)]
    state: Option<String>,
    /// Show only the most recent session.
    #[arg(long)]
    last: bool,
    /// Print each message as one JSON line, with the session's state and number.
    #[arg(long)]
    json: bool,
}

/// Prints what each harness session of the unit said and ran, oldest first. Changes nothing.
pub fn run(args: Args) -> Result<()> {
    let state = match &args.state {
        None => None,
        Some(name) => match State::parse(name) {
            Some(state) => Some(state),
            None => bail!("there is no state named {name}"),
        },
    };
    let vcs = vcs::get();
    let root = vcs.root_find(&std::env::current_dir()?)?;
    let kept = Transcripts::open(vcs.as_ref(), &root)?;
    let sessions = transcripts::select(kept.sessions(&args.unit.to_string())?, state, args.last);
    let harness = harness::get();
    for session in sessions {
        let raw = std::fs::read(&session.path)?;
        if args.json {
            for message in harness.transcript_messages(&String::from_utf8_lossy(&raw)) {
                let mut line = json!({ "state": session.state.to_string(), "n": session.n });
                match message {
                    Message::Text(text) => {
                        line["kind"] = "text".into();
                        line["text"] = text.into();
                    }
                    Message::ToolCall { tool, input } => {
                        line["kind"] = "tool_call".into();
                        line["tool"] = tool.into();
                        line["input"] = input;
                    }
                    Message::ToolResult(text) => {
                        line["kind"] = "tool_result".into();
                        line["text"] = text.into();
                    }
                }
                println!("{line}");
            }
            continue;
        }
        println!(
            "== {} {} ({}) ==\n{}",
            session.state,
            session.n,
            session.model.as_deref().unwrap_or("harness default model"),
            harness.transcript_render(&String::from_utf8_lossy(&raw))
        );
    }
    Ok(())
}
