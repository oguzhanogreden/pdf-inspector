//! HTTP API server for pdf-inspector.
//!
//! Endpoints:
//!   GET  /health   – liveness check
//!   POST /extract  – multipart "file" upload, returns full extraction JSON
//!   POST /detect   – multipart "file" upload, returns detection-only JSON

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Multipart},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use pdf_inspector::{
    detect_pdf_mem, process_pdf_mem_with_options, PdfError, PdfOptions, PdfProcessResult, PdfType,
    ProcessMode,
};
use serde::Serialize;
use serde_json::json;
use std::net::SocketAddr;
use tokio::signal;

const MAX_BODY_BYTES: usize = 100 * 1024 * 1024; // 100 MB

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    let listener = tokio::net::TcpListener::bind(addr).await?;
    log::info!("pdf-inspector server listening on {}", addr);

    axum::serve(listener, app())
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

fn app() -> Router {
    Router::new()
        .route("/health", get(handler_health))
        .route("/extract", post(handler_extract))
        .route("/detect", post(handler_detect))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
}

// =========================================================================
// Handlers
// =========================================================================

async fn handler_health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

async fn handler_extract(multipart: Multipart) -> Result<Json<ExtractResponse>, AppError> {
    let bytes = read_pdf_field(multipart).await?;
    let result = run_blocking(move || {
        process_pdf_mem_with_options(&bytes, PdfOptions::new().mode(ProcessMode::Full))
    })
    .await?;

    Ok(Json(ExtractResponse::from(result)))
}

async fn handler_detect(multipart: Multipart) -> Result<Json<DetectResponse>, AppError> {
    let bytes = read_pdf_field(multipart).await?;
    let result = run_blocking(move || detect_pdf_mem(&bytes)).await?;
    Ok(Json(DetectResponse::from(result)))
}

// =========================================================================
// Multipart reading
// =========================================================================

async fn read_pdf_field(mut multipart: Multipart) -> Result<Bytes, AppError> {
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("invalid multipart: {e}")))?
    {
        if field.name() == Some("file") {
            let bytes = field
                .bytes()
                .await
                .map_err(|e| AppError::BadRequest(format!("failed to read file: {e}")))?;
            if bytes.len() > MAX_BODY_BYTES {
                return Err(AppError::FileTooLarge);
            }
            if !bytes.starts_with(b"%PDF") {
                return Err(AppError::NotPdf);
            }
            return Ok(bytes);
        }
    }
    Err(AppError::NoFile)
}

// =========================================================================
// Blocking helper
// =========================================================================

async fn run_blocking<F, T>(f: F) -> Result<T, AppError>
where
    F: FnOnce() -> Result<T, PdfError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| AppError::Processing("worker task panicked".into()))?
        .map_err(|e| AppError::Processing(e.to_string()))
}

// =========================================================================
// Response types
// =========================================================================

#[derive(Serialize)]
struct ExtractResponse {
    pdf_type: &'static str,
    page_count: u32,
    has_text: bool,
    processing_time_ms: u64,
    markdown: Option<String>,
    markdown_length: usize,
    pages_needing_ocr: Vec<u32>,
    is_complex: bool,
    pages_with_tables: Vec<u32>,
    pages_with_columns: Vec<u32>,
    has_encoding_issues: bool,
    confidence: f32,
    title: Option<String>,
}

impl From<PdfProcessResult> for ExtractResponse {
    fn from(r: PdfProcessResult) -> Self {
        let markdown_length = r.markdown.as_ref().map(|m| m.len()).unwrap_or(0);
        let has_text = r.markdown.is_some();
        Self {
            pdf_type: pdf_type_str(r.pdf_type),
            page_count: r.page_count,
            has_text,
            processing_time_ms: r.processing_time_ms,
            markdown: r.markdown,
            markdown_length,
            pages_needing_ocr: r.pages_needing_ocr,
            is_complex: r.layout.is_complex,
            pages_with_tables: r.layout.pages_with_tables,
            pages_with_columns: r.layout.pages_with_columns,
            has_encoding_issues: r.has_encoding_issues,
            confidence: r.confidence,
            title: r.title,
        }
    }
}

#[derive(Serialize)]
struct DetectResponse {
    pdf_type: &'static str,
    page_count: u32,
    pages_needing_ocr: Vec<u32>,
    processing_time_ms: u64,
    has_encoding_issues: bool,
    confidence: f32,
    title: Option<String>,
}

impl From<PdfProcessResult> for DetectResponse {
    fn from(r: PdfProcessResult) -> Self {
        Self {
            pdf_type: pdf_type_str(r.pdf_type),
            page_count: r.page_count,
            pages_needing_ocr: r.pages_needing_ocr,
            processing_time_ms: r.processing_time_ms,
            has_encoding_issues: r.has_encoding_issues,
            confidence: r.confidence,
            title: r.title,
        }
    }
}

fn pdf_type_str(t: PdfType) -> &'static str {
    match t {
        PdfType::TextBased => "text_based",
        PdfType::Scanned => "scanned",
        PdfType::ImageBased => "image_based",
        PdfType::Mixed => "mixed",
    }
}

// =========================================================================
// Errors
// =========================================================================

enum AppError {
    NoFile,
    FileTooLarge,
    NotPdf,
    BadRequest(String),
    Processing(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            AppError::NoFile => (
                StatusCode::BAD_REQUEST,
                "no_file",
                "Expected a multipart field named 'file'".to_string(),
            ),
            AppError::FileTooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "file_too_large",
                format!("PDF exceeds maximum size of {} bytes", MAX_BODY_BYTES),
            ),
            AppError::NotPdf => (
                StatusCode::BAD_REQUEST,
                "not_pdf",
                "Uploaded file does not start with %PDF magic bytes".to_string(),
            ),
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, "bad_request", m),
            AppError::Processing(m) => (StatusCode::UNPROCESSABLE_ENTITY, "processing_error", m),
        };
        (status, Json(json!({ "error": code, "message": message }))).into_response()
    }
}

// =========================================================================
// Shutdown
// =========================================================================

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c().await.ok();
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut s) = signal::unix::signal(signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    log::info!("shutdown signal received, draining requests");
}
