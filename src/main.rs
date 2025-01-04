extern crate actix_web;
extern crate chrono;
#[macro_use]
extern crate serde;
extern crate rand;
extern crate serde_json;
extern crate uuid;

use actix_cors::Cors;
use tracing_batteries::{OpenTelemetry, Sentry, Session, prelude::*};

#[macro_use]
mod macros;

mod api;
mod models;
mod store;
mod telemetry;
mod utils;

use actix_web::{App, HttpServer};
use telemetry::TracingLogger;

fn get_listening_port() -> u16 {
    std::env::var("FUNCTIONS_CUSTOMHANDLER_PORT")
        .map(|v| v.parse().unwrap_or(8000))
        .unwrap_or(8000)
}

#[actix_rt::main]
async fn main() -> std::io::Result<()> {
    let session = Session::new("analytics", version!("v"))
        .with_battery(Sentry::new(
            "https://298960b41aed0fef5097d936dc4fa7d6@o219072.ingest.us.sentry.io/4508581932171264",
        ))
        .with_battery(OpenTelemetry::new(""));

    let state = models::GlobalState::new();

    info!("Starting server on :{}", get_listening_port());
    let result = HttpServer::new(move || {
        App::new()
            .app_data(actix_web::web::Data::new(state.clone()))
            .wrap(TracingLogger)
            .wrap(Cors::default().allow_any_origin().send_wildcard())
            .configure(api::configure)
    })
    .bind(format!("0.0.0.0:{}", get_listening_port()))?
    .run()
    .await
    .map_err(|err| {
        error!("The server exited unexpectedly: {}", err);
        sentry::capture_event(sentry::protocol::Event {
            message: Some(format!("Server Exited Unexpectedly: {}", err)),
            level: sentry::protocol::Level::Fatal,
            ..Default::default()
        });

        err
    });

    session.shutdown();
    result
}
