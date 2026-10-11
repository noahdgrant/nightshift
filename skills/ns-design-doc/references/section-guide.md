# What good looks like, section by section

Each entry gives the question the section answers, the test for a good answer, and a weak/strong pair. The examples follow one invented change, reserving stock when an online shop's checkout starts, so the shape is recognisable. They are illustrations, not records of real decisions. For an as-built doc, `SKILL.md` lists which sections change and how.

## Title

Answers: what is changing?

Test: a reader who knows the system can guess the doc's content from the title alone.

- Weak: "Checkout Design Doc"
- Strong: "Reserve stock when checkout starts instead of checking it at payment"

Name the change when the doc proposes one. Name the component only when the doc introduces that component, or describes it as-built, as in "Sync service". A title like "[Component] Design Doc" on a proposal gives a reader no way to tell this doc from the last three about the same component.

## Summary

Answers: what should I take away if I read nothing else?

Test: three sentences, and the third one names a cost. A summary with no cost in it is a pitch.

- Weak: "This document describes improvements to the checkout process to make it more robust and efficient."
- Strong: "A shopper buying the last unit can lose it to another shopper mid-checkout, and we charge them, then refund them. We reserve the stock when checkout starts and release it if they don't pay within 15 minutes. It costs a new reservations table and a job that sweeps expired holds, and busy items show slightly less available stock."

## Problem

Answers: what is broken, for whom, and why does it stop being tolerable now?

Test: contains at least one fact you could check. A ticket number, a log line, a measured rate, a customer sentence.

- Weak: "Checkout sometimes fails, causing frustration for users."
- Strong: "Across the June sale, 230 of 14,000 orders were charged and then refunded as out of stock, and 41 of those shoppers complained. Every one was an item with fewer than 5 units left. The holiday sale doubles traffic on the same items, so the count will rise."

If you cannot write a checkable fact, the honest move is to say the problem is suspected and name the measurement that would confirm it.

## Requirements: goals and non-goals

Answers: what must be true when this is done, and what are you refusing to do?

Test: each goal is something you could pass or fail. Each non-goal has a reason attached.

Where a requirements doc exists, name it in the header by path or URL with the version this design answers, and start each goal with the ID of the requirement it answers. Cite the ID rather than quoting the requirement: a quoted copy disagrees with its source the first time the requirement changes. The IDs are what let someone changing a requirement find this doc later.

- Weak goal: "Improve reliability of checkout."
- Strong goal: "STOCK-3: a shopper who starts checkout on an in-stock item can pay for it within 15 minutes without it selling out underneath them."

- Weak non-goal: "Performance optimization is out of scope."
- Strong non-goal: "We are not adding backorders. They need supplier lead times we don't hold, and reservations alone remove the refunds. Revisit if sold-out items are still the top complaint after this ships."

Non-goals are the most useful part of most design docs, because they are what reviewers try to add. Write them early and defend them.

## Constraints

Answers: what cannot move, and who says so?

Test: every constraint names its source, so a reviewer can challenge the source rather than argue with you. A limit set by a requirement, another design, or an interface cites its ID or links its section, with the ID beside any value you repeat.

- Weak: "Limited database capacity and payment provider limits."
- Strong: "The orders database peaked at 1,800 writes per second against a 2,500 ceiling in the June sale (its dashboard), so a reservation may add at most one write per checkout. The payment provider keeps a checkout session open for at most 30 minutes (its documentation), so a hold that outlives the session keeps stock nobody can pay for. Checkout runs in the shopper's browser, so no step may assume the page stays open."

Separate real constraints from decisions you have already made. The 15 minute hold is a number the team chose, so it is a decision. A decision belongs in Design, where it can be argued with.

## Architecture

Answers: what are the pieces, and who talks to whom?

Test: the diagram and the component lines together explain a path through the system that the Design section then details. If Design repeats them, one of the two sections is redundant.

Include this only when the change adds a component or changes how existing ones talk. A change inside one component does not need it, and filling it in anyway produces a diagram of the system as it already is, which every reader already has.

Keep each component to a line or two: what it is responsible for, who it talks to. Depth belongs in Design.

Leave out the technology stack unless the change adds to it. The language, build tool and server library in a doc about queue draining tell the reader nothing they did not know and cost a paragraph.

## Design

Answers: how does it work?

Test: a reviewer could build roughly the right thing from it, and can point at the exact line they disagree with.

Write the main path first, in order, then the edges. Name real identifiers: endpoints, table and column names, topic names, message types, config keys, function names where they already exist. A design written in generic nouns cannot be reviewed, because every reader fills the nouns in differently.

Interfaces get before-and-after treatment, always:

- Weak: "The storefront tells the stock service about the reservation."
- Strong: "`POST /v2/reservations` on the stock service takes `sku`, `quantity` and `cart_id`, and returns `reservation_id` and `expires_at`. Checkout sends `reservation_id` with the payment. The v1 storefront sends none; the order service treats a missing `reservation_id` as today's stock check at payment, so a mix of old and new storefronts keeps working."

That last clause is the one reviewers care about most and the one most often missing. Say what happens when the two sides disagree about version.

When the change reshapes several components, a per-component breakdown helps: responsibilities, inputs and outputs, error handling, key data structures, where state lives. When it touches one component, that structure produces five thin subsections instead of one clear paragraph. Use it when there is something to put in every slot, not because the template offered the slots.

## Alternatives considered

Answers: why this and not the obvious other thing?

Test: a reader who arrived with a different idea finds it here, described fairly.

- Weak: "We considered using a custom protocol but rejected it as too complex."
- Strong: "**Lock the stock row for the whole checkout.** Simplest, and needs no new table. We dropped it because a checkout lasts minutes, and a held lock blocks every other shopper on that item, which at the June sale's 40 checkouts a minute on top items would stall the page. **Do nothing and refund faster.** Free, and it is what we do today. Rejected because the refund is the complaint, so a faster one still leaves the shopper charged for something they can't have."

Include do-nothing every time. It is the option you are actually competing with, and it wins more often than people expect.

## Failure modes and risks

Answers: what happens when each piece breaks?

Test: every row names how you find out. A failure you cannot detect is an outage someone else reports to you.

A table keeps this honest:

| Failure | Detection | Behaviour | Blast radius |
|---|---|---|---|
| Sweep job stops | Alert when the oldest expired hold is over 5 minutes old | Expired holds keep stock unavailable until the job restarts | Every item with an expired hold, until restart |
| Payment lands after the hold expired | Order service sees `expires_at` in the past | Checks stock again, refunds only if it is gone | One order |
| Two holds for the same cart and item | Unique key on `cart_id` and `sku` | The second insert returns the first hold | None |

Reach for the unglamorous cases: duplicate delivery, a peer on an old version, a clock that moved, a queue that stops draining, two of the thing running at once. These are where production bugs come from. [domain-checklists.md](domain-checklists.md) has more per domain.

Do not write "the system is robust to failures". Show the row.

## Privacy

Answers: what personal data does this touch, and what are we obliged to do about it?

Test: names the data, where it lives, how long it stays, and what happens on a deletion request. A sentence asserting compliance is not an answer.

Every doc has this section. It needs the full treatment whenever the change carries, stores, or exposes identity, location, contact, or customer data. A location tied to a person is personal data under GDPR, so most changes that touch an order or a delivery address need it.

- Weak: "The system is GDPR compliant. No new personal data is exposed."
- Strong: "This adds a `reservations` table holding SKU, quantity, cart ID, and expiry. No address and no payment data. Cart ID links to a customer through the `carts` table, so it is personal data by association. Retention follows the existing 30 day rule on cart data, and the current deletion path, which deletes carts by customer ID, cascades to it. No new deletion work."

The strong version is longer but it is doing work: it names the fields, spots that an identifier is personal by association, and closes the deletion question. That is the reasoning a reviewer cannot do for you.

When the change touches none of that, one line listing the data the path does carry is enough: "No change. The path carries SKU and quantity only." "No personal data" on its own is not enough, because it does not show what you looked at.

## Security

Answers: what is the new way in, and what stops abuse of it?

Test: describes the change, not the system. The threat model covers what this doc adds.

Every doc has this section. It needs the full treatment when the change adds an entry point, a credential, or a permission, or exposes data to a new party. Otherwise one line saying none of those change is enough: "No change. No new endpoint, credential, or permission."

- Weak: "Uses TLS. Standard authentication applies. Mitigations for SQL injection, XSS, and DDoS are in place."
- Strong: "The reservation endpoint lets any shopper with a cart hold stock. A cart holds at most one order's quantity per SKU, and a hold lapses in 15 minutes, so the new capability is keeping an item briefly out of stock. A bot opening thousands of carts could hold a whole product line; the existing edge rate limit applies per session and per IP, and a session asking for more than 10 holds a minute is refused. The stock service's identity gains insert and delete on `reservations` only."

Generic attack-class lists are the clearest sign nobody thought about the change. One paragraph on the actual new capability beats a page of them.

## Validation

Answers: how will you know it works before customers do?

Test: names a quantity. How many orders, how long, and what number would send you back.

- Weak: "Will be tested thoroughly in staging."
- Strong: "Unit coverage on hold expiry, including a payment that lands one second after `expires_at`. A load test at twice the June sale's peak with the sweep job paused for 10 minutes. Then holds on for 5% of shoppers for one week. Rollback trigger: out-of-stock refunds not falling by half, or checkout conversion dropping by more than 1%."

## Rollout and back-out

Answers: in what order does this ship, and what do you do when it goes wrong at 2am?

Test: each step is deployable on its own, and the back-out is a concrete action rather than "revert".

Call out the ordering rule explicitly when two sides must agree, for example that the order service has to accept a `reservation_id` before any storefront sends one. Ordering mistakes are the most common way a correct design still breaks every live client.

If a step cannot be undone, and dropping a column usually cannot, say so and name what stands in for it. A feature flag, a staged rollout, a kill switch in the order service.

## Open questions

Answers: what do you still not know?

Test: every row has an owner by name and a Needed by milestone.

A draft full of open questions is a healthy draft. An approved doc with unowned open questions is a list of future incidents.
