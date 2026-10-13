//! A consumer of the `contract` feature compiles no database, runtime, HTTP or
//! object-storage crate, and a consumer of the `client` feature compiles no database,
//! object-storage SDK or server stack. Each test resolves the crate with default
//! features off and one feature on, over its normal and build dependencies.
use std::{collections::BTreeSet, process::Command};

/// Package names the contract feature must not reach: these families and their
/// `-`-suffixed members, every AWS crate, and core's store and fixtures.
fn forbidden(name: &str) -> bool {
    const FAMILIES: [&str; 5] = ["sqlx", "tokio", "hyper", "reqwest", "rmcp"];
    in_family(name, &FAMILIES) || server_only(name)
}

/// Package names the client feature must not reach: the database, the object-storage
/// SDK, core's store, Task runtime, audit writer and fixtures, and the server's HTTP and
/// token stacks.
fn forbidden_to_clients(name: &str) -> bool {
    const FAMILIES: [&str; 4] = ["sqlx", "axum", "jsonwebtoken", "opentelemetry"];
    in_family(name, &FAMILIES)
        || server_only(name)
        || matches!(name, "veoveo-task-runtime" | "veoveo-audit")
}

fn in_family(name: &str, families: &[&str]) -> bool {
    families.iter().any(|family| {
        name == *family
            || name
                .strip_prefix(family)
                .is_some_and(|rest| rest.starts_with('-') || rest.starts_with('_'))
    })
}

fn server_only(name: &str) -> bool {
    name.starts_with("aws-") || matches!(name, "veoveo-platform-store" | "veoveo-test-fixtures")
}

/// The packages the crate reaches with only `feature` on, as a consumer's build
/// resolves them. The crate's own dev-dependencies, which enable the SDK's server, do
/// not join in.
fn reached(feature: &str) -> BTreeSet<String> {
    let output = Command::new(option_env!("CARGO").unwrap_or("cargo"))
        .args([
            "tree",
            "--locked",
            "--package",
            "veoveo-mcp-artifact",
            "--no-default-features",
            "--features",
            feature,
            "--edges",
            "normal,build",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo tree runs");
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("cargo tree prints UTF-8")
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_contract_feature_reaches_no_server_dependency() {
    let reached = reached("contract");
    assert!(reached.contains("veoveo-types"), "{reached:?}");
    let found: Vec<&String> = reached.iter().filter(|name| forbidden(name)).collect();
    assert!(
        found.is_empty(),
        "the contract feature reaches server dependencies: {found:?}"
    );
}

#[test]
fn the_client_feature_reaches_no_database_storage_sdk_or_server_stack() {
    let reached = reached("client");
    for expected in ["veoveo-sdk-rust", "reqwest", "tokio"] {
        assert!(reached.contains(expected), "{expected} in {reached:?}");
    }
    let found: Vec<&String> = reached
        .iter()
        .filter(|name| forbidden_to_clients(name))
        .collect();
    assert!(
        found.is_empty(),
        "the client feature reaches server dependencies: {found:?}"
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
    for name in [
        "sqlx-postgres",
        "aws-sdk-s3",
        "axum",
        "veoveo-task-runtime",
        "veoveo-audit",
        "opentelemetry_sdk",
    ] {
        assert!(forbidden_to_clients(name), "{name}");
    }
    for name in [
        "reqwest",
        "tokio",
        "veoveo-audit-contract",
        "veoveo-sdk-rust",
    ] {
        assert!(!forbidden_to_clients(name), "{name}");
    }
}
