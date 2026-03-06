use axum::{
    body::{Body, Bytes},
    error_handling::HandleErrorLayer,
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    BoxError, Router,
};
use reqwest::{Client, Response as ReqwestResponse};
use std::collections::HashMap;
use std::time::Duration;
use tower::{buffer::BufferLayer, limit::RateLimitLayer, ServiceBuilder};
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .tcp_keepalive(Duration::from_secs(60))
        .build()
        .expect("Failed to create HTTP client");

    let middleware_stack = ServiceBuilder::new()
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .layer(CompressionLayer::new())

        .layer(HandleErrorLayer::new(|err: BoxError| async move {
            (
                StatusCode::TOO_MANY_REQUESTS,
                format!("Slow down! Error: {}", err),
            )
        }))
        .layer(BufferLayer::new(1024))
        .layer(RateLimitLayer::new(20, Duration::from_secs(60)));

    let app = Router::new()
        .route("/filter", get(filter_proxy))
        .with_state(client)
        .layer(middleware_stack);

    let port = std::env::var("PORT").unwrap_or_else(|_| "9001".to_string());
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    
    tracing::info!("Rust Proxy starting on {}", addr);
    axum::serve(listener, app).await.unwrap();
}

async fn filter_proxy(
    State(client): State<Client>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {

    let mut target_url = reqwest::Url::parse("https://interoperability.onrender.com/filter").unwrap();
    {
        let mut query_pairs = target_url.query_pairs_mut();
        for (k, v) in params {
            query_pairs.append_pair(&k, &v);
        }
    }

    let res: Result<ReqwestResponse, reqwest::Error> = client
        .get(target_url)
        .header(header::HOST, "interoperability.onrender.com")
        .header("X-Forwarded-For", headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()).unwrap_or("0.0.0.0"))
        .header(header::USER_AGENT, "Rust-Axum-Proxy")
        .send()
        .await;

    match res {
        Ok(response) => {
            let status = response.status();
            let body_bytes = response.bytes().await.unwrap_or_else(|_| Bytes::new());
            
            Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body_bytes))
                .unwrap()
        }
        Err(err) => {
            tracing::error!("Proxy Error: {}", err);
            (StatusCode::BAD_GATEWAY, format!("{{\"error\": \"{}\"}}", err)).into_response()
        }
    }
}