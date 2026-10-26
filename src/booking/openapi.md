Booking for any kind of business: rooms and other resources, providers, clients, services, schedules,
blockouts, open slots, holds and appointments. Every path is under an org: `/api/v1/orgs/{org_id}/...`; the
org in the path always wins over one in the body.

**Signing in.** `POST /auth/login` with Basic credentials sets the `jwt_token` cookie. Every route needs that
cookie or the same token as `Authorization: Bearer <token>`; a machine gets one from `POST /auth/megh/token`
(client credentials, `Authorization: Basic` with its client id and secret). A browser write from another site is refused (403) unless it carries a bearer token or its origin is trusted. Each route needs a grant derived from its path: `orgs:read:{org_id}`, then
`<resource>:<action>` or `<resource>:<action>:{id}`, where the action is `read`, `create`, `update`, `delete`,
or the custom method after `:` (`appointments:book`, `appointments:available`, `reservations:purge`).

**Writes.** POST takes the entity without its id; PUT takes the whole entity; PATCH is a JSON merge patch
(RFC 7396): fields given are changed, fields set to `null` go back to their default. DELETE of an appointment
cancels it; DELETE of anything else is a soft delete.

**Lists.** `page_size` and the opaque `page_token` from the last page; `filter` is an AIP-160 filter
(https://google.aip.dev/160): `field = "value"`, `!=`, `<`, `<=`, `>`, `>=`, `AND`, `OR`, `NOT`, `list:"value"`
for list fields. A name that is not a field of the entity is looked up in `attributes`.

**Times.** Instants are RFC 3339 (`2026-10-26T09:00:00+05:30`); durations are ISO 8601 (`PT45M`); a schedule's
`recurrence` is RFC 5545 text (`DTSTART;TZID=Asia/Kolkata:20261023T000000\nRRULE:FREQ=WEEKLY;BYDAY=MO,WE`)
whose days get the window `starts_at`–`ends_at`.

**Booking.** `POST /appointments:available` finds open slots; `POST /reservations` holds one for 10 minutes;
`POST /appointments` books, checked against what is free at that moment, with `reservation_id` to book from a
hold.
