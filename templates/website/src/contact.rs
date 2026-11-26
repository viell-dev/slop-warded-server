//! A form that the server checks: shown on GET, sent with POST, and followed by a redirect.

use crate::pages::render;
use askama::Template;
use axum::{
    Form,
    extract::rejection::FormRejection,
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;

/// What the visitor filled in. A missing field is empty.
#[derive(Default, Deserialize)]
#[serde(default)]
pub struct Message {
    name: String,
    email: String,
    text: String,
}

/// What is wrong with each field, if anything.
#[derive(Default)]
struct Problems {
    name: Option<&'static str>,
    email: Option<&'static str>,
    text: Option<&'static str>,
}

impl Problems {
    fn of(message: &Message) -> Self {
        let (name, email, text) = (
            message.name.trim(),
            message.email.trim(),
            message.text.trim(),
        );
        let address = email.split_once('@');
        let valid_email = !email.contains(char::is_whitespace)
            && address.is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'));
        Self {
            name: if name.is_empty() {
                Some("Enter your name.")
            } else if name.chars().count() > 100 {
                Some("Enter at most 100 characters.")
            } else {
                None
            },
            email: (!valid_email).then_some("Enter an e-mail address, such as name@example.com."),
            text: if text.is_empty() {
                Some("Enter a message.")
            } else if text.chars().count() > 2000 {
                Some("Enter at most 2000 characters.")
            } else {
                None
            },
        }
    }

    fn any(&self) -> bool {
        self.name.is_some() || self.email.is_some() || self.text.is_some()
    }
}

#[derive(Template)]
#[template(path = "contact.html")]
struct Contact {
    title: &'static str,
    current: &'static str,
    message: Message,
    problems: Problems,
}

impl Contact {
    fn new(message: Message, problems: Problems) -> Self {
        Self {
            title: "Contact",
            current: "/contact",
            message,
            problems,
        }
    }
}

pub async fn form() -> Response {
    let page = Contact::new(Message::default(), Problems::default());
    render(StatusCode::OK, &page)
}

pub async fn send(input: Result<Form<Message>, FormRejection>) -> Response {
    // A body that is not a form counts as an empty one.
    let message = input.map(|Form(message)| message).unwrap_or_default();
    let problems = Problems::of(&message);
    if problems.any() {
        // The form again, with what the visitor entered and what to correct.
        let page = Contact::new(message, problems);
        return render(StatusCode::UNPROCESSABLE_ENTITY, &page);
    }
    // Store the message or send it on here.
    tracing::info!("a message came through the contact form");
    // A redirect after success, so that reloading the next page does not send it again.
    Redirect::to("/contact/sent").into_response()
}

#[derive(Template)]
#[template(path = "sent.html")]
struct Sent {
    title: &'static str,
    current: &'static str,
}

pub async fn sent() -> Response {
    let page = Sent {
        title: "Thank you",
        current: "/contact",
    };
    render(StatusCode::OK, &page)
}
