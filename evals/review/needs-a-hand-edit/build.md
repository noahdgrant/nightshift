**Changed**: adds `step::read`, which reads `step` from `.tally/config.toml`. A file that is not TOML, a `step` that is not a non-negative integer, and a file that cannot be read all mean a step of 1.

**Tests**: `step_comes_from_the_config`, `no_config_means_a_step_of_1`, `no_step_means_a_step_of_1`, `a_config_that_is_not_toml_means_a_step_of_1`, `a_step_that_is_not_an_integer_means_a_step_of_1`, `a_negative_step_means_a_step_of_1` and `a_step_of_0_is_read_as_0` in `tests/step.rs`.
