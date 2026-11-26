use axum::routing::any;
use warded_server::{
    Server,
    warded::{
        Matcher, Router,
        matchers::{exact_path, get},
    },
};

async fn home() -> &'static str {
    "Hello\n"
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    warded_server::init_logging();

    // Routes are tried in order, and the first whose conditions all hold gets the request.
    let routes = Router::new().route(get().and(exact_path("/")), any(home));

    Server::from_env()?
        .serve(warded_server::app(routes))
        .await?;
    Ok(())
}
