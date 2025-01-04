use crate::api::APIError;
use crate::utils::normalize_page_uri;
use crate::{models::*, telemetry::TraceMessageExt};
use actix_web::body::BoxBody;
use actix_web::{HttpRequest, HttpResponse, Responder, get, web};
use http::StatusCode;
use tracing_batteries::prelude::*;

static GIF: &[u8] = include_bytes!("./blank.gif");

#[tracing::instrument(err, skip(state), fields(otel.kind = "internal"))]
#[get("/embed/{domain}/{path:.*}")]
pub async fn get_embedding_v1(
    req: HttpRequest,
    state: web::Data<GlobalState>,
) -> Result<Embedding, APIError> {
    let domain = req.match_info().query("domain").to_lowercase();
    let path = normalize_page_uri(req.match_info().query("path"));
    let _ = state.store.send(ViewPage { domain, path }.trace()).await?;

    Ok(Embedding)
}

struct Embedding;

impl Responder for Embedding {
    type Body = BoxBody;

    fn respond_to(self, _req: &HttpRequest) -> HttpResponse<Self::Body> {
        HttpResponse::Ok().status(StatusCode::OK).body(GIF)
    }
}

#[cfg(test)]
mod tests {
    use actix_web::body::MessageBody;
    use actix_web::web::Bytes;

    use crate::api::embed::get_embedding::GIF;
    use crate::api::test::*;
    use crate::models::*;

    #[actix_rt::test]
    async fn embed_v1() {
        test_log_init();

        test_state!(state = []);

        test_request!(GET "/embed/test.com/about" => OK | state = state);

        let gif = test_request!(GET "/embed/test.com/about" => OK | state = state);
        assert_eq!(gif.into_body().try_into_bytes().unwrap(), Bytes::from(GIF));

        let page: PageV1 =
            test_request!(GET "/api/v1/page/test.com/about" => OK with content | state = state);
        assert_eq!(page.domain, "test.com".to_string());
        assert_eq!(page.path, "/about".to_string());
        assert_eq!(page.views, 2, "views should be 2");
    }
}
