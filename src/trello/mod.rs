//! Integração com o Trello: lê o quadro configurado (padrão "Programação
//! Máquinas") e serve os cartões das listas configuradas em JSON.

pub mod client;
pub mod model;

pub use client::TrelloClient;

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::get,
};

use crate::{
    error::AppError,
    state::AppState,
    trello::model::{CartaoOp, Lista, Quadro, slug},
};

/// Prefixo sob o qual as rotas do Trello ficam montadas no servidor.
pub const PREFIX: &str = "/api/trello";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/cartoes", get(cartoes))
        .route("/listas/{lista}", get(lista))
        .route("/ops", get(ops))
}

/// GET /api/trello/cartoes — o quadro com as listas configuradas e seus cartões.
async fn cartoes(State(state): State<AppState>) -> Result<Json<Quadro>, AppError> {
    let trello = state.trello()?;
    let raw = trello.board().await?;
    Ok(Json(model::build(raw, trello.lists())?))
}

/// GET /api/trello/listas/{lista} — uma única lista. Aceita o slug
/// ("em-andamento") ou o nome, sem diferenciar maiúsculas nem acentos.
async fn lista(
    State(state): State<AppState>,
    Path(lista): Path<String>,
) -> Result<Json<Lista>, AppError> {
    let trello = state.trello()?;
    let key = slug(&lista);
    let Some(name) = trello.lists().iter().find(|l| slug(l) == key) else {
        let slugs: Vec<String> = trello.lists().iter().map(|l| slug(l)).collect();
        return Err(AppError::NotFound(format!(
            "Lista \"{lista}\" não é servida. Use uma de: {}",
            slugs.join(", ")
        )));
    };

    let raw = trello.board().await?;
    let quadro = model::build(raw, std::slice::from_ref(name))?;
    Ok(Json(quadro.listas.into_iter().next().expect("build devolve a lista pedida")))
}

/// GET /api/trello/ops — cada cartão com suas OPs, máquina e quando entrou
/// em cada lista.
async fn ops(State(state): State<AppState>) -> Result<Json<Vec<CartaoOp>>, AppError> {
    let trello = state.trello()?;
    let board_id = trello.board_id().await?;
    let (raw, moves) = tokio::try_join!(trello.board(), trello.moves(&board_id))?;
    let quadro = model::build(raw, trello.lists())?;
    Ok(Json(model::ops(quadro, &moves)))
}
