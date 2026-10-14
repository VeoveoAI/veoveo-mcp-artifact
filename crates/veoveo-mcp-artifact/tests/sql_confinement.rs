//! Artifact's SQL names only the `artifact` schema: its queries, migrations and test
//! queries. Core state, such as Tasks, process instances, usage and audit, reaches
//! Artifact only through the SDK and core's runtime API.

#[test]
fn sql_names_only_the_artifact_schema() {
    veoveo_sdk_rust::testing::assert_sql_confined(env!("CARGO_MANIFEST_DIR"), "artifact");
}
