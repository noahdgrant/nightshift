---
name: ns-service-design
description: What a network service needs always and what it needs only on evidence. Use when a unit builds or changes a network service.
---

# Service design

Reference only. A service needs what holds up under failure first, and scale only when something shows the need.

## Guardrail or safety net

A **guardrail** prevents a mistake: types that admit only valid values, validation at the edge, limits, an interface that is hard to misuse. A **safety net** limits the damage after one: rollback, transactions, retry, a circuit breaker, recovery. A service has both. For each risk, the design names which one it uses.

## Every service has

Each is added at every boundary the unit adds, whatever the load.

- **Timeout**: on every call out of the process, set where the call is made.
- **Retry**: with a growing wait, only on a call that is safe to make twice and only on a failure known to be transient. How is in the retry section of `.nightshift/coding-standards.md`; do not restate it.
- **Idempotency key**: on every write a client may send twice, checked where the write is stored. It is what makes the retry safe.
- **Rate limit**: on every endpoint a stranger can reach, at the edge.
- **Circuit breaker**: around each dependency that can fail, in the one place that calls it.
- **Logs, metrics and tracing**: so a failure can be seen and located, written where the request enters and where each call out is made.

## Scale levers, on evidence only

Add a lever only when the unit's brief or a measurement shows its symptom. A lever added with no symptom shown is a finding in the Spec area of the review.

| Symptom | Lever |
| --- | --- |
| slow reads | a cache |
| write spikes | a queue |
| too much traffic | a load balancer |
| a read-heavy database | read replicas |
| a write-heavy database | sharding |
| slow queries | indexes |
| slow file loads | object storage and a CDN |
