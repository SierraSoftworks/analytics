mod ui;

use actix_cors::Cors;
use actix_web::{App, HttpResponse, HttpServer, web};

use crate::config::Config;
use crate::telemetry::TracingLogger;

/// Start the HTTP server and block until it shuts down.
pub async fn run(config: Config) -> std::io::Result<()> {
    let address = config.web.address.clone();

    HttpServer::new(move || {
        App::new()
            .wrap(TracingLogger)
            // TODO(phase 4): restrict permissive CORS to the public `/track/*` scope.
            .wrap(Cors::permissive())
            .route("/api/v1/health", web::get().to(health))
            // SPA fallback: serve the embedded frontend, falling back to index.html.
            .default_service(web::get().to(ui::serve))
    })
    .bind(&address)?
    .run()
    .await
}

async fn health() -> HttpResponse {
    HttpResponse::Ok().json(analytics_api::Health {
        ok: true,
        version: version!().to_string(),
    })
}
