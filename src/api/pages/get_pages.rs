use crate::api::APIError;
use crate::{models::*, telemetry::TraceMessageExt};
use actix_web::{HttpRequest, get, web};
use tracing_batteries::prelude::*;

#[tracing::instrument(err, skip(state), fields(otel.kind = "internal"))]
#[get("/api/v1/pages/{domain}")]
pub async fn get_pages_v1(
    req: HttpRequest,
    state: web::Data<GlobalState>,
) -> Result<web::Json<Vec<PageV1>>, APIError> {
    let domain = req.match_info().query("domain").to_lowercase();
    state
        .store
        .send(GetPages { domain }.trace())
        .await?
        .map(|page| page.into_iter().map(|p| p.into()).collect::<Vec<_>>())
        .map(web::Json)
}

#[cfg(test)]
mod tests {
    use crate::api::test::*;
    use crate::models::*;

    #[actix_rt::test]
    async fn get_pages_v1() {
        test_log_init();

        test_state!(
            state = [
                ViewPage {
                    domain: "test.com".to_string(),
                    path: "/".to_string(),
                },
                ViewPage {
                    domain: "test.com".to_string(),
                    path: "/".to_string(),
                }
            ]
        );

        let content: Vec<PageV1> =
            test_request!(GET "/api/v1/pages/test.com" => OK with content | state = state);
        assert_eq!(content.len(), 1);
        assert_eq!(content[0].domain, "test.com".to_string());
        assert_eq!(content[0].path, "/".to_string());
        assert_eq!(content[0].views, 2, "views should be 2");
        assert_eq!(content[0].likes, 0, "likes should be 0");
    }
}
