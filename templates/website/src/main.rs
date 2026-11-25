mod pages;
mod routes;

use tracing_subscriber::EnvFilter;
use warded_server::Server;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tower_http=debug".into());
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let app = routes::app()?;
    Server::from_env()?.serve(app).await?;
    Ok(())
}
