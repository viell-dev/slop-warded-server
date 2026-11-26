---
description: "A short glossary for the Warded Server templates."
---

# Glossary

## In the application

- **Axum**. A Rust web library that provides handlers, extractors and response types.
- **Tower**. A Rust library for building and composing services and middleware.
- **Endpoint**. A method and path an application answers, such as `GET /notes`.
- **Condition**. A test a request must pass to select a route. Read
  `get().and(exact_path("/notes"))` as “the method is GET and the path is exactly `/notes`.”
- **Route**. A rule that selects what answers a request, for example `GET /notes` selects the
  function that lists the notes.
- **Fallback**. A handler or service that answers when no route matches. It can show a 404 page.
- **Handler**. A function that runs for a request and returns a response, such as text, a page
  or JSON. A route can also send the request to a service, such as a file server.
- **Service**. Something that takes a request and returns a response. In Tower, both a whole
  application and a handler adapted with `into_service()` can be services.
- **Extractor**. A handler argument that Axum fills in before calling the function, for example
  data read from a form, JSON or part of the URL. It can reject invalid input.
- **State**. Data or a resource the application passes to handlers. The API template shares
  its notes across requests this way. Axum's `State` argument gives a handler access to them.
- **Middleware**. Code that runs around a service. It can log a request, reject it early or
  change the response. A **layer** is how Tower attaches middleware to a service.
- **Template**. A project template gives you starter files. An Askama **HTML template** is a file
  that Rust fills with values to make a page.

[Routes and requests](./requests.md) shows how these parts fit together. See
[Middleware](./middleware.md) for examples of layers.

## At the address

- **Loopback**. An address that reaches only this computer, such as `127.0.0.1` or `::1`.
- **Development address**. The template's plain HTTP listener at `http://127.0.0.1:3000/`.
  Cargo sets it through `WARDED_DEV_ADDR`. See [Configuration](./configuration.md#the-development-address).
- **TLS**. The protocol that encrypts a connection and lets the client check the server's identity.
- **Certificate**. What the server presents over HTTPS to prove its identity. **PEM** is a text
  format for storing certificates and keys. See [Certificates](./certificates.md).
- **HTTP/3**. The version of HTTP that runs over **QUIC**, a transport that uses UDP and includes
  TLS. See [HTTP/3](./http3.md) for how browsers use it.

## In a response

- **303 See Other**. The browser should open another address with GET, for example after
  sending a form.
- **400 Bad Request**. The server could not read the request, for example because a path value
  was not a number.
- **404 Not Found**. The requested page or resource was not found.
- **422 Unprocessable Content**. The server read the input but could not use it, for example
  because a required field was empty.
- **500 Internal Server Error**. Something failed while the server handled the request.
