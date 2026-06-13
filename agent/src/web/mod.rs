mod ui;

use actix_cors::Cors;
use actix_web::{App, HttpResponse, HttpServer, web};

use crate::config::Config;
use crate::errors::{Result, ResultExt};
use crate::telemetry::TracingLogger;

/// Start the HTTP server and block until it shuts down.
pub async fn run(config: Config) -> Result<()> {
    let address = config.web.address.clone();

    let server = HttpServer::new(move || {
        App::new()
            .wrap(TracingLogger)
            // TODO(phase 4): restrict permissive CORS to the public `/track/*` scope.
            .wrap(Cors::permissive())
            .route("/api/v1/health", web::get().to(health))
            // SPA fallback: serve the embedded frontend, falling back to index.html.
            .default_service(web::get().to(ui::serve))
    })
    .bind(&address)
    .wrap_user_err(
        format!("Could not bind the analytics server to `{address}`."),
        &[
            "Make sure the address is correct and the port is not already in use.",
            "When binding a privileged port (<1024), ensure the process has permission.",
        ],
    )?;

    server
        .run()
        .await
        .or_system_err(&["The HTTP server stopped unexpectedly; check the logs for details."])
}

async fn health() -> HttpResponse {
    HttpResponse::Ok().json(analytics_api::Health { ok: true })
}
