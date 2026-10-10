# Low-stock reordering

Status: needs-triage
Category:
Priority: priority:medium

Reported by: Sam (customer)

## Description

We keep running out of fast-moving items because nobody notices until a reservation fails. I'd like the inventory tool to help us reorder.

What we need, in this order of importance:

1. A reorder point per item. `inventory set-reorder-point <sku> <quantity>` stores it in the state file. Items without one never count as low. Existing state files must keep loading.
2. `inventory status --low` lists only items whose available stock is at or below their reorder point, in the same table as `status`.
3. `inventory reorder-report <path>` writes a CSV with one row per low item: `sku,name,available,reorder_point,suggested_order`, where `suggested_order` is twice the reorder point minus available.

The second and third obviously need the first. We'd use 2 every morning and 3 once a week when we place orders, so we could live with 2 shipping before 3.

## Comments
