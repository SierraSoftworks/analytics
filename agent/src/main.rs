#[macro_use]
mod macros;

mod analytics;
mod config;
mod errors;
mod ingest;
mod ratelimit;
mod state;
// Some store CRUD (pixels/triage) and analytics helpers are consumed in Phases 6-7;
// allow the forward-looking surface until then.
#[allow(dead_code, unused_imports)]
mod store;
mod telemetry;
mod web;

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use tracing_batteries::{Analytics, OpenTelemetry, Sentry, Session, prelude::*};

use crate::config::Config;
use crate::errors::ResultExt;
use crate::ratelimit::RateLimiter;
use crate::state::AppState;
use crate::store::Store;

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
    let _ = dotenvy::from_path(&args.env);

    let config = match Config::load(&args.config) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("{}", human_errors::pretty(&err));
            return ExitCode::FAILURE;
        }
    };

    let session = build_telemetry(&config);

    let outcome = serve(config).await;
    let code = match &outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            error!("The server exited unexpectedly: {err}");
            eprintln!("{}", human_errors::pretty(&err));
            ExitCode::FAILURE
        }
    };

    session.shutdown();
    code
}

/// Open storage, start the ingest pipeline, and run the web server.
async fn serve(config: Config) -> errors::Result<()> {
    let store = Arc::new(Store::open(&config.storage.redb_path)?);
    let ingest = ingest::spawn(store.clone(), config.storage.clone());

    // Parse the ACL once at startup (config load already validated its syntax).
    let acl = Arc::new(config.web.admin.acl_filter()?);
    let http = reqwest::Client::builder()
        .build()
        .or_system_err(&["Failed to initialise the HTTP client used for OIDC."])?;

    let tracking_limiter = Arc::new(RateLimiter::from_rule(&config.ratelimit.tracking));
    let unauth_limiter = Arc::new(RateLimiter::from_rule(&config.ratelimit.unauthenticated));
    spawn_limiter_cleanup(tracking_limiter.clone(), unauth_limiter.clone());

    let state = AppState {
        store,
        ingest,
        config: Arc::new(config),
        http,
        oidc_cache: Arc::new(web::helpers::oidc::OidcCache::default()),
        acl,
        tracking_limiter,
        unauth_limiter,
    };

    info!("Starting analytics server on {}", state.config.web.address);
    web::run(state).await
}

/// Periodically reclaim memory from idle rate-limit buckets.
fn spawn_limiter_cleanup(tracking: Arc<RateLimiter>, unauth: Arc<RateLimiter>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(120));
        loop {
            tick.tick().await;
            tracking.cleanup();
            unauth.cleanup();
        }
    });
}

fn build_telemetry(config: &Config) -> Session {
    let service_name = config.telemetry.service_name.clone();
    let environment = config.telemetry.environment.clone();
    let sentry_dsn = config.telemetry.sentry_dsn.clone();
    // `OpenTelemetry::new` requires a `&'static str`; leak the configured endpoint
    // (a one-time startup allocation) when one is provided.
    let otlp_endpoint: &'static str = match config.telemetry.otlp_endpoint.as_deref() {
        Some(endpoint) if !endpoint.is_empty() => Box::leak(endpoint.to_owned().into_boxed_str()),
        _ => "",
    };

    // `Session::new` returns a builder; the first `with_battery` transitions it into
    // the live `Session`, so add the unconditional battery first.
    let mut session = Session::new(service_name, version!("v"))
        .with_battery(OpenTelemetry::new(otlp_endpoint))
        .with_battery(Analytics::new("https://analytics.sierrasoftworks.com"));
    if let Some(dsn) = sentry_dsn {
        session = session.with_battery(Sentry::new((
            dsn,
            sentry::ClientOptions {
                environment: environment.map(Into::into),
                ..Default::default()
            },
        )));
    }
    session
}
