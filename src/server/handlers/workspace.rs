use std::path::PathBuf;

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use super::{app_err, ApiResult, AppState};

// ── Inline query structs ──

#[derive(Deserialize)]
pub struct WorkspaceFileParams {
    pub path: String,
}

// ── Private helpers ──

fn sanitize_workspace_path(
    path: &str,
) -> Result<PathBuf, (axum::http::StatusCode, axum::Json<super::ErrorResponse>)> {
    use std::path::Component;

    let candidate = std::path::Path::new(path);
    let mut cleaned = PathBuf::new();
    for component in candidate.components() {
        match component {
            Component::Normal(part) => cleaned.push(part),
            Component::CurDir => {}
            _ => return Err(app_err!(StatusCode::BAD_REQUEST, "invalid workspace path")),
        }
    }

    if cleaned.as_os_str().is_empty() {
        return Err(app_err!(StatusCode::BAD_REQUEST, "invalid workspace path"));
    }

    Ok(cleaned)
}

fn collect_workspace_files(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<String>,
    depth: usize,
) {
    if depth > 5 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut sorted: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    sorted.sort_by_key(|e| e.file_name());
    for entry in sorted {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        if path.is_dir() {
            out.push(format!("{}/", rel));
            collect_workspace_files(root, &path, out, depth + 1);
        } else {
            out.push(rel);
        }
    }
}

// ── Public handlers ──

pub async fn handle_agent_workspace(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
) -> ApiResult<serde_json::Value> {
    let workspace_dir = state.store.agents_dir().join(&name);
    if !workspace_dir.exists() {
        return Ok(Json(serde_json::json!({
            "path": workspace_dir.to_string_lossy(),
            "files": []
        })));
    }
    let mut files: Vec<String> = Vec::new();
    collect_workspace_files(&workspace_dir, &workspace_dir, &mut files, 0);
    Ok(Json(serde_json::json!({
        "path": workspace_dir.to_string_lossy(),
        "files": files
    })))
}

pub async fn handle_agent_workspace_file(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    Query(params): Query<WorkspaceFileParams>,
) -> ApiResult<serde_json::Value> {
    let workspace_dir = state.store.agents_dir().join(&name);
    let relative = sanitize_workspace_path(&params.path)?;
    let file_path = workspace_dir.join(&relative);

    if !file_path.is_file() {
        return Err(app_err!(
            StatusCode::BAD_REQUEST,
            "workspace file not found"
        ));
    }

    let metadata = std::fs::metadata(&file_path)
        .map_err(|e| app_err!(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let bytes = std::fs::read(&file_path)
        .map_err(|e| app_err!(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let limit = 100_000usize;
    let truncated = bytes.len() > limit;
    let content = if truncated {
        String::from_utf8_lossy(&bytes[..limit]).into_owned()
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64);

    Ok(Json(serde_json::json!({
        "path": relative.to_string_lossy(),
        "content": content,
        "truncated": truncated,
        "sizeBytes": metadata.len(),
        "modifiedMs": modified_ms
    })))
}
