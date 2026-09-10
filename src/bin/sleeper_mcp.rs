use rmcp::ServiceExt;
use sleeper::{
    analysis::AnalysisService,
    client::SleeperClient,
    config::Config,
    db::StatsDb,
    mcp::SleeperMcpServer,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // MCP uses stdio for transport — suppress all tracing output so it doesn't
    // corrupt the JSON-RPC stream. Use SLEEPER_LOG=warn to enable logging to stderr.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_env("SLEEPER_LOG")
                .add_directive(tracing::Level::ERROR.into()),
        )
        .with_target(false)
        .init();

    let config = Config::default();
    let client = SleeperClient::new(&config).await?;
    let db = StatsDb::open(&config.stats_db_path()).await?;
    let svc = AnalysisService::new(client, db);
    let server = SleeperMcpServer::new(svc);

    let service = server.serve(rmcp::transport::io::stdio()).await?;
    service.waiting().await?;
    Ok(())
}
