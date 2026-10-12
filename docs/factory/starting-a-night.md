# Starting a night

Run `ns watch` from the systemd user timer in `contrib/systemd/`, so it gets a service of its own. A terminal or a tmux session works too, but then the watcher shares its fate with whatever else runs there. On 2026-10-10 the kernel OOM-killed one test binary, systemd stopped the tmux scope it ran in, and that took `ns watch` and the night down with it (#208).

The phases run with permissions bypassed inside worktrees. Run on a machine where that's acceptable.

## Before the first night

1. Log in to claude with the subscription account (`claude /login`). Without the login `ns run` refuses to start, with exit 2 (see ["Billing"](../FACTORY.md#billing)).
2. Install `ns` and the skills from a nightshift checkout: `cargo install --path cli`, then `ns install`.
3. Put the account PRs should come from in the user config, `~/.config/nightshift/config.toml`, and run `ns doctor` in the checkout. Its `forge` entry should show `token_resolved: true`, that account, `repo_readable: true` and, for an SSH `origin`, the same account under `ssh`.

   ```toml
   [forge.github]
   token_command = "gh auth token --user <account>"
   ```

   With repos under more than one account, add a table per owner or per repo. The most specific one wins: the repo's, then the owner's, then `[forge.github]`. The owner and repo come from `origin`'s URL, SSH host aliases included. GitLab takes `[forge.gitlab.owners."<group>"]` and `[forge.gitlab.repos."<group>/<project>"]`, with nested groups on their full path.

   ```toml
   [forge.github.owners."example-org"]
   token_command = "gh auth token --user work-account"

   [forge.github.repos."example-org/backend"]
   token_command = "gh auth token --user other-account"
   ```

   `ns run` and `ns watch` check at start that the chosen token can read the repo, and exit 2 naming the account and the table when it can't. A `GH_TOKEN` set in the environment still wins over every table, with a warning. `NS_CONFIG`, which points `ns` at another config file, remains an escape hatch, but it has to be set in every shell and in the systemd unit, so prefer the tables. See `cli/README.md` (`[forge]`).

   With an SSH `origin`, give pushes a keepalive. A pre-push hook that runs all of CI leaves the SSH connection idle for minutes, and a dropped connection fails the push with exit 141 after the checks passed. `ns run` gives its phases `GIT_SSH_COMMAND="ssh -o ServerAliveInterval=30 -o ServerAliveCountMax=40"` unless `GIT_SSH_COMMAND`, `GIT_SSH` or `core.sshCommand` is already set. For your own pushes, set it in the checkout, or `ServerAliveInterval` in `~/.ssh/config`. `ns doctor` warns when nothing sets one (`ssh_keepalive`).

   ```bash
   git config core.sshCommand 'ssh -o ServerAliveInterval=30 -o ServerAliveCountMax=40'
   ```

4. Keep the watched checkout on the default branch with no changes to tracked files. `ns watch` reads the gate command from it at start. When your skills are symlinks into it, `ns watch` also fast-forwards it before each unit (on a night with codex phases, once the running units end) so codex phases and your own sessions see current skills (claude phases run each unit's pinned snapshot); on another branch or with local changes it only warns, and the night runs on the old skills. Skills linked from a separate nightshift checkout are never updated by `ns watch`, so pull that checkout yourself.
5. Run `ns watch --dry-run` in the checkout to see the queue it would work.

## Install the timer

```bash
mkdir -p ~/.config/systemd/user
cp contrib/systemd/ns-watch.service contrib/systemd/ns-watch.timer ~/.config/systemd/user/
systemctl --user edit ns-watch.service
```

The unit runs in `~/src/myrepo`. Point it at your checkout in the drop-in that `systemctl --user edit` opens. To change the end of the night, clear `ExecStart` and `ExecCondition` with an empty line each and set them again. Both are lists, so a drop-in that only adds a line leaves the packaged one running too:

```ini
[Service]
WorkingDirectory=%h/src/myrepo
ExecCondition=
ExecCondition=/bin/sh -c 'h=$$(date +%%H); [ "$$h" -ge 21 ] || [ "$$h" -lt 7 ]'
ExecStart=
ExecStart=%h/.cargo/bin/ns watch --until 08:00
```

Then enable the timer, and let your user manager run while you are logged out:

```bash
systemctl --user daemon-reload
systemctl --user enable --now ns-watch.timer
loginctl enable-linger "$USER"
```

The timer starts the service at 22:00 local time. To start a night earlier, after 21:00, run `systemctl --user start ns-watch.service`. Follow it with `journalctl --user -u ns-watch -f`, which shows `ns watch`'s stderr and, at the end, its JSON summary. Watch the first night start this way: `ns doctor` in a terminal checks the terminal's environment, not the service's.

`--until` is local time, from `TZ` or the system zone, and means the next 06:30. A night that starts at 08:00 would run until 06:30 the next day, so the units guard against late starts:

- The timer has no `Persistent=`, so a 22:00 missed while the machine was off is skipped, not run at boot.
- A 22:00 missed while the machine was suspended does run on resume; systemd catches up calendar timers after a suspend. The service's `ExecCondition=` skips any start between 06:00 and 21:00, so that one is skipped too. It skips a daytime `systemctl --user start` as well: for a daytime run, start `ns watch` in a terminal. If you change `--until`, change the hours in `ExecCondition=` to match, as in the drop-in above.

Only one `ns watch` runs per repo. A second one, from a terminal or another unit pointed at the same checkout, exits 5 with `another ns watch (pid <pid>) is running`. The timer never starts the service twice, because systemd doesn't start a unit that is already running.

The service gets the token through the config's `token_command`, as in a terminal. If that command can't run under systemd, for example because gh keeps the token in a desktop keyring that stays locked while you are logged out, put the token in `~/.config/nightshift/ns-watch.env` instead, outside any repo, and `chmod 600` it:

```bash
GH_TOKEN=<token>
```

The unit reads that file when it exists, and a `GH_TOKEN` in the environment wins over the config.

The service has no SSH agent either, so `git fetch` and `git push` over an SSH remote whose key lives in the agent fail on every unit. Use an HTTPS `origin` and run `gh auth setup-git` once, so git authenticates with `GH_TOKEN`, or use a key that needs no agent.

What the other settings in `ns-watch.service` do:

- `OOMPolicy=continue` keeps the service running when the kernel OOM-kills one of its processes. The default for a service is to stop it, which is the 2026-10-10 failure again. The killed phase or gate fails its attempt, and the night goes on.
- `KillMode=mixed` sends `systemctl --user stop`'s SIGTERM to `ns watch` alone. `ns watch` then starts nothing new, sends SIGTERM to the running phase's process group and SIGKILL 5 s later, moves the phase's partial artifact aside, returns the issue to the ready label and exits 143 (see ["Units a night didn't finish"](watch.md#units-a-night-didnt-finish)). The default, `KillMode=control-group`, would send SIGTERM to every process in the service at the same moment, the phase and any `git` or `gh` call included, so the shutdown would no longer go in that order. If `ns watch` hasn't exited after `TimeoutStopSec=2min`, systemd sends SIGKILL to everything in the service. Two minutes covers the 5 s grace and the `gh` calls that relabel the issue.
- `SuccessExitStatus=143` records a stop as a clean exit, not a failure.
- `MemoryHigh=` and `MemoryMax=` are commented out. Set them to cap the memory of the whole night, `ns watch` included. With `MemoryMax=` the kernel OOM-kills inside the service before the machine runs short, and `OOMPolicy=continue` keeps the night going. They need cgroup v2. A per-phase memory cap in `nightshift.toml` is planned in #208. `OOMPolicy=` covers only the kernel's OOM killer: where systemd-oomd runs, it can kill the whole service under memory pressure, and a `MemoryHigh=` that throttles the night before the machine runs short makes that less likely.
- `PATH` lists `~/.cargo/bin` and `~/.local/bin`, since a user service doesn't read your shell profile. Add the directories where `claude` and `gh` live if they are elsewhere.

## When the watched repo is nightshift

On the nightshift source, `ns watch` updates its own binary, so units that merge changes to `cli/` reach the rest of the night. The repo counts as the nightshift source when its `cli/Cargo.toml` names the package `nightshift`. `ns --version` shows the commit the binary was built from (`ns 0.1.0 (<sha>)`); `build.rs` records it.

Each time `ns watch` fills its slots, it fetches and looks for commits on `origin/<default>` that touch `cli/` and that the build commit lacks. When it finds some, it enters DRAINING: it prints `ns watch: origin/<default> has cli/ changes since this ns was built (<from> -> <to>); draining: no new unit until the running ones end`, claims nothing and runs no triage pass. Running units finish undisturbed, and paused ones still resume. Idle slots are fine. With `--parallel 1` the update comes between units; at the start of a night with an old binary, it comes before the first unit.

When no unit runs or is paused, it updates:

1. Extract `cli/` at the new commit (`git archive`) into `<git-common-dir>/ns/self-update/src`, and build it with `cargo build --release --locked`, its target directory `<git-common-dir>/ns/self-update/target`, then copy the binary to `<git-common-dir>/ns/self-update/ns-<sha>`. The installed `ns` is never touched.
2. Check that `ns-<sha> --version` names the new commit and that `ns-<sha> watch --dry-run` exits 0, run with this `ns watch`'s own arguments and a copy of the night's state, so an argument the new binary no longer takes fails the check here, not after the exec. A dry run given `--resume-night` only reads the arguments and the state: it lists no queue and calls neither `gh` nor the network, so a flaky connection cannot fail the check.
3. Write the night's state to `<git-common-dir>/ns/self-update/night-<pid>.json`, print `ns watch: self-update <from> -> <to>; handing off to <path>`, log a `self_update` event (`from`, `to`) in the run log, and `exec` the new binary with the same arguments plus a hidden `--resume-night <file>`. The process keeps its pid. The new binary reads the state, removes the file, and carries on: the deadline, the spend, the units run so far and the issues finished, the triage pass's runs and the issues it tried, the harness-failure count, the units claimed toward `--max-units`. Its summary covers the whole night.

If the build or the check fails, the night goes on with the current binary. `ns watch` prints `ns watch: warning: self-update to <to> failed; keeping this ns: <error>`, logs a `self_update_failed` event (`from`, `to`, `error`), leaves DRAINING and never tries that commit again tonight. A later commit is tried when it lands.

A stop (`--until`, the budget, a signal) ends a drain without an update. For any other repo `ns watch` never drains or rebuilds, and `--no-self-update` turns it off on nightshift too. An `ns` built outside a git checkout has no build commit and prints `ns watch: self-update off: this ns has no build commit`.

The new binary lives under the git dir, so the next night starts with the installed one, and updates at once if `cli/` moved. A pull and rebuild before each night is no longer needed; to start nights on a current binary anyway, keep the installed one current with `cargo install --path cli` after merging.

## Stopping a night

`systemctl --user stop ns-watch.service`, or Ctrl-C when `ns watch` runs in a terminal, ends the night early. Every unit in progress goes back to the ready label and resumes on the next night. A second Ctrl-C ends `ns watch` at once, without returning the issues; each unit's `ns run` then ends its phase too. Under systemd a second `stop` sends nothing more; `systemctl --user kill --kill-whom=main ns-watch.service` sends the second SIGTERM (without `--kill-whom=main` it signals the phase too). That, or the SIGKILL at `TimeoutStopSec`, leaves the issue in progress, and the next `ns watch` returns it at start.

## Supervising a night

The `ns-supervise` skill runs a session one level above `ns watch`: it attaches to the night or starts it, pings you when you are needed, and writes a status report to `<git-common-dir>/ns/handoffs/nightshift-<night>.md`. It reads `[supervise]` through `ns factory validate`. Each ping goes to the harness's notification when it has one and to a message in the supervisor's thread. When `notify` is set, the supervisor also runs it with `sh -c`, the one-line ping on stdin. `silence_minutes` is how long the run log may stay quiet while units run before it pings.

## In the morning

Start with the summary at the end of the journal:

```bash
journalctl --user -u ns-watch --since yesterday
```

The run log, `.git/ns/runs.jsonl` in the main checkout's git dir, has one JSON line per event. Its `ts` is UTC. This lists how each run ended, in local time, triage-only runs included:

```bash
jq -r 'select(.event == "end")
  | [(.ts | fromdate | strflocaltime("%a %H:%M")), .unit, .outcome, .reason]
  | @tsv' .git/ns/runs.jsonl
```

A `phase` event has the attempt and a `transcript` path for the phase's full output. One that ran to the end also has `exit`, `timed_out` and `cost_usd`; one cut short by a stop has `interrupted` and `archived` instead. `gate` events carry the CI gate's `log`. To see one unit's phases, filter with `select(.unit == "<unit>")`.

`ns quality --since` takes a UTC instant too. For the night that started at 22:00 yesterday:

```bash
ns quality --since "$(date -u -d "@$(date -d 'yesterday 22:00' +%s)" +%FT%TZ)"
```

The summary's `cleaned` lists the units whose worktrees the night removed after a human merged their PR; a unit the merge step merged was cleaned by its own run. Each attempt is a `cleanup` event, and one that stayed carries its `reason`:

```bash
jq -r 'select(.event == "cleanup") | [.unit, .removed, .reason // ""] | @tsv' .git/ns/runs.jsonl
```

On the tracker, a merged unit's issue is closed. Under `merge.policy = "human"`, a unit that ended `done` has `done_label` (`status:in-review` by default) and an open PR for you to merge. A stuck unit, or a `done` that needs a human merge, has `stuck_label` (`status:ready-for-human` by default) and a comment with the reason and the last artifact. Every PR waiting on a human merge carries `human_merge_label`, with a comment saying why, so the merge queue is:

```bash
gh pr list --label needs-human-merge
```

## Resuming a unit

A unit's worktree, branch and `.ns/<unit>/` artifacts stay after it stops, so it resumes where the [state table](run.md) says. They go only once its change merges (see ["Cleanup"](unit-lifecycle.md#cleanup)).

- **Paused on a usage limit.** `ns watch` sleeps until the limit resets and resumes the same unit. If the reset falls at or after `--until`, it returns the issue to the ready label, and the next night picks it up. To resume it sooner, after the limit resets, run `ns run --issue <n>` in the checkout. Do that only when no `ns watch` is running: while one sleeps on the limit it holds no run lock, so nothing stops two runs working the same unit. `ns run` changes no labels, so set the issue's label yourself afterwards.
- **Interrupted by a stop.** The issue is back on the ready label. The next run runs the cut-short phase again, and what that phase had written is in `.ns/<unit>/history/<artifact>-interrupted-<n>.md`.
- **Lost to a crash, a SIGKILL or a lost terminal.** The issue keeps `status:in-progress`. The next `ns watch` returns it to the ready label at start and lists it in `requeued`.
- **Stuck.** Read the comment, fix what it names, and put the ready label back. `ns run --issue <n>` by hand also gives each phase fresh attempts, and leaves the labels to you.

To run `ns watch` in a terminal instead, keep its log somewhere you can read in the morning:

```bash
ns watch --until 06:30 >> ~/.local/share/nightshift/watch.log 2>&1
```
