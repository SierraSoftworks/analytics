use actix_web::web;

mod add_like;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(add_like::add_like_v1);
}
