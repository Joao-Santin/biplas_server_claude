//! Integração com o Trello: o quadro configurado (padrão "Programação
//! Máquinas") é sincronizado para o banco (`sincronizacao::trello`) e as
//! rotas daqui servem o que está gravado — nunca consultam o Trello.

pub mod client;
pub mod model;

pub use client::TrelloClient;

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::get,
};
use chrono::{DateTime, SecondsFormat, Utc};
use sqlx::{PgPool, types::Json as SqlJson};

use crate::{
    error::AppError,
    state::AppState,
    trello::model::{Cartao, CartaoOp, Entradas, Lista, Quadro, slug},
};

/// Prefixo sob o qual as rotas do Trello ficam montadas no servidor.
pub const PREFIX: &str = "/api/trello";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/cartoes", get(cartoes))
        .route("/listas/{lista}", get(lista))
        .route("/ops", get(ops))
}

/// Monta o quadro gravado no banco. `so_slug` restringe a uma lista.
async fn carregar(db: &PgPool, so_slug: Option<&str>) -> Result<Quadro, AppError> {
    let quadro: Option<(String, String, String)> =
        sqlx::query_as("SELECT id, nome, url FROM trello_quadro").fetch_optional(db).await?;
    let Some((id, nome, url)) = quadro else {
        return Err(AppError::Indisponivel(
            "O Trello ainda não foi sincronizado. Peça com POST /api/sync/trello.".into(),
        ));
    };

    let listas: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT id, nome, slug FROM trello_listas WHERE $1::text IS NULL OR slug = $1 ORDER BY ordem",
    )
    .bind(so_slug)
    .fetch_all(db)
    .await?;
    let cartoes: Vec<(String, SqlJson<Cartao>)> = sqlx::query_as(
        "SELECT lista_id, dados FROM trello_cartoes c JOIN trello_listas l ON l.id = c.lista_id
         WHERE $1::text IS NULL OR l.slug = $1 ORDER BY posicao",
    )
    .bind(so_slug)
    .fetch_all(db)
    .await?;

    let mut listas: Vec<Lista> = listas
        .into_iter()
        .map(|(id, nome, slug)| Lista { id, nome, slug, total: 0, cartoes: Vec::new() })
        .collect();
    for (lista_id, SqlJson(cartao)) in cartoes {
        if let Some(l) = listas.iter_mut().find(|l| l.id == lista_id) {
            l.cartoes.push(cartao);
        }
    }
    for l in &mut listas {
        l.total = l.cartoes.len();
    }
    Ok(Quadro { id, nome, url, listas })
}

/// GET /api/trello/cartoes — o quadro com as listas configuradas e seus cartões.
async fn cartoes(State(state): State<AppState>) -> Result<Json<Quadro>, AppError> {
    Ok(Json(carregar(state.db()?, None).await?))
}

/// GET /api/trello/listas/{lista} — uma única lista. Aceita o slug
/// ("em-andamento") ou o nome, sem diferenciar maiúsculas nem acentos.
async fn lista(
    State(state): State<AppState>,
    Path(lista): Path<String>,
) -> Result<Json<Lista>, AppError> {
    let db = state.db()?;
    let key = slug(&lista);
    let quadro = carregar(db, Some(&key)).await?;
    if let Some(l) = quadro.listas.into_iter().next() {
        return Ok(Json(l));
    }
    let slugs: Vec<String> =
        sqlx::query_scalar("SELECT slug FROM trello_listas ORDER BY ordem").fetch_all(db).await?;
    Err(AppError::NotFound(format!("Lista \"{lista}\" não é servida. Use uma de: {}", slugs.join(", "))))
}

/// GET /api/trello/ops — cada cartão com suas OPs, máquina e quando entrou
/// em cada lista.
async fn ops(State(state): State<AppState>) -> Result<Json<Vec<CartaoOp>>, AppError> {
    let db = state.db()?;
    let quadro = carregar(db, None).await?;
    let linhas: Vec<(String, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT m.cartao_id, l.slug, max(m.data)
         FROM trello_movimentos m
         JOIN trello_listas l ON l.id = m.lista_id
         JOIN trello_cartoes c ON c.id = m.cartao_id
         GROUP BY m.cartao_id, l.slug",
    )
    .fetch_all(db)
    .await?;
    let mut entradas = Entradas::new();
    for (cartao, slug, data) in linhas {
        // Mesmo formato do Trello: 2026-10-03T10:00:00.000Z
        entradas.entry(cartao).or_default().insert(slug, data.to_rfc3339_opts(SecondsFormat::Millis, true));
    }
    Ok(Json(model::ops(quadro, &entradas)))
}
