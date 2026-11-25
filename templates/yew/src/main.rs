mod api;
mod spa;

use tower_http::compression::CompressionLayer;
use tracing_subscriber::EnvFilter;
use warded_server::Server;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let app = spa::app(api::routes(), "web/dist".as_ref())?;
    let mut server = Server::from_env()?;
    if cfg!(debug_assertions) {
        // Trunk's development server passes requests for the API on to this address.
        server = server.serve_http(([127, 0, 0, 1], 3000).into());
    }
    server.serve(app.layer(CompressionLayer::new())).await?;
    Ok(())
}
