#[tokio::main]
async fn main() -> anyhow::Result<()> {
    veoveo_mcp_artifact::server::run().await
}
