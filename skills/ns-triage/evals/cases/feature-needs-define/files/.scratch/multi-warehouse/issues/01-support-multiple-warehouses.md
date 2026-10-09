# Support multiple warehouses with transfers

Status: needs-triage
Category:
Priority: priority:low

Reported by: Priya (operations lead)

## Description

We're opening a second site in the spring, and probably a third next year. The inventory tool needs to handle multiple warehouses.

Rough wishlist:

- each site has its own stock
- move stock between sites (transfers), and see what's in transit
- reservations should come from the nearest site, or maybe any site with stock? Not sure yet
- a combined report across all sites, plus per-site reports
- the CLI should still work for people who only care about one site

Not sure how the state file should look with several sites, or whether we need to migrate the existing one. Open to suggestions.

## Comments
