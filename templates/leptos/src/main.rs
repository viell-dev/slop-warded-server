#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use leptos::prelude::*;
    use leptos_axum::{LeptosRoutes, generate_route_list};
    use tracing_subscriber::EnvFilter;
    use warded_server::Server;
    use {{crate_name}}::app::{App, shell};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let options = get_configuration(None)?.leptos_options;
    let app = axum::Router::new()
        .leptos_routes(&options, generate_route_list(App), {
            let options = options.clone();
            move || shell(options.clone())
        })
        .fallback(leptos_axum::file_and_error_handler(shell))
        .with_state(options.clone());

    // Warded routes that come before the site's pages go here.
    let routes = warded_server::warded::Router::new().fallback(app);
    let mut server = Server::from_env()?;
    if cfg!(debug_assertions) {
        // Without HTTPS, so that no certificate is in the way and live reload works.
        server = server.serve_http(options.site_addr);
        println!("\n  Open http://localhost:{}\n", options.site_addr.port());
    }
    server.serve(warded_server::app(routes)).await?;
    Ok(())
}

/// The browser starts at `hydrate` in `lib.rs` instead.
#[cfg(not(feature = "ssr"))]
fn main() {}
