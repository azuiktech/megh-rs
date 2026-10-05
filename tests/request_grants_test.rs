//! The grants a request needs: one per resource level, ancestors read, custom verbs.

use megh::{request_grants, Grant};

fn grants(method: &str, template: &str, uri: &str) -> Vec<String> {
    request_grants(method, template, uri).iter().map(|g| g.as_str().to_string()).collect()
}

#[test]
fn a_get_of_one_resource_needs_read_on_that_id() {
    assert_eq!(grants("GET", "/v1/organizations/{organization}", "/v1/organizations/acme"), ["organizations:read:acme"]);
}

#[test]
fn the_http_method_gives_the_action() {
    let (t, u) = ("/v1/organizations/{organization}", "/v1/organizations/acme");
    assert_eq!(grants("PATCH", t, u), ["organizations:update:acme"]);
    assert_eq!(grants("PUT", t, u), ["organizations:update:acme"]);
    assert_eq!(grants("DELETE", t, u), ["organizations:delete:acme"]);
}

#[test]
fn a_collection_request_has_no_instance() {
    assert_eq!(grants("POST", "/v1/organizations", "/v1/organizations"), ["organizations:create"]);
    assert_eq!(grants("GET", "/v1/organizations", "/v1/organizations"), ["organizations:read"]);
}

#[test]
fn a_custom_verb_replaces_the_action_and_keeps_the_id() {
    assert_eq!(grants("POST", "/v1/organizations/{organization}", "/v1/organizations/acme:activate"), ["organizations:activate:acme"]);
}

#[test]
fn a_custom_verb_on_a_collection_has_no_instance() {
    assert_eq!(grants("POST", "/v1/organizations:batchCreate", "/v1/organizations:batchCreate"), ["organizations:batchcreate"]);
}

#[test]
fn every_resource_level_needs_its_own_grant() {
    let template = "/v1/organizations/{organization}/courses/{course}/lessons/{lesson}";
    let uri = "/v1/organizations/o/courses/c/lessons/l";
    assert_eq!(grants("GET", template, uri), ["organizations:read:o", "courses:read:c", "lessons:read:l"]);
}

#[test]
fn ancestors_need_read_and_only_the_leaf_uses_the_action() {
    let template = "/v1/organizations/{organization}/courses/{course}/lessons/{lesson}";
    let uri = "/v1/organizations/o/courses/c/lessons/l";
    assert_eq!(grants("PATCH", template, uri), ["organizations:read:o", "courses:read:c", "lessons:update:l"]);
}

#[test]
fn a_nested_collection_needs_the_parents_and_no_leaf_instance() {
    assert_eq!(
        grants("POST", "/v1/organizations/{organization}/courses", "/v1/organizations/o/courses"),
        ["organizations:read:o", "courses:create"]
    );
}

#[test]
fn a_custom_verb_on_a_nested_resource_applies_to_the_leaf_only() {
    assert_eq!(
        grants("POST", "/v1/organizations/{organization}/courses/{course}", "/v1/organizations/o/courses/c:publish"),
        ["organizations:read:o", "courses:publish:c"]
    );
}

#[test]
fn leading_literals_are_prefixes_not_resources() {
    assert_eq!(grants("GET", "/api/v1/invoices/{id}", "/api/v1/invoices/99"), ["invoices:read:99"]);
    assert_eq!(grants("GET", "/api/invoices", "/api/invoices"), ["invoices:read"]);
}

#[test]
fn ids_and_verbs_are_lowercased() {
    assert_eq!(grants("POST", "/v1/organizations/{organization}", "/v1/organizations/ORG_1:Activate"), ["organizations:activate:org_1"]);
}

#[test]
fn the_derived_grants_can_be_checked_with_implies() {
    let derived = request_grants("POST", "/v1/organizations/{organization}", "/v1/organizations/acme:activate");
    assert!(Grant::new("organizations:read,activate:*").implies(&derived[0]));
    assert!(!Grant::new("organizations:read:*").implies(&derived[0]));
}

#[test]
fn a_path_without_a_resource_needs_a_root_grant_rather_than_none() {
    assert_eq!(grants("GET", "/", "/"), ["root:read"]);
}
