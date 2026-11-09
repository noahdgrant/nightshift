# Review

What review reads hardest and leaves out. Read by `ns-review`.

## Where review looks hardest

- A tuple read as `.1` and `.2`: it should be a named struct.
- A `match` that ends in `_` where it could name each case.
- A bare `bool` argument: it should be a named type or two functions.
- Behaviour the unit did not ask for.
- A committed file that names a path or command a fresh checkout lacks.

## What to leave out

- Style that `cargo fmt` or `clippy` already owns.
