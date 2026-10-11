---
name: ns-principle-contract-first
description: "Apply when adding or changing an interface: a module's public API, a schema, a wire format, CLI flags or JSON output. Define the interface and its tests before the implementation, and version it when it must break."
disable-model-invocation: true
metadata:
  upstream: addyosmani/agent-skills@1401c8b8030e:skills/api-and-interface-design
---

# Contract First

Define the interface before the code behind it: the types, the schema, the wire format, the CLI flags and JSON output. Write the tests that pin it next. The implementation comes last and fits the contract.

**Why:** An interface grown out of its implementation leaks that implementation: its field names, its error types, its ordering. Callers then depend on all of it ([Hyrum's law](../ns-principle-hyrums-law/SKILL.md)). Writing the contract first makes each exposed behaviour a choice.

**Pattern:**
- **Write the contract down first.** Signatures and types, the schema or frame layout, each flag and output field, and the error modes. Put it where callers read it: the type definitions, the `--help` text, the schema file.
- **Test the contract, then implement.** The first red test asserts the interface as a caller sees it: the exact JSON, the exit code, the frame bytes.
- **One error shape.** Every error from one interface comes back the same way (one exception type, one JSON error object, one exit code per class of failure).
- **Prefer extension.** New optional fields and new flags leave existing callers working.
- **Version it when it must break.** Bump a version field, add a new flag or endpoint, and keep the old one until its callers move. A silent break is a defect.

**Python:** before adding `inventory extend`, write its contract: `inventory extend <reservation-id> <duration>`, prints `R0001: 2 x BOLT-M6 until <iso>`, exits 1 with `inventory: error: ...` on an unknown or expired reservation. Pin each line in a CLI test, then write `Warehouse.extend`.

**Firmware:** before writing the driver for a new sensor message, write the frame layout as a packed struct with a static size assertion, the byte order and the scaling, and a host test that encodes a known reading to its exact bytes. Add a protocol version byte, so a later layout change can be detected by the receiver.
