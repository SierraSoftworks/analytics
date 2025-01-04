use actix_web::web;

mod add_view;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(add_view::add_view_v1);
}
