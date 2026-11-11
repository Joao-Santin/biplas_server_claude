mod dados;
mod eventos;
pub mod historico;
mod panorama;

use axum::{
    Json, Router,
    extract::{Request, State},
    middleware::{self, Next},
    response::Response,
    routing::get,
};
use serde_json::{Value, json};

use crate::{egestor, error::AppError, sincronizacao, state::AppState, trello};

/// Monta o Router final:
///
/// - `GET /health`             — status do servidor e de cada integração
/// - `/api/egestor/*`          — gateway para a API do eGestor
/// - `/api/trello/*`           — quadro do Trello (cartões, listas, OPs)
/// - `/api/fontes`, `/api/eventos` — dados recebidos de outras fontes (PostgreSQL)
///
/// Tudo sob `/api` exige `x-api-key` quando `API_KEY` está definida.
pub fn build(state: AppState) -> Router {
    let api = Router::new()
        .nest(egestor::PREFIX.trim_start_matches("/api"), egestor::router())
        .nest(trello::PREFIX.trim_start_matches("/api"), trello::router())
        .merge(eventos::router())
        .merge(dados::router())
        .merge(panorama::router())
        .merge(historico::router())
        .merge(sincronizacao::router())
        .route_layer(middleware::from_fn_with_state(state.clone(), exigir_api_key));

    Router::new()
        .route("/health", get(health))
        .nest("/api", api)
        .with_state(state)
}

async fn exigir_api_key(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, AppError> {
    if let Some(esperada) = &state.api_key {
        let recebida = req.headers().get("x-api-key").and_then(|v| v.to_str().ok());
        if recebida != Some(&**esperada) {
            return Err(AppError::NaoAutorizado);
        }
    }
    Ok(next.run(req).await)
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    let banco = match &state.db {
        None => "desativado",
        Some(pool) => match sqlx::query("SELECT 1").execute(pool).await {
            Ok(_) => "ok",
            Err(e) => {
                tracing::warn!("health: banco indisponível: {e}");
                "erro"
            }
        },
    };
    Json(json!({
        "status": "ok",
        "banco": banco,
        "egestor": state.egestor.is_some(),
        "trello": state.trello.is_some(),
    }))
}
