# Verify cases

`ns eval verify` runs the verify skill on each case here and counts how often it reaches the expected verdict. Each run is a real harness call, so it uses subscription time.

A case is a directory `evals/verify/<case>/`:

- `repo/`: a small repository with the change already applied. Each run copies it into a new temporary directory, makes it a repository with one commit, and deletes the directory afterwards.
- `unit.md`: the unit's title on the first line, then its description.
- `build.md`: the build summary the verifier is given.
- `expect`: `PASS`, `FAIL` or `INCONCLUSIVE`.

A case uses only commands that are deterministic and need no network: a script in the fixture, or `cargo test` on a tiny crate with its lockfile and no dependencies. The case is not run in a container and its network is not limited, so keep it offline. Cases are written by hand; there is no `ns eval add verify`.
