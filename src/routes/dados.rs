//! Dados do eGestor gravados pela sincronização, para os programas da rede.
//! Cada rota devolve uma lista com o JSON de cada registro exatamente como o
//! eGestor o devolveu. A data dos dados é `ultima_ok_em` em `GET /api/sync`.

use axum::{
    Router,
    extract::{Path, Query, State},
    http::header::CONTENT_TYPE,
    response::IntoResponse,
    routing::get,
};
use serde::Deserialize;

use crate::{error::AppError, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new().route("/dados/egestor/{recurso}", get(listar))
}

#[derive(Deserialize)]
struct Filtro {
    /// Só para `producao`: `true` = só as OPs em aberto.
    #[serde(default)]
    abertas: bool,
}

/// GET /api/dados/egestor/{produtos|categorias|composicoes|producao}[?abertas=true]
async fn listar(
    State(state): State<AppState>,
    Path(recurso): Path<String>,
    Query(f): Query<Filtro>,
) -> Result<impl IntoResponse, AppError> {
    let sql = match recurso.as_str() {
        "produtos" => "SELECT COALESCE(jsonb_agg(dados ORDER BY codigo), '[]')::text FROM egestor_produtos",
        "categorias" => "SELECT COALESCE(jsonb_agg(dados ORDER BY codigo), '[]')::text FROM egestor_categorias",
        "composicoes" => {
            "SELECT COALESCE(jsonb_agg(dados ORDER BY cod_produto), '[]')::text FROM egestor_composicoes"
        }
        "producao" if f.abertas => {
            "SELECT COALESCE(jsonb_agg(dados ORDER BY codigo), '[]')::text FROM egestor_producao WHERE aberta"
        }
        "producao" => "SELECT COALESCE(jsonb_agg(dados ORDER BY codigo), '[]')::text FROM egestor_producao",
        _ => {
            return Err(AppError::NotFound(format!(
                "Recurso \"{recurso}\" não existe. Use produtos, categorias, composicoes ou producao."
            )));
        }
    };
    // O JSON é montado pelo próprio PostgreSQL e vai direto na resposta.
    let corpo: String = sqlx::query_scalar(sql).fetch_one(state.db()?).await?;
    Ok(([(CONTENT_TYPE, "application/json")], corpo))
}
