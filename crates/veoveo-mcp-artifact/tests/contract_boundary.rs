//! A consumer of the `contract` feature compiles no database, runtime, HTTP or
//! object-storage crate. The test resolves the crate with default features off and
//! only `contract` on, over its normal and build dependencies.
use std::{collections::BTreeSet, process::Command};

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
    // `cargo tree` without dev edges resolves features as a consumer's build does; the
    // crate's own dev-dependencies, which enable the SDK's server, do not join in.
    let output = Command::new(option_env!("CARGO").unwrap_or("cargo"))
        .args([
            "tree",
            "--locked",
            "--package",
            "veoveo-mcp-artifact",
            "--no-default-features",
            "--features",
            "contract",
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
    let tree = String::from_utf8(output.stdout).expect("cargo tree prints UTF-8");
    let reached: BTreeSet<&str> = tree
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert!(reached.contains("veoveo-types"), "{reached:?}");
    let found: Vec<&str> = reached.into_iter().filter(|name| forbidden(name)).collect();
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
