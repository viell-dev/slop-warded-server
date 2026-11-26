---
description: "Start the website template, edit HTML, add a page, and extend the contact form."
---

# Website template

The template starts with a small site whose HTML is rendered on the server.
The names used below are in the [glossary](./glossary.md).

## Start

See [what you need](./getting-started.md#what-you-need) for Rust, a C compiler,
`cargo-generate` and the shell to run the commands in.

```sh
cargo generate viell-dev/slop-warded-server templates/website --tag 0.1.0-alpha.3 --name my-site
cd my-site
cargo run
```

Open `http://127.0.0.1:3000/`. You should see a Welcome heading and links to Home,
About and Contact. The project's `.cargo/config.toml` sets `WARDED_DEV_ADDR` for this
[development address](./configuration.md#the-development-address).

To rebuild and restart when you save a file, stop `cargo run` and start
[bacon](https://dystroy.org/bacon/) instead. Quit bacon with `q`.

```sh
cargo install --locked bacon
bacon run-long
```

The project's `bacon.toml` watches `templates/` and `static/` as well as Rust files.
Leave the page open. It reloads when the server starts again. If it stays on the old page, see
[Troubleshooting](./troubleshooting.md#the-page-does-not-reload).

## What is where

| To change                                 | Open                                          |
| ----------------------------------------- | --------------------------------------------- |
| Text on the home page                     | `templates/home.html`                         |
| The shared menu and page frame            | `templates/base.html`                         |
| The values a page shows                   | `src/pages.rs`                                |
| The contact form and what it accepts      | `templates/contact.html` and `src/contact.rs` |
| Which paths answer and how they're tested | `src/routes.rs` and `src/routes/tests.rs`     |
| Styles, icons and images                  | `static/`                                     |
| How the server starts                     | `src/main.rs`                                 |
| The watched directories or local address  | `bacon.toml` or `.cargo/config.toml`          |

## First change

In `templates/home.html`, replace the text in the first `<p>` with
`This is my first site.` and save. HTML templates are compiled into the program, so stop and
restart plain `cargo run` to see the change. `bacon run-long` does that by itself.

<<< ../templates/website/templates/home.html

## Add a page or endpoint

Make a News page using About as a starting point. [Askama](https://askama.rs/en/stable/)
fills in the HTML template from the fields of a Rust struct.

1. Copy `templates/about.html` to `templates/news.html`. Change its paragraph to
   `Our first news item.` and keep the line that extends `base.html` and the content block.
2. In `src/pages.rs`, copy the `About` struct (including its two attributes) and the
   `about` function below it. In the copies, rename the type to `News` in both places,
   change the HTML template path to `news.html`, rename the function to `news`, and set
   `title` to `"News"` and `current` to `"/news"`.
3. In `src/routes.rs`, add `.route(page("/news"), pages::news.into_service())` after the
   route for `/about`. The `page` helper accepts GET (read the page) and HEAD (get its headers).
4. In `templates/base.html`, copy the About menu item. In the copy, replace both occurrences
   of `/about` with `/news` and the link text with `News`.

<<< ../templates/website/src/pages.rs

Use the News link from Home. At `/news` you should see the News heading and your paragraph.
The menu marks News as the current page using the `current` value you set.

## Add data or a form

Give the contact form an optional subject. The template shows form errors itself.
[Forms](./website.md#forms) explains what happens on an invalid or successful submission.

1. In `src/contact.rs`, add `subject: String,` to `Message`. It already defaults missing fields
   to empty strings.
2. In `templates/contact.html`, put this field inside `<form>`, before the Message field. The
   `name` matches the Rust field, and `value` keeps the entry if the server shows the form again.

```html
<p class="field">
  <label for="subject">Subject (optional)</label>
  <input id="subject" name="subject" value="{{ message.subject }}">
</p>
```

<<< ../templates/website/templates/contact.html

Open Contact, enter a subject and leave the other fields blank. After you select Send, the
form should keep the subject and show the problems with the required fields. Fill in those
fields to reach the Thank you page. The example logs that a message arrived. To keep or send it,
add your own handling in `send` in `src/contact.rs`.

## Test and put it online

Run `cargo test` to check the template's pages, form and static files.
[Testing](./testing.md) shows how to add checks for your changes.

Run `cargo build --release` and copy the built program and `static/` for deployment.
Start the program from the directory that contains `static/`. Run this alpha on your own computer
or a private network whose users you trust. Keep it off public sites and read the
[limits](./limits.md) before deploying.
[Deployment](./deployment.md) covers starting the binary and setting its addresses.
[Certificates](./certificates.md) explains how to get and configure a trusted certificate.
For a Linux server, build on Linux or use the [Dockerfile](./deployment.md#containers).
A Windows executable does not run on Linux.

## Learn more

- How do I add languages or cache stylesheets? [A website](./website.md)
- How do I share HTML between pages? [Askama](https://askama.rs/en/stable/)
- How do labels, fields and validation work? [MDN's web forms guide](https://developer.mozilla.org/en-US/docs/Learn_web_development/Extensions/Forms)
- How can I match a request by more than its path? [Warded's guides](https://viell-dev.github.io/slop-warded-router/)
