mod api;
mod app;
mod auth;
mod components;
mod filters;
mod format;
mod pages;

fn main() {
    console_error_panic_hook::set_once();
    wasm_logger::init(wasm_logger::Config::default());
    yew::Renderer::<app::App>::new().render();
}
