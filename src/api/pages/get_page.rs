use crate::api::APIError;
use crate::utils::normalize_page_uri;
use crate::{models::*, telemetry::TraceMessageExt};
use actix_web::{HttpRequest, get, web};
use tracing_batteries::prelude::*;

#[tracing::instrument(err, skip(state), fields(otel.kind = "internal"))]
#[get("/api/v1/page/{domain}/{path:.*}")]
pub async fn get_page_v1(
    req: HttpRequest,
    state: web::Data<GlobalState>,
) -> Result<PageV1, APIError> {
    let domain = req.match_info().query("domain").to_lowercase();
    let path = normalize_page_uri(req.match_info().query("path"));
    state
        .store
        .send(GetPage { domain, path }.trace())
        .await?
        .map(|page| page.into())
}

#[cfg(test)]
mod tests {
    use crate::api::test::*;
    use crate::models::*;

    #[actix_rt::test]
    async fn get_page_v1() {
        test_log_init();

        test_state!(
            state = [
                ViewPage {
                    domain: "test.com".to_string(),
                    path: "/about".to_string(),
                },
                ViewPage {
                    domain: "test.com".to_string(),
                    path: "/".to_string(),
                }
            ]
        );

        let content: PageV1 =
            test_request!(GET "/api/v1/page/test.com/about" => OK with content | state = state);
        assert_eq!(content.domain, "test.com".to_string());
        assert_eq!(content.path, "/about".to_string());
        assert_eq!(content.views, 1, "views should be 1");
        assert_eq!(content.likes, 0, "likes should be 0");
    }
}
