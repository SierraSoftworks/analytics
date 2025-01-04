use actix_web::web;

mod get_page;
mod get_pages;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(get_page::get_page_v1)
        .service(get_pages::get_pages_v1);
}
