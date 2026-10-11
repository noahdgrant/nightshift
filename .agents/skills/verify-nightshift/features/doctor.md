# Doctor

`ns doctor` reports the config path and whether it parses, the configured roles, which harnesses are on PATH, each harness skill directory and how many `ns-*` skills it holds, and the forge accounts. It names each problem and never prints a token.

## Sub-features

- `doctor-ok` reports `ok: true` with no problems on a complete config.
- `doctor-problems` lists a missing `[roles.default]` under `problems`.
- `doctor-harnesses` shows each harness binary's path, or `null`.
- `doctor-skill-dirs` counts `ns-*` skills per harness directory.

## How to get to it (user POV)

- `ns doctor`
- `NS_CONFIG=<file> ns doctor`

## Driving it with ns

Preconditions:

- Baseline, in `$V/repo`.

- **Healthy.** Run `cap doctor-ok "$NS" doctor`. `ok` is `true`, `problems` is `[]`, `harnesses.claude.path` is `$V/bin/claude`, `harnesses.codex.on_path` is `false`.
- **Problem.** Run `: > $V/c-empty.toml` and `cap doctor-problems env NS_CONFIG=$V/c-empty.toml "$NS" doctor`. `problems` contains `no [roles.default]`.
- **Skill dirs.** Read `skill_dirs` from `$OUT/doctor-ok.out`, or run `cap doctor-skill-dirs "$NS" doctor` after [install](./install.md). Each path is under `$V/home`; `ns_skills` is `0` before [install](./install.md) and the installed count after.

## Gotchas

- `ok: false` still exits `0`. Assert on `ok` and `problems`, not the exit code.
- `forge` fields are covered in [forge-config.md](./forge-config.md).
