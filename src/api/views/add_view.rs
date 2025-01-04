use crate::api::APIError;
use crate::utils::normalize_page_uri;
use crate::{models::*, telemetry::TraceMessageExt};
use actix_web::{HttpRequest, post, web};
use tracing_batteries::prelude::*;

#[tracing::instrument(err, skip(state), fields(otel.kind = "internal"))]
#[post("/api/v1/view/{domain}/{path:.*}")]
pub async fn add_view_v1(
    req: HttpRequest,
    state: web::Data<GlobalState>,
) -> Result<PageV1, APIError> {
    let domain = req.match_info().query("domain").to_lowercase();
    let path = normalize_page_uri(req.match_info().query("path"));
    state
        .store
        .send(ViewPage { domain, path }.trace())
        .await?
        .map(|page| page.into())
}

#[cfg(test)]
mod tests {
    use crate::api::test::*;
    use crate::models::*;

    #[actix_rt::test]
    async fn add_view_v1() {
        test_log_init();

        test_state!(state = []);

        let content: PageV1 =
            test_request!(POST "/api/v1/view/test.com/about" => OK with content | state = state);
        assert_eq!(content.domain, "test.com".to_string());
        assert_eq!(content.path, "/about".to_string());
        assert_eq!(content.views, 1, "views should be 1");
        assert_eq!(content.likes, 0, "likes should be 0");

        let content: PageV1 =
            test_request!(POST "/api/v1/view/test.com/about" => OK with content | state = state);
        assert_eq!(content.domain, "test.com".to_string());
        assert_eq!(content.path, "/about".to_string());
        assert_eq!(content.views, 2, "views should be 2");
        assert_eq!(content.likes, 0, "likes should be 0");
    }
}
