//! The pages, from the Askama templates in `templates/`.

use askama::Template;
use axum::{
    http::{
        StatusCode,
        header::{CONTENT_LANGUAGE, VARY},
    },
    response::{Html, IntoResponse, Response},
};

#[derive(Template)]
#[template(path = "home.html")]
struct Home {
    language: &'static str,
    title: &'static str,
    text: &'static str,
}

#[derive(Template)]
#[template(path = "not-found.html")]
struct NotFound {
    language: &'static str,
    title: &'static str,
}

pub async fn home() -> Response {
    let page = Home {
        language: "en",
        title: "Welcome",
        text: "This page is served over HTTP/1.1, HTTP/2 and HTTP/3.",
    };
    render(StatusCode::OK, page.language, &page)
}

pub async fn home_in_swedish() -> Response {
    let page = Home {
        language: "sv",
        title: "Välkommen",
        text: "Den här sidan skickas över HTTP/1.1, HTTP/2 och HTTP/3.",
    };
    render(StatusCode::OK, page.language, &page)
}

pub async fn not_found() -> Response {
    let page = NotFound {
        language: "en",
        title: "Page not found",
    };
    render(StatusCode::NOT_FOUND, page.language, &page)
}

fn render(status: StatusCode, language: &'static str, page: &impl Template) -> Response {
    match page.render() {
        Ok(html) => {
            let headers = [
                (CONTENT_LANGUAGE, language),
                // Tells caches to keep the languages apart.
                (VARY, "accept-language"),
            ];
            (status, headers, Html(html)).into_response()
        }
        Err(error) => {
            tracing::error!(%error, "could not render a page");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
