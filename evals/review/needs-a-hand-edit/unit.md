feat(cli): tally reads its step from the config file

Add `step::read(root)`, which returns the `step` in `.tally/config.toml` under `root`. A missing file or a missing `step` means a step of 1. Nothing calls it yet.
