use axum::extract::{Path, State};
use axum::{Extension, Json};
use uuid::Uuid;

use crate::agent::{self, ChatRequest, ChatTurn, ConversationView};
use crate::error::AppResult;
use crate::session::DemoSession;
use crate::AppState;

/// POST /api/agent/chat
pub async fn chat(
    State(st): State<AppState>,
    Extension(DemoSession(sid)): Extension<DemoSession>,
    Json(req): Json<ChatRequest>,
) -> AppResult<Json<ChatTurn>> {
    Ok(Json(agent::chat(&st, sid, req).await?))
}

/// GET /api/agent/conversations/{id}
pub async fn conversation(
    State(st): State<AppState>,
    Extension(DemoSession(sid)): Extension<DemoSession>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<ConversationView>> {
    Ok(Json(agent::conversation(&st, sid, id).await?))
}
