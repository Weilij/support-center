//! Request-id correlation: the response body `requestId`, the `X-Request-ID`
//! response header, and an inbound `X-Request-ID` all share one id.

mod common;

use common::spawn_app;

#[tokio::test]
async fn reuses_inbound_x_request_id_in_body_and_header() {
    let app = spawn_app().await;
    // Unauthenticated request: even the error envelope carries the correlation id.
    let (_status, body, headers) = app
        .request_with_headers(
            "GET",
            "/api/teams",
            None,
            None,
            &[("x-request-id", "corr-xyz-1")],
        )
        .await;
    assert_eq!(body["requestId"], "corr-xyz-1");
    assert_eq!(
        headers.get("x-request-id").unwrap().to_str().unwrap(),
        "corr-xyz-1"
    );
}

#[tokio::test]
async fn generates_id_shared_by_body_and_header() {
    let app = spawn_app().await;
    let (_status, body, headers) = app.request("GET", "/api/teams", None, None).await;
    let id = body["requestId"].as_str().unwrap();
    assert!(!id.is_empty());
    assert_eq!(headers.get("x-request-id").unwrap().to_str().unwrap(), id);
}
