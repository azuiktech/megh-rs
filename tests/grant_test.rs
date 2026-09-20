//! Unit tests for Shiro Grant matching and route grant derivation.

use megh::{request_action, request_grant, Grant};

#[test]
fn test_request_action_mapping() {
    assert_eq!(request_action("POST"), "create");
    assert_eq!(request_action("post"), "create");
    assert_eq!(request_action("PUT"), "update");
    assert_eq!(request_action("PATCH"), "update");
    assert_eq!(request_action("DELETE"), "delete");
    assert_eq!(request_action("GET"), "read");
    assert_eq!(request_action("HEAD"), "read");
    assert_eq!(request_action("OPTIONS"), "read");
}

#[test]
fn test_request_grant_derivation() {
    // Static routes
    assert_eq!(
        request_grant("GET", Some("/api/invoices"), "/api/invoices"),
        Grant::new("invoices:read")
    );
    assert_eq!(
        request_grant("POST", Some("/courses"), "/courses"),
        Grant::new("courses:create")
    );

    // Parameterized routes with :id syntax
    assert_eq!(
        request_grant("GET", Some("/api/invoices/:id"), "/api/invoices/99"),
        Grant::new("invoices:read:99")
    );
    assert_eq!(
        request_grant("DELETE", Some("/api/members/:id"), "/api/members/mem-1"),
        Grant::new("members:delete:mem-1")
    );
    assert_eq!(
        request_grant("PATCH", Some("/courses/:id"), "/courses/c-100"),
        Grant::new("courses:update:c-100")
    );

    // Parameterized routes with {id} syntax
    assert_eq!(
        request_grant("GET", Some("/courses/{id}"), "/courses/c-100"),
        Grant::new("courses:read:c-100")
    );
}

#[test]
fn test_shiro_hierarchical_implication() {
    // Shiro rule: granted "invoices:read" implies requested "invoices:read:99"
    let g = Grant::new("invoices:read");
    assert!(g.implies(&Grant::new("invoices:read")));
    assert!(g.implies(&Grant::new("invoices:read:99")));
    assert!(!g.implies(&Grant::new("invoices:create")));
    assert!(!g.implies(&Grant::new("invoices:delete:99")));

    // Granted resource only implies all actions and instances
    let g_res = Grant::new("invoices");
    assert!(g_res.implies(&Grant::new("invoices:read")));
    assert!(g_res.implies(&Grant::new("invoices:create")));
    assert!(g_res.implies(&Grant::new("invoices:update:123")));

    // Wildcard action implies all actions
    let g_wild = Grant::new("course:*");
    assert!(g_wild.implies(&Grant::new("course:read")));
    assert!(g_wild.implies(&Grant::new("course:create")));
    assert!(g_wild.implies(&Grant::new("course:update:c-1")));

    // Specific instance grant implies only that instance
    let g_inst = Grant::new("members:delete:mem-1");
    assert!(g_inst.implies(&Grant::new("members:delete:mem-1")));
    assert!(!g_inst.implies(&Grant::new("members:delete:mem-2")));
    assert!(!g_inst.implies(&Grant::new("members:read:mem-1")));
}
