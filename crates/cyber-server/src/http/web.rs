//! Embedded, inert public shell. All user data remains behind the API guard.
use super::AppState;
use axum::http::{StatusCode, header};
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    middleware::Next,
    response::Response,
    routing::get,
};

const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' blob:; font-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'; object-src 'none'";

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/",
            get(|| async {
                asset(
                    "text/html; charset=utf-8",
                    include_bytes!("../../../../web/dist/index.html"),
                )
            }),
        )
        .route(
            "/app/app.js",
            get(|| async {
                asset(
                    "text/javascript; charset=utf-8",
                    include_bytes!("../../../../web/dist/app.js"),
                )
            }),
        )
        .route(
            "/app/app.css",
            get(|| async {
                asset(
                    "text/css; charset=utf-8",
                    include_bytes!("../../../../web/dist/app.css"),
                )
            }),
        )
}
fn asset(kind: &'static str, bytes: &'static [u8]) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, kind)
        .body(Body::from(bytes))
        .expect("static response")
}
/// Applies to errors too. Dynamic data, credentials and stale approval cards must never
/// be replayed from a shared cache; the public shell contains no embedded credentials.
pub(super) async fn security_headers(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    // API failures retain the existing structured guard response and CORS contract.
    let asset_path = request.uri().path() == "/" || request.uri().path().starts_with("/app/");
    let foreign = asset_path
        && request
            .headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|origin| {
                !super::guard::allowed_origin(
                    origin,
                    request.headers(),
                    &state.options.cors_origins,
                )
            });
    let mut response = if foreign {
        Response::builder()
            .status(StatusCode::FORBIDDEN)
            .body(Body::empty())
            .expect("static response")
    } else {
        next.run(request).await
    };
    for (name, value) in [
        (header::CONTENT_SECURITY_POLICY, CSP),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::X_FRAME_OPTIONS, "DENY"),
    ] {
        response
            .headers_mut()
            .insert(name, value.parse().expect("static header"));
    }
    let previous = response
        .headers()
        .get(header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !previous.split(',').any(|part| part.trim() == "no-store") {
        let value = if previous.is_empty() {
            "no-store".to_owned()
        } else {
            format!("no-store, {previous}")
        };
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            value.parse().expect("valid cache header"),
        );
    }
    response
}
