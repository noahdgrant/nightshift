# `ns transcripts`

## `ns transcripts list [--since <date>] [--project <path>] [--new] [--mark]`

Lists Claude Code sessions from `$CLAUDE_CONFIG_DIR/projects` (default `~/.claude/projects`) as `{"index","marked","sessions"}`, oldest first. Each session has `id`, `source`, `date` (its first event's timestamp), `project` (its working directory), `size`, `mtime` and `path`. `--since 2026-10-01` keeps sessions that started on or after that UTC date. `--project <path>` keeps sessions run in that directory (trailing slashes and symlinks are normalised; a session with no recorded working directory is matched by its Claude directory name). `--new` keeps sessions whose size or mtime differ from the index at `$XDG_DATA_HOME/nightshift/transcripts-index.json` (default `~/.local/share/nightshift/`). `--mark` records the listed sessions there as processed.

## `ns transcripts read <id> [--compact]`

Prints one session (an id or a unique prefix) as `{"id","source","date","project","path","entries"}`. Entries are user prompts, assistant text, tool calls with their JSON input, tool output, and errors, in order. Thinking and injected meta messages are left out. Text over 2000 characters ends in `…[truncated N chars]`. `--compact` cuts at 200, tool output included. Exits 1 if no session matches, 2 if the prefix matches several. Entries are verbatim and can contain secrets (tokens in tool output, pasted credentials), so review before sharing or committing them.

Each harness's store is a `Reader` in `cli/src/transcripts.rs`. Only Claude Code has one so far. `--project` takes the path itself and the reader decides what matches. The same id under two project directories is two sessions: the index keys on source and store path, and `read` exits 2 with both paths.
