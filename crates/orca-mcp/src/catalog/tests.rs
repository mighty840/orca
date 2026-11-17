use std::collections::BTreeMap;

use super::*;

fn args(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn paths_fill_the_route_and_put_the_rest_in_the_query() {
    let logs = op("get_logs").unwrap();
    assert_eq!(
        logs.path(&args(&[("name", "web"), ("tail", "50")]))
            .unwrap(),
        "/api/v1/services/web/logs?follow=false&tail=50"
    );
    assert_eq!(
        logs.path(&args(&[("name", "web"), ("tail", "999999")]))
            .unwrap(),
        "/api/v1/services/web/logs?follow=false&tail=2000",
        "capped"
    );
    assert_eq!(
        op("get_status")
            .unwrap()
            .path(&args(&[("project", "a b")]))
            .unwrap(),
        "/api/v1/status?project=a%20b"
    );
    assert_eq!(
        op("get_status").unwrap().path(&args(&[])).unwrap(),
        "/api/v1/status"
    );
}

#[test]
fn a_name_cannot_escape_its_path_segment() {
    let p = op("get_service")
        .unwrap()
        .path(&args(&[("name", "../secrets")]))
        .unwrap();
    assert_eq!(p, "/api/v1/services/..%2Fsecrets");
    let err = op("get_service").unwrap().path(&args(&[])).unwrap_err();
    assert!(err.contains("name"), "{err}");
}

#[test]
fn resource_uris_resolve_to_operations() {
    let (op, a) = match_resource("orca://services/web/logs?tail=10").unwrap();
    assert_eq!(
        (op.tool, a["name"].as_str(), a["tail"].as_str()),
        ("get_logs", "web", "10")
    );
    let (op, _) = match_resource("orca://services").unwrap();
    assert_eq!(op.tool, "get_status");
    assert_eq!(
        match_resource("orca://services/web").unwrap().0.tool,
        "get_service"
    );
    assert!(match_resource("orca://nope").is_none());
    assert!(match_resource("orca://services//logs").is_none());
}

#[test]
fn tools_and_resources_are_unique() {
    let mut tools: Vec<_> = OPS.iter().map(|o| o.tool).collect();
    let mut res: Vec<_> = OPS.iter().map(|o| o.resource).collect();
    tools.sort();
    tools.dedup();
    res.sort();
    res.dedup();
    assert_eq!((tools.len(), res.len()), (OPS.len(), OPS.len()));
}
