//! Shared MCP HTTP admission. Plugin authorization remains in Electron.
use axum::{
    body::{to_bytes, Body},
    extract::Request,
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    Router,
};
use rmcp::{
    transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    },
    ServerHandler,
};
use std::time::Duration;
use subtle::ConstantTimeEq;

pub fn router<S>(handler: S, token: String, host: String) -> Router
where
    S: ServerHandler + Clone + Send + Sync + 'static,
{
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_max_request_body_bytes(64 * 1024);
    let service = StreamableHttpService::new(
        move || Ok(handler.clone()),
        LocalSessionManager::default().into(),
        config,
    );
    let authorization = format!("Bearer {token}");
    Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn(move |request: Request, next: Next| {
            let expected = authorization.clone();
            let host = host.clone();
            async move { admit(request, next, &expected, &host).await }
        }))
}

async fn admit(request: Request, next: Next, authorization: &str, host: &str) -> Response {
    if request.headers().contains_key("origin")
        || request.headers().get("host").and_then(|v| v.to_str().ok()) != Some(host)
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let provided = request
        .headers()
        .get("authorization")
        .map(|v| v.as_bytes())
        .unwrap_or_default();
    if !bool::from(provided.ct_eq(authorization.as_bytes())) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let (parts, body) = request.into_parts();
    match tokio::time::timeout(Duration::from_secs(15), to_bytes(body, 64 * 1024)).await {
        Ok(Ok(bytes)) => {
            // Preserve the existing endpoint's malformed-JSON status (rmcp otherwise uses 415).
            if parts.method == axum::http::Method::POST
                && serde_json::from_slice::<serde_json::Value>(&bytes).is_err()
            {
                return StatusCode::BAD_REQUEST.into_response();
            }
            next.run(Request::from_parts(parts, Body::from(bytes)))
                .await
        }
        Ok(Err(_)) => StatusCode::PAYLOAD_TOO_LARGE.into_response(),
        Err(_) => StatusCode::REQUEST_TIMEOUT.into_response(),
    }
}
