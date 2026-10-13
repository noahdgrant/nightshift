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

- Baseline. Run `. "$V/env.sh"; cd "$V/repo"` before the recipes, as the `SKILL.md` Drive does.

- **Healthy.** Run `cap doctor-ok "$NS" doctor`. It exits `0`; `ok` is `true`, `problems` is `[]`, `harnesses.claude.path` is `$V/bin/claude`, `harnesses.codex.on_path` is `false`.
- **Problem.** Run `: > "$V/c-empty.toml"` and `cap doctor-problems env NS_CONFIG="$V/c-empty.toml" "$NS" doctor`. It exits `1`; `ok` is `false` and `problems` contains `no [roles.default]`.
- **Skill dirs.** Read `skill_dirs` from `$OUT/doctor-ok.out`, or run `cd "$V/repo" && cap doctor-skill-dirs "$NS" doctor` after [install](./install.md), which leaves you in `$NS_SRC`. Each path is under `$V/home`; `ns_skills` is `0` in a fresh sandbox before [install](./install.md), and the number of links install made after it (the Install recipes end with every link restored, so doctor run after them shows the full count).

## Gotchas

- Run every doctor recipe from `$V/repo`, whose `origin` is local, never from `$NS_SRC`. From `$NS_SRC`, `ns doctor` reads this repo's real `origin` and `ssh -T` probes its real forge host.
- `ns doctor` exits `1` exactly when `ok` is `false`. Warnings never change the exit code.
- `forge` fields are covered in [forge-config.md](./forge-config.md).
