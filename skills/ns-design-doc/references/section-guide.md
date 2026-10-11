# What good looks like, section by section

Each entry gives the question the section answers, the test for a good answer, and a weak/strong pair. The examples follow one change, resuming interrupted firmware downloads on a fleet of cellular devices, so the shape is recognisable. They are illustrations, not records of real decisions. For an as-built doc, `SKILL.md` lists which sections change and how.

## Title

Answers: what is changing?

Test: a reader who knows the system can guess the doc's content from the title alone.

- Weak: "Firmware Update Design Doc"
- Strong: "Resume interrupted firmware downloads instead of restarting them"

Name the change when the doc proposes one. Name the component only when the doc introduces that component, or describes it as-built, as in "Sync service". A title like "[Component] Design Doc" on a proposal gives a reader no way to tell this doc from the last three about the same component.

## Summary

Answers: what should I take away if I read nothing else?

Test: three sentences, and the third one names a cost. A summary with no cost in it is a pitch.

- Weak: "This document describes improvements to the firmware update process to make it more robust and efficient."
- Strong: "A device that drops mid-download currently restarts from byte zero, which burns the full 4 MB image in cellular data on every retry. We add a resume path using HTTP range requests against the existing signed URL. It costs one new resource on the custom object and makes the download state survive a reboot, which means a new flash region to manage."

## Problem

Answers: what is broken, for whom, and why does it stop being tolerable now?

Test: contains at least one fact you could check. A ticket number, a log line, a measured rate, a customer sentence.

- Weak: "Firmware updates sometimes fail, causing frustration for users."
- Strong: "Across the June pilot, 23 of 140 devices needed three or more attempts to take an update, and 6 never completed. Devices on weak cellular restart the whole transfer after every drop. The next feature release ships as firmware, so those 6 customers would never get it."

If you cannot write a checkable fact, the honest move is to say the problem is suspected and name the measurement that would confirm it.

## Requirements: goals and non-goals

Answers: what must be true when this is done, and what are you refusing to do?

Test: each goal is something you could pass or fail. Each non-goal has a reason attached.

Where a requirements doc exists, name it in the header by path or URL with the version this design answers, and start each goal with the ID of the requirement it answers. Cite the ID rather than quoting the requirement: a quoted copy disagrees with its source the first time the requirement changes. The IDs are what let someone changing a requirement find this doc later.

- Weak goal: "Improve reliability of the update path."
- Strong goal: "FIRMWARE-3: a device that loses connectivity at any point in a download completes the update on a later wake cycle without re-sending bytes it already has."

- Weak non-goal: "Performance optimization is out of scope."
- Strong non-goal: "We are not adding delta updates. They need a build-time patch pipeline and a second flash slot, and resume alone recovers most of the wasted data. Revisit if data cost is still the top complaint after this ships."

Non-goals are the most useful part of most design docs, because they are what reviewers try to add. Write them early and defend them.

## Constraints

Answers: what cannot move, and who says so?

Test: every constraint names its source, so a reviewer can challenge the source rather than argue with you. A limit set by a requirement, another design, or an interface cites its ID or links its section, with the ID beside any value you repeat.

- Weak: "Limited memory and power on the device."
- Strong: "The partition map leaves 4 KB of settings storage, of which 1.2 KB is free today (measured from the current build). The device wakes for around 30 seconds per cycle, bounded by MAX_TRANSMIT_WAIT (RFC 7252), so no step may assume a longer window. The object store's signed URLs cannot be valid for more than 7 days (its documentation), so a resume that spans longer needs a fresh URL."

Separate real constraints from decisions you have already made. The gateway's 60 minute URL lifetime is a setting the team chose, so it is a decision. A decision belongs in Design, where it can be argued with.

## Architecture

Answers: what are the pieces, and who talks to whom?

Test: the diagram and the component lines together explain a path through the system that the Design section then details. If Design repeats them, one of the two sections is redundant.

Include this only when the change adds a component or changes how existing ones talk. A change inside one component does not need it, and filling it in anyway produces a diagram of the system as it already is, which every reader already has.

Keep each component to a line or two: what it is responsible for, who it talks to. Depth belongs in Design.

Leave out the technology stack unless the change adds to it. The language, build tool and server library in a doc about queue draining tell the reader nothing they did not know and cost a paragraph.

## Design

Answers: how does it work?

Test: a reviewer could build roughly the right thing from it, and can point at the exact line they disagree with.

Write the main path first, in order, then the edges. Name real identifiers: object and resource numbers, topic names, message types, config keys, function names where they already exist. A design written in generic nouns cannot be reviewed, because every reader fills the nouns in differently.

Interfaces get before-and-after treatment, always:

- Weak: "The device reports its download progress to the gateway."
- Strong: "Custom Object 32768 gains Resource 3, `download_offset` (integer, read-only). The gateway reads it before writing Package URI. Old firmware does not implement Resource 3; the gateway treats a 4.04 as offset zero and falls back to a full download, so a mix of old and new firmware keeps working."

That last clause is the one reviewers care about most and the one most often missing. Say what happens when the two sides disagree about version.

When the change reshapes several components, a per-component breakdown helps: responsibilities, inputs and outputs, error handling, key data structures, where state lives. When it touches one component, that structure produces five thin subsections instead of one clear paragraph. Use it when there is something to put in every slot, not because the template offered the slots.

## Alternatives considered

Answers: why this and not the obvious other thing?

Test: a reader who arrived with a different idea finds it here, described fairly.

- Weak: "We considered using a custom protocol but rejected it as too complex."
- Strong: "**Chunk the image into separate LwM2M writes.** Keeps everything inside the protocol we already speak and needs no HTTP client on the device. We dropped it because a 4 MB image at 1 KB per write is 4000 round trips, which does not fit in a 30 second awake window even at best-case latency. **Do nothing and retry more often.** Free, and it is what we do today. Rejected because retries are what is burning the data, so more of them makes the complaint worse."

Include do-nothing every time. It is the option you are actually competing with, and it wins more often than people expect.

## Failure modes and risks

Answers: what happens when each piece breaks?

Test: every row names how you find out. A failure you cannot detect is an outage someone else reports to you.

A table keeps this honest:

| Failure | Detection | Behaviour | Blast radius |
|---|---|---|---|
| Power lost mid-flash-write | Bootloader sees an incomplete image on next boot | Boots the previous slot, reports the offset it reached | One device, one wake cycle |
| Signed URL expires before resume | Device gets 403 on the range request | Reports the error, gateway mints a new URL on the next wake | One device, one day at worst |
| Stored offset disagrees with flash contents | Offset checked against a hash of the written region | Discards the offset and starts over | One device, one full download |

Reach for the unglamorous cases: duplicate delivery, a peer on old firmware, a clock that moved, a queue that stops draining, two of the thing running at once. These are where field bugs come from. [domain-checklists.md](domain-checklists.md) has more per domain.

Do not write "the system is robust to failures". Show the row.

## Privacy

Answers: what personal data does this touch, and what are we obliged to do about it?

Test: names the data, where it lives, how long it stays, and what happens on a deletion request. A sentence asserting compliance is not an answer.

Every doc has this section. It needs the full treatment whenever the change carries, stores, or exposes identity, location, contact, or customer data. A location tied to a person is personal data under GDPR, so most changes on a device-to-backend path need it.

- Weak: "The system is GDPR compliant. No new personal data is exposed."
- Strong: "This adds a per-device download log to the backend database holding device ID, timestamp, and byte offset. No location and no customer identifier. Device ID links to a customer through the `device_owners` table, so it is personal data by association. Retention follows the existing 90 day rule on the `device_logs` table, and the current deletion path, which deletes by device ID, covers it. No new deletion work."

The strong version is longer but it is doing work: it names the fields, spots that an identifier is personal by association, and closes the deletion question. That is the reasoning a reviewer cannot do for you.

When the change touches none of that, one line listing the data the path does carry is enough: "No change. The path carries device ID and retry count only." "No personal data" on its own is not enough, because it does not show what you looked at.

## Security

Answers: what is the new way in, and what stops abuse of it?

Test: describes the change, not the system. The threat model covers what this doc adds.

Every doc has this section. It needs the full treatment when the change adds an entry point, a credential, or a permission, or exposes data to a new party. Otherwise one line saying none of those change is enough: "No change. No new endpoint, credential, or permission."

- Weak: "Uses TLS. Standard authentication applies. Mitigations for SQL injection, XSS, and DDoS are in place."
- Strong: "The resume path lets a device request an arbitrary byte range from its own signed URL. The URL is already scoped to one object and expires in 60 minutes, so the new capability is reading part of a firmware image the device is entitled to read in full. An attacker with a stolen PSK could resume another device's download only by also obtaining that device's URL, which the gateway writes per device over DTLS. The gateway's service identity gains read on the firmware bucket only, not list."

Generic attack-class lists are the clearest sign nobody thought about the change. One paragraph on the actual new capability beats a page of them.

## Validation

Answers: how will you know it works before customers do?

Test: names a quantity. How many devices, how long, and what number would send you back.

- Weak: "Will be tested thoroughly on hardware."
- Strong: "Unit coverage on the offset state machine, including a forced reset between every chunk. Hardware-in-the-loop run with the modem killed at 10%, 50%, and 99%. Then 10 devices in the office on a throttled link for 48 hours. Rollback trigger: completion rate under 95% or any device that fails to boot."

## Rollout and back-out

Answers: in what order does this ship, and what do you do when it goes wrong at 2am?

Test: each step is deployable on its own, and the back-out is a concrete action rather than "revert".

Call out the ordering rule explicitly when two sides must agree, for example that the gateway has to tolerate the new resource before any device reports it. Ordering mistakes are the most common way a correct design still breaks every deployed device.

If a step cannot be undone, and flashing firmware usually cannot, say so and name what stands in for it. A feature flag, a staged rollout, a kill switch on the gateway side.

## Open questions

Answers: what do you still not know?

Test: every row has an owner by name and a Needed by milestone.

A draft full of open questions is a healthy draft. An approved doc with unowned open questions is a list of future incidents.
