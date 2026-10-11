# Domain checklists

Prompts to raise during the interview and to check the draft against. Pull only the ones the change touches. A list of answered-but-irrelevant questions is exactly the bureaucracy this format exists to avoid.

Under `gates: auto`, with no one to ask, answer each prompt that applies from the requirements and the survey, and turn the rest into Open questions rows.

## Any change that crosses a boundary

Applies whenever a device, a gateway, a backend, a CLI, or a web app has to agree with another one of them.

- What does a new peer do when it meets an old one, and the reverse? Name the version on each side.
- Can the two sides deploy independently, and in which order? If not, say what coordinates them.
- Is the change additive to the wire format? If a field changed meaning rather than being added, that is a break, however it looks in the schema file.
- What happens on duplicate delivery, out-of-order delivery, and replay?
- What is the identifier that ties the two sides together, and is it stable?

## Firmware and embedded

- Flash and RAM cost of the change, measured, not guessed. Which partition, and how much headroom is left after.
- Power cost per wake cycle. Does anything here extend the awake window or add a wake?
- Cellular data per device per day, and the monthly cost at the target device count. Data is usually the expensive part.
- Does it survive a reset at every step? Walk the sequence and put a power cut at each line.
- Settings and flash writes: how many, how often, and what is the endurance budget.
- Does it hold across an OTA in both directions, including a downgrade?
- What happens to a device that is already in a bad state when this ships? There are always some.
- How does anyone debug this on a device in the field, with no console and no debug probe?
- Timers and clocks: what breaks if the clock jumps backwards, or was never set?
- Watchdog: can any new path block long enough to trip it?

## Protocol and wire format (LwM2M, CoAP, Protobuf)

- Object and resource IDs, explicitly, including whether a new one falls outside the ranges the OMA registry reserves.
- Message size against the MTU and the DTLS overhead. Does anything now need block-wise transfer?
- Behaviour under Queue Mode specifically. Does this assume a device is reachable when it is asleep?
- Retransmission and timeout constants, and where they come from.
- Protobuf field numbers: added, never reused, never renumbered. Is the field optional in a way old readers tolerate?
- Does the change need both the device and the gateway to update in lockstep? If yes, that is the riskiest sentence in the doc.

## Gateway

- Registration, registration update, and deregistration: what does the change do on each?
- Queue Mode and the awake window. What is queued, where is it durable, and what drains it?
- Ordering guarantees you are relying on, and whether the message bus actually gives them.
- Idempotency: which operation gets retried, and what key makes a retry safe?
- Backpressure. What happens when the downstream topic or database is slow, and what gets dropped first?
- Per-device state held in memory: what happens on a restart or a scale event?
- Observability: what log line or metric tells an operator this is working, and which one tells them it is not?

## Backend and data

- Schema migration: is it online, and is it reversible? A migration with no down step needs saying.
- Does the old code run against the new schema during the deploy window?
- Backfill: how much data, how long, and does it run while traffic is served?
- Read and write amplification as the device count grows. Run the numbers at 10x the current count.
- Multi-tenant isolation. Can one customer's data reach another's through this path?
- Retention and personal data: see Privacy below.

## Privacy

- Which fields does this path carry, store, or log? List them. A location tied to a person is personal data.
- Is any identifier personal by association, through a table that links it to a customer?
- Retention: what is the TTL on anything new, and does an existing rule already cover it?
- Deletion: when a customer asks to be deleted, does the existing path reach this data? By which key?
- Does data cross a region or reach a new processor, including a third-party service?
- Does anything new land in logs, traces, or an analytics export? That is the usual accidental leak.

## Security

- What is the new entry point, and who can reach it?
- Authentication and authorization for that specific path, not the system's model in general.
- Credentials and secrets: created, rotated, or expiring. What is the lifetime, and what happens when it lapses? Signed URLs and PSKs both count.
- Permissions: which identity gains which permission, and is it the narrowest one that works?
- What does a caller with a stolen device credential gain that they did not have before?
- Input that crosses a trust boundary: what validates it, and what happens on malformed input? Include payloads the device sends up.
- Encryption in transit and at rest for anything new, and where the key lives.
- Does this add a way to make the system do expensive work on an unauthenticated request?
- Threat model for the change: who would abuse it, what they would gain, what stops them.

## Infrastructure and operations

- Infrastructure-as-code change or manual step? A manual step needs an owner and a runbook line.
- Cost delta per month, with the assumption it rests on.
- Permissions and secrets: see Security above.
- Alerting: does this add a new way to fail silently?
