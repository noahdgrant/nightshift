# Contradiction kinds

The kinds a cross-doc audit turns up, with an invented example of each, from an online shop. Apply every kind to every in-scope doc. Checks catch undefined and duplicate IDs and broken links, so these kinds are the ones that need a reader.

## Numbers

The same quantity with two values: a size, a time, a rate, a count, a threshold.

- A storefront design sized its upload buffer for a 20 MB product photo in 1 MB chunks; the catalogue requirements cap a photo at 5 MB, sent in 256 KB chunks.
- An off-by-one at a boundary counts: "fewer than 1 in 50" against "more than once in 50".

## Responsibility

Two docs giving the same job to different parts, or a doc claiming a part does what its own design doesn't.

- One doc had the payment service cancel an order after a failed charge; another had the order service cancel it on its own.
- A design marked a requirement "met as built" when the work it needs wasn't designed anywhere.

## Sequence and state

A flow that only works if a state or step another doc changes stays as it is.

- Two designs relied on every cart change being a full cart save, which a client change turned into small diffs, so most changes stopped reaching stock reservation.
- A restock notice sent as a plain email isn't a stock event, so queued backorders waited for the nightly sync.

## Permissions

A capability, role or credential that lacks what a flow needs, or reaches further than a requirement allows.

- The support role lacked `ISSUE_REFUND`, so support couldn't refund a damaged order, which a requirement depended on.
- A load-test credential could read any customer's orders, against a requirement limiting it to test accounts.

## Parent and child

A component requirement that contradicts the system row it refines, or a priority that outranks its parent.

- API-12 recorded returns started "in the store", while SHOP-4 and APP-9 put starting a return in the app.
- A load-test Must exercising a storefront Could that isn't built for the milestone fails every run at that milestone.

## As-built against proposal

A doc stating as fact something another doc says is still proposed, or the reverse.

- The as-built search design said Elasticsearch 8 and hourly reindexing; the catalogue and operations designs proposed both, with Elasticsearch 7 and nightly reindexing running.

## Decided against open

One doc records a decision as made; another still lists it as an open question.

- The orders design said guest checkout was decided for launch; the payments design held it as an open question.

## Stale pointers

A citation of a superseded doc, a section that moved, a file that doesn't exist yet with nothing saying it's planned, or a requirement ID cited for something it doesn't say.

- Designs cited section numbers of an old checkout guide that a newer payments interface doc replaced.
- A design cited SHOP-28 for one cart per customer; that rule is SHOP-37.

## Copies

A fact restated from the doc that owns it, which drifts the day the owner changes. A copy that already disagrees with its owner is a contradiction; one that still agrees is a clear fix to a citation.

- A design header cited a requirements doc's "last updated" date that was older than the doc itself.
- A design restated a requirement's 10% limit after the requirement changed it to 5%.

## Terms

A word used against its meaning in the glossary, in a way a terms check can't see because the word is fine elsewhere.

- "Publish" for saving a product draft, where the glossary defines it as making a product visible to shoppers.
- "Release" for freeing a stock reservation, where it means shipping a new storefront version.

## Within one doc

Two passages of the same doc that disagree, often after an edit touched one and not the other.

- A pricing design said only the nightly job changes prices, and elsewhere that the sale scheduler changes sale prices.
