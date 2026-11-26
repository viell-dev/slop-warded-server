mod contact;
mod pages;
mod routes;

use warded_server::Server;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    warded_server::init_logging();

    let app = routes::app()?;
    Server::from_env()?.serve(app).await?;
    Ok(())
}
