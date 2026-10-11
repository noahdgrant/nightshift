# Contradiction kinds

The kinds a cross-doc audit turns up, with an example of each. Apply every kind to every in-scope doc. Checks catch undefined and duplicate IDs and broken links, so these kinds are the ones that need a reader.

## Numbers

The same quantity with two values: a size, a time, a rate, a count, a threshold.

- A server sized its cache for a 4 MB firmware image in 1 KB blocks; the device requirements cap the image at an 864 KB slot, pulled in 512-byte blocks.
- An off-by-one at a boundary counts: "fewer than 1 in 50" against "more than once in 50".

## Responsibility

Two docs giving the same job to different parts, or a doc claiming a part does what its own design doesn't.

- One doc had the server start recovery mode after a theft; another had the device start it on its own.
- A design marked a requirement "met as built" when the work it needs wasn't designed anywhere.

## Sequence and state

A flow that only works if a state or step another doc changes stays as it is.

- Two designs relied on every check-in being a full registration, which a firmware change turned into short updates, so most check-ins stopped reaching the backend.
- An alert sent as a bare message isn't a check-in, so queued device commands waited for the next heartbeat.

## Permissions

A capability, role or credential that lacks what a flow needs, or reaches further than a requirement allows.

- The owner role lacked `SET_CONFIG`, so the owner's phone couldn't turn cellular back on, which a requirement depended on.
- A test rig credential could reach any unpaired device, against a requirement limiting it to devices under test.

## Parent and child

A component requirement that contradicts the system row it refines, or a priority that outranks its parent.

- SERVER-12 recorded plans chosen "in the app", while SYS-30 and MOBILE-19 put buying plans on the web.
- A test rig Must checking a device Could that isn't built for the milestone fails every run at that milestone.

## As-built against proposal

A doc stating as fact something another doc says is still proposed, or the reverse.

- The as-built database design said Postgres 16 and merge-triggered deploys; the cloud and delivery designs proposed both, with Postgres 15 and tag deploys running.

## Decided against open

One doc records a decision as made; another still lists it as an open question.

- The database design said audit logging was decided for production; the cloud design held it as an open question.

## Stale pointers

A citation of a superseded doc, a section that moved, a file that doesn't exist yet with nothing saying it's planned, or a requirement ID cited for something it doesn't say.

- Designs cited section numbers of an old pairing guide that a newer interface doc replaced.
- A design cited SERVER-28 for one environment per device; that rule is SERVER-37.

## Copies

A fact restated from the doc that owns it, which drifts the day the owner changes. A copy that already disagrees with its owner is a contradiction; one that still agrees is a clear fix to a citation.

- A design header cited a requirements doc's "last updated" date that was older than the doc itself.
- A design restated a requirement's 10% limit after the requirement changed it to 5%.

## Terms

A word used against its meaning in the glossary, in a way a terms check can't see because the word is fine elsewhere.

- "Promote" for setting a channel default, where the glossary defines it as deploying an image between environments.
- "Rollout" for model updates, where it means setting pending firmware.

## Within one doc

Two passages of the same doc that disagree, often after an edit touched one and not the other.

- A delivery design said only one workflow moves the environment tags, and elsewhere that a second workflow moves the production tag.
