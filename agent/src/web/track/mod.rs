//! Public, unauthenticated tracking endpoints.

mod hit;
mod ping;
mod tracker;

use actix_cors::Cors;
use actix_web::web;

/// Register `/tracker.js` and the CORS-enabled `/track/*` beacon endpoints.
pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/tracker.js", web::get().to(tracker::tracker_js)).service(
        // Beacons are sent cross-origin from tracked sites, so this scope (and only
        // this scope) is permissively CORS-enabled. No credentials are involved.
        web::scope("/track")
            .wrap(Cors::permissive())
            .route("/ping", web::get().to(ping::ping))
            .route("/hit", web::post().to(hit::hit)),
    );
}
