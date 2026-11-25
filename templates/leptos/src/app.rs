use leptos::prelude::*;
use leptos_meta::{MetaTags, Stylesheet, Title, provide_meta_context};
use leptos_router::{
    components::{A, Route, Router, Routes},
    path,
};

/// The HTML document around every page.
pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8" />
                <meta name="viewport" content="width=device-width, initial-scale=1" />
                <AutoReload options=options.clone() />
                <HydrationScripts options />
                <MetaTags />
            </head>
            <body>
                <App />
            </body>
        </html>
    }
}

/// Which page belongs to which address.
#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    view! {
        <Stylesheet id="leptos" href="/pkg/{{project-name}}.css" />
        <Router>
            <header>
                <nav>
                    <A href="/">"Home"</A>
                    <A href="/server">"Server"</A>
                </nav>
            </header>
            <main>
                <Routes fallback=NotFound>
                    <Route path=path!("/") view=Home />
                    <Route path=path!("/server") view=FromServer />
                </Routes>
            </main>
        </Router>
    }
}

#[component]
fn Home() -> impl IntoView {
    let count = RwSignal::new(0);
    view! {
        <Title text="Welcome" />
        <h1>"It works"</h1>
        <p>"Edit " <code>"src/app.rs"</code> " and save to see this page change."</p>
        <button on:click=move |_| *count.write() += 1>"Count: " {count}</button>
        <ul class="cards">
            <li>
                <a href="https://book.leptos.dev">"Leptos book"</a>
                "Components, signals and forms"
            </li>
            <li>
                <a href="https://viell-dev.github.io/slop-warded-server/">"Warded Server"</a>
                "Certificates, settings and deployment"
            </li>
        </ul>
    }
}

/// A function that always runs on the server, where it could ask a database.
#[server]
pub async fn greet(name: String) -> Result<String, ServerFnError> {
    Ok(format!("Hello {name}, from the server"))
}

#[component]
fn FromServer() -> impl IntoView {
    let greet = ServerAction::<Greet>::new();
    view! {
        <Title text="Server" />
        <h1>"Ask the server"</h1>
        <ActionForm action=greet>
            <input type="text" name="name" value="you" aria-label="Name" />
            <button type="submit">"Greet"</button>
        </ActionForm>
        <p>{move || greet.value().get().and_then(Result::ok)}</p>
    }
}

#[component]
fn NotFound() -> impl IntoView {
    view! {
        <Title text="Page not found" />
        <h1>"Page not found"</h1>
        <A href="/">"Go to the start page"</A>
    }
}
