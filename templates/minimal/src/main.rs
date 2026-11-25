use axum::routing::any;
use tracing_subscriber::EnvFilter;
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
async fn main() -> Result<(), warded_server::Error> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // Routes are tried in order, and the first whose conditions all hold gets the request.
    let routes = Router::new().route(get().and(exact_path("/")), any(home));

    Server::from_env()?.serve(warded_server::app(routes)).await
}
