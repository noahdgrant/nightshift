feat(cli): tally reads its step from the config file

Add `step::read(root)`, which returns the `step` in `.tally/config.toml` under `root`. A missing file or a missing `step` means a step of 1. So does a file that cannot be read or is not TOML, and a `step` that is not a non-negative integer. Nothing calls it yet.
