# Clarifications

The user's answers to design questions, in their terms, newest section first. Binding until the user changes them.
Read before proposing or changing anything.

## 2026-10-24 — the customer is an org that belongs to the seller (S5, approved)

- `parent_id` on `organizations` is the interim hierarchy (one parent, or none).
- "A client's org has the client's id (trigger plus backfill)": creating a client, by any path, also creates an org with the client's own id, named like the client, under the business that has the client; existing clients are backfilled.
- "The customer check on create and PUT only": an invoice's `customer_id` must be an org whose parent is the seller (400 otherwise); it is not checked again at `:open`.
- "The invoice tests updated for it."
- "No need to do any changes in megh-go. Want to have an implementation for megh-rs only. Will come to megh-go later." So megh-rs may add columns to `organizations` (`parent_id`) that megh-go does not have; the parity test lists it.

## 2026-10-23 — payments keep the invoice balance (S4, approved)

- Processing and succeeded payments count toward `amount_paid` (the sum of `amount` less `refunded`); a failed or void payment does not. `balance_due` is the total less `amount_paid`.
- "No overpaying with 409": a payment that would pay more than is owed is refused.
- "Refund via PATCH `refunded`": the refund is the refunded part of the payment; no new verb.
- "Number set when recorded and never changed": the payment's number comes from the app's payment `Ids` when it is recorded; a body that sets `number` is refused.
- "The payment tests updated for the number and Formats."

## 2026-10-23 — open and void an invoice (S3, approved)

- Totals: "per-line half-up rounding" (each line's base, tax and total are rounded to the currency's minor unit, halves up; the invoice is the sum of its lines). `:open` and `:void` as described: `:open` needs a line, takes the number from the app's `Ids`, fixes the totals and locks the invoice; `:void` only for an open invoice with nothing paid.
- "`bill_to` kept as drafted until Org has an address": `:open` does not copy the customer's address yet.
- "`Formats` as an Extension": the app gives the billing router its invoice and payment `Ids` with an axum `Extension`.

## 2026-10-23 — billing is simple invoicing and payment

- "It should be simple: an inventory and an invoicing." Billing already has subscriptions (the SaaS plans of an org); the invoicing goes in the same module. "I want to make it absolutely simple."
- HubSpot is the reference for invoicing (https://developers.hubspot.com/docs/api-reference/latest/overview), checked repeatedly. Product and inventory are not from HubSpot: "those need to be taken from Odoo / Zoho."
- Discount and tax are a percentage only for now. "Note down that a fixed slab is to be done later (usually a fixed discount are vouchers which can be applied to a payment)."
- "The vendor is nothing but an org." No separate vendor entity. The customer also always points to an org: "if an org be auto generated to a client, so it always points to an org."
- "ExchangeRate likely not useful. Where is it to be used? All are on a fixed base unit." Removed. A payment has no PaymentKind, no location, no exchange rate, no reference ("too many unnecessary fields").
- Cut approved ("mostly ok"): Vendor, PaymentKind, the purchase side, ExchangeRate, extra payment fields, credit notes, Application, PaymentTerm, Order and OrderLine, Document, Party. A payment pays one invoice and keeps its refunded part (as HubSpot does); an invoice stores its amounts (subtotal, taxes, total, amount paid, balance due) as HubSpot does.
- The id of an invoice and of a payment (receipt) is natural and customizable: "a prefix, separator, optional middle part (usually date) and sequential." "Its constructor takes a function to generate the id. It is stored the same way (Name etc)." So: an `Ids` function supplied by the app, the result stored as a `Name`, a counter row per org, kind and scope. Prefix can be generic (INV) or the company abbreviation.
- "What is Document? And why hardcoding the table names? All entities have a Table annotation to give the name and can be configured." Helpers use the entity's `TABLE_NAME`, not a hardcoded name.
- Interim for who may be a customer: a nullable `parent_id` on `organizations`; a customer org must have the seller as its parent (approved, S5).
- Plain words: "too much complex ... with lots of unnecessary jargons". Explain in plain words.

## 2026-10-22 — address, org, entities (earlier in the same work)

- A generic address type, used for the org, the invoice and so on (exists: `booking::value::Address`). "Any shared value can go to a common module, e.g. address and contact can be moved to org and used." Reorganizing modules is for later: "entities are public, so should work without moving."
- Org: industry, language and primary "don't make sense that much". Keep name and description, add email and home, work and mobile phone. "Org can be implemented as entity. That is perfectly ok."
- Product is its own entity, not Service. Org denotes both organization and location through the flexible hierarchy, the same as booking. Quantity is a decimal (`rust_decimal`). A `completed` status works for both product and service. The same grouping serves the product catalogue. A SKU can be at multiple locations; like a room it has a single owner, and like capacity it has a stock count.
- Money has a currency with a minor unit and a factor; rounding is by the library (`rusty_money`).
