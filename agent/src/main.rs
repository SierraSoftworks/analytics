#[macro_use]
mod macros;

mod config;
mod errors;
// The store's public surface (re-exports + methods) is consumed from Phase 4
// (ingest) onward; allow the forward-looking dead code until then.
#[allow(dead_code, unused_imports)]
mod store;
mod telemetry;
mod web;

use std::process::ExitCode;

use clap::Parser;
use tracing_batteries::{OpenTelemetry, Sentry, Session, prelude::*};

use crate::config::Config;

/// Lightweight, privacy-preserving analytics for your websites and applications.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Path to the YAML configuration file.
    #[arg(short, long, default_value = "config.yaml", env = "ANALYTICS_CONFIG")]
    config: String,

    /// Path to an environment file to load (if it exists).
    #[arg(short, long, default_value = ".env")]
    env: String,
}

#[actix_web::main]
async fn main() -> ExitCode {
    let args = Args::parse();

    // Load environment variables from the .env file if present (used by config
    // interpolation). A missing file is not an error.
    let _ = dotenvy::from_path(&args.env);

    let config = match Config::load(&args.config) {
        Ok(config) => config,
        Err(err) => {
            // Display includes the actionable advice.
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    };

    // Pull telemetry settings out of `config` so it can be moved into the server.
    // `OpenTelemetry::new` requires a `&'static str`; leak the configured endpoint
    // (a one-time startup allocation) when one is provided.
    let service_name = config.telemetry.service_name.clone();
    let sentry_dsn = config.telemetry.sentry_dsn.clone();
    let environment = config.telemetry.environment.clone();
    let otlp_endpoint: &'static str = match config.telemetry.otlp_endpoint.as_deref() {
        Some(endpoint) if !endpoint.is_empty() => Box::leak(endpoint.to_owned().into_boxed_str()),
        _ => "",
    };

    // `Session::new` returns a builder; the first `with_battery` transitions it into
    // the live `Session`, so add the unconditional battery first.
    let mut session =
        Session::new(service_name, version!("v")).with_battery(OpenTelemetry::new(otlp_endpoint));
    if let Some(dsn) = sentry_dsn {
        session = session.with_battery(Sentry::new((
            dsn,
            sentry::ClientOptions {
                environment: environment.map(Into::into),
                ..Default::default()
            },
        )));
    }

    info!("Starting analytics server on {}", config.web.address);
    let outcome = web::run(config).await;
    let code = match &outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            error!("The server exited unexpectedly: {err}");
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    };

    session.shutdown();
    code
}
