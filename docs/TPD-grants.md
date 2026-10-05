# TPD — Grants per resource level

Status: `[x]` done (PR pending, issue #67). Extends F7 of [TPD-authentication-authorization](TPD-authentication-authorization.md). Supersedes the approach of #65 / #66.

## Why
Resource-oriented APIs ([Google API design guide](https://cloud.google.com/apis/design)) nest resources and add custom verbs:

```
GET    /v1/organizations/{organization}
POST   /v1/organizations/{organization}/courses
PATCH  /v1/organizations/{organization}/courses/{course}
POST   /v1/organizations/acme:activate
```
Each level is its own resource with its own rule. The authorizer derived one grant from the last segment, which also got routes with a leading parameter wrong (#65).

## Grant format (unchanged, same as megh-go)
`resource:action:instance`. Each part may be `*`, or a comma list (`read,write,batchCreate`, `1,2,5`); trailing parts may be omitted (all); matching is case-insensitive. `Grant` and `Grant::implies` do not change.

## Interface
```rust
pub fn request_grants(method: &str, matched_template: &str, uri_path: &str) -> Vec<Grant>;
```
`authorizer` requires **every** grant in the list to be implied by the member's grants (a `Member`'s, else the `Vec<Grant>` extension). 401, 403 and 404 are as before. `request_grant` (one grant) stays as it is and is no longer used by `authorizer`.

## Derivation
The matched template says which path segments are ids; the request path gives the values. Under `Router::nest` the path is the original URI (`OriginalUri`), so the two align.

1. A literal template segment is a **resource** when a `{param}` follows it or it is the last segment. Other literals (`v1`, `api`) are prefixes and produce no grant. A `{param}` is the **id** of the resource before it.
2. The last path segment may end in `:verb`. The verb is cut off; the rest is that segment's value or name (`acme:activate` → id `acme`; `organizations:batchCreate` → resource `organizations`).
3. Action (confirmed by the owner): the leaf level (the last resource) uses the verb if there is one, else the HTTP method (`POST` create, `PUT`/`PATCH` update, `DELETE` delete, otherwise read). **Every ancestor level uses `read`.** 
4. Instance: the id value, or none for a level without an id (list, create), which only a grant with `*` or no instance satisfies.

| Request | Grants required |
|---|---|
| `GET /v1/organizations/acme` | `organizations:read:acme` |
| `POST /v1/organizations` | `organizations:create` |
| `POST /v1/organizations/acme:activate` | `organizations:activate:acme` |
| `POST /v1/organizations:batchCreate` | `organizations:batchcreate` |
| `GET /v1/organizations/o/courses/c/lessons/l` | `organizations:read:o`, `courses:read:c`, `lessons:read:l` |
| `PATCH /v1/organizations/o/courses/c/lessons/l` | `organizations:read:o`, `courses:read:c`, `lessons:update:l` |
| `POST /v1/organizations/o/courses` | `organizations:read:o`, `courses:create` |
| `POST /v1/organizations/o/courses/c:publish` | `organizations:read:o`, `courses:publish:c` |

A grant `organizations:read,activate:*` satisfies both the `GET` and the `:activate` rows. A grant on one level implies nothing about another: `organizations:*` does not satisfy `courses:read:c`. No action implies another either: `update` is not `read`, so a member who may edit a lesson also needs `read` on its parents, and on the lesson itself for a `GET`, listed explicitly (`lessons:read,update:l`).

## Not in this change
Checking that the id in `organizations/{o}` is the member's own organization; porting the rule to megh-go; migrating stored grants in applications.

## Verification
`tests/request_grants_test.rs` (the table above, prefixes, case, nesting) and `tests/authorizer_levels_test.rs` (the middleware: independent levels, comma lists, custom verbs, `nest`).
