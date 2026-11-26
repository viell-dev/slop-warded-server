---
description: "A short glossary for the Warded Server templates."
---

# Glossary

- **Route**. A rule that selects what answers a request, for example `GET /notes` selects the
  function that lists the notes.
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

[Routes and requests](./requests.md) shows how these parts fit together. See
[Middleware](./middleware.md) for examples of layers.
