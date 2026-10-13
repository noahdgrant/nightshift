# Doctor

`ns doctor` reports the config path and whether it parses, the configured roles, which harnesses are on PATH, each harness skill directory and how many `ns-*` skills it holds, and the forge accounts. It names each problem and never prints a token.

## Sub-features

- `doctor-ok` reports `ok: true` with no problems on a complete config, and each harness binary's path or `null`, and exits 0.
- `doctor-problems` lists a missing `[roles.default]` under `problems`, and exits 1 with the full report still on stdout.
- `doctor-skill-dirs` counts `ns-*` skills per harness directory.

## How to get to it (user POV)

- `ns doctor`
- `NS_CONFIG=<file> ns doctor`

## Driving it with ns

Preconditions:

- Baseline. Run every doctor recipe from `$V/repo`, so never with `--src`.

- **Healthy.** It exits `0`; `ok` is `true`, `problems` is `[]`, `harnesses.claude.path` is `$V/bin/claude`, `harnesses.codex.on_path` is `false`.

  ```bash
  "$V/sandbox" cap doctor-ok ns doctor
  ```

- **Problem.** It exits `1`; `ok` is `false` and `problems` contains `no [roles.default]`.

  ```bash
  "$V/sandbox" cp /dev/null "$V/c-empty.toml"
  "$V/sandbox" cap doctor-problems env NS_CONFIG="$V/c-empty.toml" ns doctor
  ```

- **Skill dirs.** Each path in `skill_dirs` is under `$V/home`. `claude`'s `ns_skills` is the number of `skills/ns-*` directories, because Launch installed them; the others are `0` in a fresh sandbox, and after [install](./install.md) `agents` and `codex` (which share `$V/home/.agents/skills`) count the links it made.

  ```bash
  "$V/sandbox" cap doctor-skill-dirs ns doctor
  ```

## Gotchas

- Never run `ns doctor` with `--src`. From `$NS_SRC`, `ns doctor` reads this repo's real `origin` and tries `ssh -T` on its real forge host (#536). The sandbox's `ssh` fails the probe, but `forge.github.repo` and `forge.github.ssh` then describe the real repo, not the sandbox's.
- `ns doctor` exits `1` exactly when `ok` is `false`. Warnings never change the exit code.
- `forge` fields are covered in [forge-config.md](./forge-config.md).
