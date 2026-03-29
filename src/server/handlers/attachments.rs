use axum::extract::{Multipart, Path, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use tracing::info;
use uuid::Uuid;

use super::{api_err, internal_err, ApiResult, AppState, ErrorResponse};

async fn store_upload(state: AppState, mut multipart: Multipart) -> ApiResult<serde_json::Value> {
    let field = multipart
        .next_field()
        .await
        .map_err(|e| api_err(e.to_string()))?
        .ok_or_else(|| api_err("no file uploaded"))?;

    let filename = field.file_name().unwrap_or("upload").to_string();
    let content_type = field
        .content_type()
        .unwrap_or("application/octet-stream")
        .to_string();
    let data = field.bytes().await.map_err(|e| api_err(e.to_string()))?;

    let ext = std::path::Path::new(&filename)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();

    let file_id = Uuid::new_v4().to_string();
    let attachments_dir = state.store.attachments_dir();
    std::fs::create_dir_all(&attachments_dir).map_err(|e| internal_err(e.to_string()))?;

    let stored_path = attachments_dir.join(format!("{}{}", file_id, ext));
    std::fs::write(&stored_path, &data).map_err(|e| internal_err(e.to_string()))?;

    let size = data.len() as i64;
    let att_id = state
        .store
        .create_attachment(
            &filename,
            &content_type,
            size,
            stored_path.to_string_lossy().as_ref(),
        )
        .map_err(|e| api_err(e.to_string()))?;

    info!(filename = %filename, id = %att_id, "attachment uploaded");

    Ok(Json(
        serde_json::json!({ "id": att_id, "filename": filename, "sizeBytes": size }),
    ))
}

pub async fn handle_upload(
    State(state): State<AppState>,
    Path(_agent_id): Path<String>,
    multipart: Multipart,
) -> ApiResult<serde_json::Value> {
    store_upload(state, multipart).await
}

pub async fn handle_public_upload(
    State(state): State<AppState>,
    multipart: Multipart,
) -> ApiResult<serde_json::Value> {
    store_upload(state, multipart).await
}

pub async fn handle_get_attachment(
    State(state): State<AppState>,
    Path(attachment_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    let attachment = state
        .store
        .get_attachment(&attachment_id)
        .map_err(|e| api_err(e.to_string()))?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "attachment not found".to_string(),
                }),
            )
        })?;

    let data = std::fs::read(&attachment.stored_path).map_err(|e| internal_err(e.to_string()))?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, attachment.mime_type)],
        data,
    ))
}
