use crate::api::APIError;
use crate::utils::normalize_page_uri;
use crate::{models::*, telemetry::TraceMessageExt};
use actix_web::{post, web, HttpRequest};
use tracing_batteries::prelude::*;

#[tracing::instrument(err, skip(state), fields(otel.kind = "internal"))]
#[post("/api/v1/like/{domain}/{path:.*}")]
pub async fn add_like_v1(
    req: HttpRequest,
    state: web::Data<GlobalState>,
) -> Result<PageV1, APIError> {
    let domain = req.match_info().query("domain").to_lowercase();
    let path = normalize_page_uri(req.match_info().query("path"));
    state
        .store
        .send(LikePage { domain, path }.trace())
        .await?
        .map(|page| page.into())
}

#[cfg(test)]
mod tests {
    use crate::api::test::*;
    use crate::models::*;

    #[actix_rt::test]
    async fn add_like_v1() {
        test_log_init();

        test_state!(state = []);

        let content: PageV1 =
            test_request!(POST "/api/v1/like/test.com/about" => OK with content | state = state);
        assert_eq!(content.domain, "test.com".to_string());
        assert_eq!(content.path, "/about".to_string());
        assert_eq!(content.views, 0, "views should be 0");
        assert_eq!(content.likes, 1, "likes should be 1");

        let content: PageV1 =
            test_request!(POST "/api/v1/like/test.com/about" => OK with content | state = state);
        assert_eq!(content.domain, "test.com".to_string());
        assert_eq!(content.path, "/about".to_string());
        assert_eq!(content.views, 0, "views should be 0");
        assert_eq!(content.likes, 2, "likes should be 2");
    }
}
