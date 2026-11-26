mod notes;
mod problem;
mod routes;

use notes::Notes;
use warded_server::Server;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    warded_server::init_logging();

    let app = routes::app(Notes::default())?;
    Server::from_env()?.serve(app).await?;
    Ok(())
}
