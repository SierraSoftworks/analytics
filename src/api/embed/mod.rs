use actix_web::web;

mod get_embedding;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(get_embedding::get_embedding_v1);
}
