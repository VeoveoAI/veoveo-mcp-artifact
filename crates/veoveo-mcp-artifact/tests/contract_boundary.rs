//! A consumer of the `contract` feature compiles no database, runtime, HTTP or
//! object-storage crate. The test resolves the crate with default features off and
//! only `contract` on, then walks its normal and build dependencies.
use std::{
    collections::{BTreeMap, BTreeSet},
    process::Command,
};

use serde_json::Value;

/// Package names the contract feature must not reach: these families and their
/// `-`-suffixed members, every AWS crate, and core's store and fixtures.
fn forbidden(name: &str) -> bool {
    const FAMILIES: [&str; 5] = ["sqlx", "tokio", "hyper", "reqwest", "rmcp"];
    FAMILIES.iter().any(|family| {
        name == *family
            || name
                .strip_prefix(family)
                .is_some_and(|rest| rest.starts_with('-'))
    }) || name.starts_with("aws-")
        || matches!(name, "veoveo-platform-store" | "veoveo-test-fixtures")
}

#[test]
fn the_contract_feature_reaches_no_server_dependency() {
    let output = Command::new(option_env!("CARGO").unwrap_or("cargo"))
        .args([
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--no-default-features",
            "--features",
            "contract",
            "--manifest-path",
            concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"),
        ])
        .output()
        .expect("cargo metadata runs");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value = serde_json::from_slice(&output.stdout).expect("metadata is JSON");
    let names: BTreeMap<&str, &str> = metadata["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .map(|package| {
            (
                package["id"].as_str().expect("package id"),
                package["name"].as_str().expect("package name"),
            )
        })
        .collect();
    let nodes: BTreeMap<&str, &Value> = metadata["resolve"]["nodes"]
        .as_array()
        .expect("resolve nodes")
        .iter()
        .map(|node| (node["id"].as_str().expect("node id"), node))
        .collect();
    let root = metadata["resolve"]["root"]
        .as_str()
        .expect("the crate is the root");

    let mut reached = BTreeSet::new();
    let mut pending = vec![root];
    while let Some(id) = pending.pop() {
        for dependency in nodes[id]["deps"].as_array().expect("deps") {
            let normal_or_build = dependency["dep_kinds"]
                .as_array()
                .expect("dep kinds")
                .iter()
                .any(|kind| kind["kind"].is_null() || kind["kind"] == "build");
            let package = dependency["pkg"].as_str().expect("dependency id");
            if normal_or_build && reached.insert(package) {
                pending.push(package);
            }
        }
    }
    let reached_names: BTreeSet<&str> = reached.iter().map(|id| names[id]).collect();
    assert!(reached_names.contains("veoveo-types"), "{reached_names:?}");
    let found: Vec<&str> = reached_names
        .into_iter()
        .filter(|name| forbidden(name))
        .collect();
    assert!(
        found.is_empty(),
        "the contract feature reaches server dependencies: {found:?}"
    );
}

#[test]
fn the_boundary_names_server_dependencies() {
    for name in [
        "sqlx",
        "sqlx-postgres",
        "tokio",
        "aws-sdk-s3",
        "aws-smithy-runtime",
        "veoveo-platform-store",
    ] {
        assert!(forbidden(name), "{name}");
    }
    for name in ["serde", "veoveo-types", "tokio2", "schemars"] {
        assert!(!forbidden(name), "{name}");
    }
}
