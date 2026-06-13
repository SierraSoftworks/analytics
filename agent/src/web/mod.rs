mod api;
pub mod extract;
pub mod helpers;
mod track;
mod ui;

use actix_web::{App, HttpServer, web};

use crate::errors::{Result, ResultExt};
use crate::state::AppState;
use crate::telemetry::TracingLogger;

/// Start the HTTP server and block until it shuts down.
pub async fn run(state: AppState) -> Result<()> {
    let address = state.config.web.address.clone();
    let data = web::Data::new(state);

    let server = HttpServer::new(move || {
        App::new()
            .app_data(data.clone())
            .wrap(TracingLogger)
            // Public tracking endpoints (their own CORS) + the tracker script.
            .configure(track::configure)
            // /api/v1: public health + auth, everything else gated by api_auth.
            .configure(api::configure)
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
