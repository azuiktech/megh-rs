# megh — Product Requirements

megh is a shared backend for app development, in the family of Firebase and PocketBase: an app brings its screens and its business rules; megh brings the parts every business app needs. Written for agents; no technology.

Marks: `✅` shipped · `[~]` partly shipped, with what is missing named · `[ ]` not started.

## Features

| # | Feature | Status | TPD |
|---|---|---|---|
| 1 | Multi-tenant: every record belongs to a tenant, the partition a business works in | ✅ | [TPD-tenants](TPD-tenants.md) |
| 2 | Subscriptions with add-ons, like a software subscription: plans, prices, seats, add-ons, cancel | ✅ | [TPD-billing](TPD-billing.md) |
| 3 | Members of a tenant; one person can be a member of many tenants | [~] members and token done; invites, auto-join and onboarding missing | [TPD-tenants](TPD-tenants.md) |
| 4 | Simple entity system over the database with audit: typed rows, generic create/read/update/delete, paging and filters, soft delete, row-change audit. Not an ORM and not meant to become one | ✅ | [TPD-booking](TPD-booking.md), [TPD-audit](TPD-audit.md), [TPD-soft-delete](TPD-soft-delete.md) |
| 5 | Path-based authorization: a grant names a resource, an action and optionally an instance; the route decides which grant a call needs | ✅ | [TPD-authentication-authorization](TPD-authentication-authorization.md), [TPD-grants](TPD-grants.md) |
| 6 | Sign-up and login: OAuth providers and basic (password) login; machines sign in with client credentials | ✅ | [TPD-authentication-authorization](TPD-authentication-authorization.md) |
| 7 | Front-end SDK for sign-up and login | [~] OAuth popup flow done; basic login in the SDK not confirmed | [TPD-authentication-authorization](TPD-authentication-authorization.md) |
| 8 | Products and inventory: products with a price history, stock per tenant that sales and purchases change | [~] | [TPD-booking](TPD-booking.md), [TPD-billing](TPD-billing.md) |
| 9 | Booking: services, providers, resources, clients, schedules, availability search, book, cancel, reschedule | [~] reschedule not confirmed | [TPD-booking](TPD-booking.md) |
| 10 | Durable events: events are held in the database, not in memory; each is delivered exactly once in practice (very high probability); back pressure on slow consumers; events are held while a connection is poor, for front ends and for back-end consumers | [~] live push and catch-up from memory done; database storage, back pressure and holding for back-end consumers not started | [TPD-events](TPD-events.md) |
| 11 | Messaging between a user and the business's agent (below) | [ ] | [TPD-messaging](TPD-messaging.md), to write |
| 12 | Invoicing and payment: invoices (receivable and payable), lines, taxes, numbering, payments, refunds | [~] | [TPD-billing](TPD-billing.md) |

## Messaging (feature 11)

Messaging is for support, appointment booking, product purchase and the like: a user talking to the business's agent, usually an AI agent. It is **not a user-to-user chat room**. Modelled on the Facebook Messenger, Twilio and WhatsApp connectors. The user writes free text; the agent answers with text, attachments and structured forms (a2ui 1.0). Its design lives in its own TPD.

| # | Feature | Status |
|---|---|---|
| 11.1 | Conversation: one thread between a user (member, client or anonymous visitor) and the tenant's agents or staff; may link a client, appointment, product or invoice | [ ] |
| 11.2 | Messages: free text and attachments from user and system; reply-to; attachment size cap configurable | [ ] |
| 11.3 | Labels on a thread (tenant-defined; set by user, staff or agent) | [ ] |
| 11.4 | Indicators: typing, sent/read, unread counts | [ ] |
| 11.5 | Structured messages: the agent sends any a2ui 1.0 surface; the user fills or answers it | [ ] |
| 11.6 | Canned templates as surfaces (after Messenger): quick replies (up to 13), button template (text and up to 3 buttons), carousel (up to 10 cards), receipt from an invoice and payment | [ ] |
| 11.7 | Ready-made surfaces: (1) pick products/services/providers with optional count; (2) login/logout as member; (3) open times per product/service; (4) calendar for a date or range; (5) client intake; (6) feedback: numeric rating or good/ok/poor, optional text and attachment; (7) approve/reject with field name: value; (8) any generic a2ui surface is always accepted | [ ] |
| 11.8 | Anonymous visitors may chat; the agent shows the login surface when it needs a member | [ ] |
| 11.9 | Agents are machine members using the same interface; megh holds no AI logic | [ ] |
| 11.10 | Delivery: live push; a reconnecting client receives everything still pending | [ ] |
| 11.11 | Web UI: conversation list, thread, composer, indicators, surface renderer; host apps style it | [ ] |
| 11.12 | External connectors (later): Facebook Messenger, Twilio, WhatsApp map external threads onto conversations | [ ] |

Open: whether a messaging window (Messenger: 24 hours, 7 days for a human agent) applies to external channels.
