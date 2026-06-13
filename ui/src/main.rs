use yew::prelude::*;

#[function_component]
fn App() -> Html {
    html! {
        <main>
            <h1>{ "Analytics" }</h1>
            <p class="muted">{ "Dashboard coming soon." }</p>
        </main>
    }
}

fn main() {
    console_error_panic_hook::set_once();
    wasm_logger::init(wasm_logger::Config::default());
    yew::Renderer::<App>::new().render();
}
