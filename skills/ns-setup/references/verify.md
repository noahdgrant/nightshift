# Verify

How to prove a change works on the real surface, beyond the test suite. `ns-verify` reads this file.

<!-- Keep one of the two variants below. -->

## Set up

- **Verify skill**: `verify-[project]`. Load it to drive the product and collect evidence.
- **Control CLI**: `[command]`. Start with `[command] doctor`, then `[command] --help`.
- **CPU share**: scripts that start parallel work read `NS_CPUS` when it is set: [e.g. `-j "${NS_CPUS:-$(nproc)}"` in `scripts/test.sh`, or `none` when nothing runs in parallel].

## Not set up yet

Not set up yet: run ns-setup-verify.
