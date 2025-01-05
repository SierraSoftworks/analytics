extern crate actix_web;
extern crate chrono;
#[macro_use]
extern crate serde;
extern crate serde_json;

use actix_cors::Cors;
use clap::Parser;
use tracing_batteries::{prelude::*, OpenTelemetry, Sentry, Session};

#[macro_use]
mod macros;

mod api;
mod models;
mod store;
mod telemetry;
mod utils;

use actix_web::{App, HttpServer};
use telemetry::TracingLogger;

/// Lightweight and privacy preserving analytics for your website(s).
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// The SQLite database connection string to use.
    ///
    /// For testing purposes, you can use `:memory:` to create an in-memory database. By
    /// default, we will use a file-based database at `analytics.sqldb`, however this can
    /// be overridden by setting the `DATABASE` environment variable or passing the `--database`
    /// argument.
    #[arg(short, long, default_value = "analytics.sqldb", env = "DATABASE")]
    database: String,

    /// The port to listen for incoming requests on.
    #[arg(
        short,
        long,
        default_value_t = 8000,
        env = "FUNCTIONS_CUSTOMHANDLER_PORT"
    )]
    port: u16,

    /// The name of the service which will be reported to OpenTelemetry endpoints.
    #[arg(long, env = "SERVICE_NAME", default_value = "analytics")]
    service_name: String,

    /// The Sentry DSN to use for error reporting.
    #[arg(long, env = "SENTRY_DSN")]
    sentry_dsn: Option<String>,

    /// The environment to report to Sentry.
    #[arg(long, env = "SENTRY_ENVIRONMENT")]
    sentry_environment: Option<String>,
}

fn get_listening_port() -> u16 {
    std::env::var("FUNCTIONS_CUSTOMHANDLER_PORT")
        .map(|v| v.parse().unwrap_or(8000))
        .unwrap_or(8000)
}

#[actix_rt::main]
async fn main() -> std::io::Result<()> {
    let args = Args::parse();

    let session = Session::new(args.service_name, version!("v"))
        .with_battery(Sentry::new((
            args.sentry_dsn.unwrap_or("https://298960b41aed0fef5097d936dc4fa7d6@o219072.ingest.us.sentry.io/4508581932171264".into()),
            sentry::ClientOptions {
                environment: args.sentry_environment.map(|v| v.into()),
                ..Default::default()
            },
        )))
        .with_battery(OpenTelemetry::new(""));

    let state = models::GlobalState::new(args.database).map_err(|e| {
        eprintln!("Failed to initialize database connection: {e}");
        session.record_error(&e);

        std::io::ErrorKind::Other
    })?;

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
