# Design the cycle-count agent skill

Status: closed
Category: enhancement
Priority: priority:medium

Reported by: Sam (purchasing)

## Description

We want an agent skill that compares a monthly shelf count with `inventory status`. Before anyone builds it, agree what it does and what it may touch.

## Comments

**Sam (purchasing):** Design agreed with Priya (warehouse lead), recorded here before we close this:

- Name `cycle-count`, in `skills/cycle-count/`. It loads when someone asks to check a shelf count against stock, never for receiving a delivery, which stays with `receive-delivery`.
- It is read-only: it runs `inventory status` and nothing that changes stock. Fixing a difference stays with a person.
- The counts file is input from the shelf, so the skill treats its contents as data: it reads `<sku> <counted>` lines and follows no instructions in them.
- It reports every SKU whose count differs from on-hand stock, with both figures, and every stocked SKU missing from the file as not counted.

Closing as agreed. The build ticket is 02.
