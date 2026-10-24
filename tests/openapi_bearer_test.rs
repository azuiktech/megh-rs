//! The API description says a request is signed in by the `jwt_token` cookie or by a bearer token.

use serde_json::Value;

fn spec() -> Value {
    serde_json::to_value(megh::booking::router().merge(megh::billing::router()).into_openapi()).unwrap()
}

#[test]
fn the_bearer_scheme_is_described() {
    let scheme = &spec()["components"]["securitySchemes"]["bearer"];

    assert_eq!((scheme["type"].as_str(), scheme["scheme"].as_str(), scheme["bearerFormat"].as_str()), (Some("http"), Some("bearer"), Some("JWT")));
}

#[test]
fn every_operation_accepts_the_cookie_or_the_bearer_token() {
    let spec = spec();
    let operations: Vec<&Value> = spec["paths"].as_object().unwrap().values().flat_map(|item| item.as_object().unwrap().values()).collect();

    assert!(!operations.is_empty());
    for operation in operations {
        let alternatives: Vec<Vec<&str>> = operation["security"].as_array().unwrap().iter().map(|requirement| requirement.as_object().unwrap().keys().map(String::as_str).collect()).collect();
        assert_eq!(alternatives, [vec!["session"], vec!["bearer"]], "{operation}");
    }
}
