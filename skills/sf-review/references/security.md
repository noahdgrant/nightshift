# Security reviewer

Find practical, exploitable issues, not theoretical ones. Start from the **trust boundaries**: every place untrusted data enters the diff. For each, run STRIDE (spoofing, tampering, repudiation, information disclosure, denial of service, elevation of privilege) before listing findings.

For every finding, trace the input path from the boundary to the sink and show it. Critical and High findings carry an exploitation scenario.

## General

- **Input handling**: validated once at the boundary, with allowlists, length and range limits. Injection into SQL, shell (`subprocess` with `shell=True`, `os.system`), `eval`/`exec`, `pickle`/`yaml.load` on untrusted data, path traversal in file paths, template injection.
- **AuthN and AuthZ**: every protected entry point checks identity and ownership. Passwords hashed with bcrypt, scrypt or argon2. Tokens time-limited and validated.
- **Secrets**: none in code, logs, error messages, test fixtures or config committed to the repo.
- **Data protection**: sensitive fields excluded from responses and logs; encryption in transit and at rest where required.
- **Errors**: generic to the caller, no stack traces or internals.
- **Dependencies**: new or upgraded packages from trusted sources, no known CVEs, no typosquats or install-time scripts.
- **Third parties**: webhook signatures verified, server-side fetches of user-supplied URLs allowlisted (SSRF).
- **LLM features**: model output treated as untrusted (never into `eval`, SQL, shell or paths); permissions enforced in code, not the prompt; secrets kept out of context; token and recursion limits set.
- **TOCTOU** in security-critical checks.

Never recommend disabling a security control as a fix.

## Firmware

- **Memory safety**: buffer and array bounds on every copy (`memcpy`, `strcpy`, `sprintf`), length fields trusted before validation, integer overflow in size arithmetic, use-after-free, uninitialised reads, stack buffers sized by external input.
- **Untrusted input over serial, radio and USB**: every frame, packet and descriptor is attacker-controlled until validated. Check length, type and range before parsing, reject malformed frames, and bound parser state machines. Fuzzable parsers with no fuzz or property test are a finding.
- **Secrets in flash**: keys, credentials and certificates stored in plain flash or compiled into the image. Prefer the secure element, key store or read-protected region the platform offers. Secrets in debug logs over UART count too.
- **Debug ports left open**: JTAG/SWD enabled, readout protection off, debug shells or test commands compiled into production builds, verbose logging on a production console.
- **Firmware signing and rollback**: images and OTA updates verified against a signature before boot or install, anti-rollback counters or version checks in place, a failed update falls back to a known-good image.
- **Fault handling**: hard faults and watchdog resets leave the device in a safe state, not a debug or bootloader mode an attacker can use.

## Severity

| Label | Criteria | review.md |
|---|---|---|
| Critical | Exploitable remotely or over the wire, leads to compromise or data breach | Critical |
| High | Exploitable with some conditions, significant exposure | Critical |
| Medium | Limited impact or needs authenticated or physical access | Important |
| Low / Info | Defence in depth, no current risk | Suggestion |
