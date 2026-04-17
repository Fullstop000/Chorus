use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::agent::templates::group_by_category;
use crate::agent::AgentRuntime;
use crate::store::messages::SenderType;
use crate::store::AgentRecordUpsert;

use super::{app_err, ApiResult, AppState};

// ── Response types ──

#[derive(Serialize)]
pub struct TemplatesResponse {
    pub categories: Vec<crate::agent::templates::TemplateCategory>,
}

#[derive(Deserialize)]
pub struct LaunchTrioRequest {
    pub template_ids: Vec<String>,
}

#[derive(Serialize)]
pub struct LaunchTrioResponse {
    pub channel_id: String,
    pub agents: Vec<LaunchTrioAgent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<LaunchTrioError>,
}

#[derive(Serialize)]
pub struct LaunchTrioAgent {
    pub id: String,
    pub name: String,
    pub display_name: String,
}

#[derive(Serialize)]
pub struct LaunchTrioError {
    pub template_id: String,
    pub error: String,
}

// ── Handlers ──

pub async fn handle_list_templates(State(state): State<AppState>) -> ApiResult<TemplatesResponse> {
    let categories = group_by_category(&state.templates);
    Ok(Json(TemplatesResponse { categories }))
}

pub async fn handle_launch_trio(
    State(state): State<AppState>,
    Json(req): Json<LaunchTrioRequest>,
) -> Result<(StatusCode, Json<LaunchTrioResponse>), (StatusCode, Json<super::ErrorResponse>)> {
    if req.template_ids.is_empty() || req.template_ids.len() > 10 {
        return Err(app_err!(
            StatusCode::BAD_REQUEST,
            "template_ids must contain 1-10 entries"
        ));
    }

    // Resolve templates.
    let mut resolved = Vec::new();
    for tid in &req.template_ids {
        let template = state.templates.iter().find(|t| t.id == *tid);
        match template {
            Some(t) => resolved.push(t.clone()),
            None => {
                return Err(app_err!(
                    StatusCode::BAD_REQUEST,
                    "template not found: {tid}"
                ))
            }
        }
    }

    // Create the trio channel.
    let channel_name = format!(
        "trio-{}-{}",
        chrono::Utc::now().format("%Y%m%d-%H%M%S"),
        &uuid::Uuid::new_v4().to_string()[..6]
    );
    let channel_id = state
        .store
        .create_channel(
            &channel_name,
            None,
            crate::store::channels::ChannelType::Channel,
        )
        .map_err(|e| {
            app_err!(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create trio channel: {e}"
            )
        })?;

    // Join the human user to the channel.
    if let Ok(humans) = state.store.get_humans() {
        if let Some(human) = humans.first() {
            let _ = state
                .store
                .join_channel(&channel_name, &human.name, SenderType::Human);
        }
    }

    let mut agents = Vec::new();
    let mut errors = Vec::new();

    for template in &resolved {
        let base_name = template
            .id
            .split('/')
            .nth(1)
            .unwrap_or(&template.id)
            .to_string();

        // Resolve default model for the runtime.
        let model = match resolve_default_model(&state, &template.suggested_runtime).await {
            Some(m) => m,
            None => {
                errors.push(LaunchTrioError {
                    template_id: template.id.clone(),
                    error: format!(
                        "no models available for runtime '{}'",
                        template.suggested_runtime
                    ),
                });
                continue;
            }
        };

        // Create the agent record. Every agent gets a `{base}-{hex4}`
        // slug; retry on the rare UNIQUE-constraint collision.
        let mut created: Option<(String, String)> = None;
        let mut last_error: Option<String> = None;
        for _ in 0..MAX_SLUG_ATTEMPTS {
            let candidate = format!("{base_name}-{}", random_slug_suffix());
            match state.store.create_agent_record(&AgentRecordUpsert {
                name: &candidate,
                display_name: &template.name,
                description: template.description.as_deref(),
                system_prompt: Some(&template.prompt_body),
                runtime: &template.suggested_runtime,
                model: &model,
                reasoning_effort: None,
                env_vars: &[],
            }) {
                Ok(id) => {
                    created = Some((id, candidate));
                    break;
                }
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("UNIQUE constraint") {
                        last_error = Some(msg);
                        continue;
                    }
                    last_error = Some(msg);
                    break;
                }
            }
        }
        let (agent_id, agent_name) = match created {
            Some(pair) => pair,
            None => {
                errors.push(LaunchTrioError {
                    template_id: template.id.clone(),
                    error: format!(
                        "failed to create agent: {}",
                        last_error.unwrap_or_else(|| {
                            format!(
                                "could not allocate a unique slug after {MAX_SLUG_ATTEMPTS} attempts"
                            )
                        })
                    ),
                });
                continue;
            }
        };

        // Join agent to the trio channel.
        let _ = state
            .store
            .join_channel(&channel_name, &agent_name, SenderType::Agent);

        // Start the agent.
        if let Err(e) = state.lifecycle.start_agent(&agent_name, None).await {
            warn!(
                agent = %agent_name,
                error = %e,
                "trio agent created but failed to start"
            );
        }

        agents.push(LaunchTrioAgent {
            id: agent_id,
            name: agent_name,
            display_name: template.name.clone(),
        });
    }

    // Post kickoff message.
    if !agents.is_empty() {
        let names: Vec<&str> = agents.iter().map(|a| a.display_name.as_str()).collect();
        let kickoff = format!("Team assembled: {}. Let's get to work.", names.join(", "));
        if let Err(e) = state.store.create_system_message(&channel_id, &kickoff) {
            warn!(error = %e, "failed to post trio kickoff message");
        }
    }

    info!(
        channel = %channel_name,
        agents_created = agents.len(),
        errors = errors.len(),
        "launch trio completed"
    );

    let status = if errors.is_empty() {
        StatusCode::CREATED
    } else {
        StatusCode::MULTI_STATUS
    };

    Ok((
        status,
        Json(LaunchTrioResponse {
            channel_id,
            agents,
            errors,
        }),
    ))
}

/// Maximum attempts at inserting a `{base}-{hex4}` agent slug before
/// giving up. 16⁴ = 65_536 suffix combinations per base, so hitting this
/// cap implies either a pathological number of siblings or a real bug.
pub(crate) const MAX_SLUG_ATTEMPTS: u32 = 5;

/// 4-character lowercase hex suffix used to keep agent slugs unique.
/// Callers combine it with the user-facing base name as `{base}-{hex4}`
/// and retry on the rare UNIQUE-constraint collision.
pub(crate) fn random_slug_suffix() -> String {
    use rand::Rng;
    let n: u16 = rand::rng().random();
    format!("{n:04x}")
}

/// Pick the first available model for a runtime.
async fn resolve_default_model(state: &AppState, runtime: &str) -> Option<String> {
    let rt = AgentRuntime::parse(runtime)?;
    state
        .runtime_status_provider
        .list_models(rt)
        .await
        .ok()
        .and_then(|models| models.into_iter().next())
}
