use gloo_net::http::Request;
use serde::Deserialize;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

#[derive(Deserialize)]
struct Greeting {
    text: String,
}

#[component]
fn App() -> Html {
    let count = use_state(|| 0);
    let answer = use_state(String::new);

    let add = {
        let count = count.clone();
        Callback::from(move |_| count.set(*count + 1))
    };
    let greet = {
        let answer = answer.clone();
        Callback::from(move |_| {
            let answer = answer.clone();
            spawn_local(async move {
                let response = Request::get("/api/greeting/you").send().await;
                answer.set(match response {
                    Ok(response) => match response.json::<Greeting>().await {
                        Ok(greeting) => greeting.text,
                        Err(error) => error.to_string(),
                    },
                    Err(error) => error.to_string(),
                });
            });
        })
    };

    html! {
        <main>
            <h1>{ "It works" }</h1>
            <p>{ "Edit " }<code>{ "web/src/main.rs" }</code>{ " and save to see this page change." }</p>
            <button onclick={add}>{ "Count: " }{ *count }</button>
            <p><button onclick={greet}>{ "Greet" }</button>{ " " }{ &*answer }</p>
        </main>
    }
}

fn main() {
    yew::Renderer::<App>::new().render();
}
