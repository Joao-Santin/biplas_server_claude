//! Dados do Panorama de Injetados guardados no banco e compartilhados por
//! todos os computadores: configuração de colunas, simulações e realizado
//! parcial das OPs. Toda mudança é avisada em `/api/notificacoes`
//! (`{"tipo":"dados","recurso":...}`), para os outros panoramas abertos
//! recarregarem.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{error::AppError, sincronizacao::origem, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/panorama/colunas/{tabela}", get(ler_colunas).put(gravar_colunas))
        .route("/panorama/simulacoes", get(listar_simulacoes).post(criar_simulacao))
        .route("/panorama/simulacoes/{id}", delete(excluir_simulacao))
        .route("/panorama/parciais", get(listar_parciais).put(gravar_parcial))
        .route("/panorama/parciais/{op}/{linha}/{cod_produto}", delete(excluir_parcial))
}

// ---- Colunas -----------------------------------------------------------------

fn tabela_valida(tabela: &str) -> Result<(), AppError> {
    match tabela {
        "necessidades" | "produtos" => Ok(()),
        _ => Err(AppError::NotFound(format!("Tabela \"{tabela}\" não existe. Use necessidades ou produtos."))),
    }
}

/// GET /api/panorama/colunas/{tabela} — `{"colunas": [...]}`, ou `null` se
/// ainda não foi configurada (o programa usa o padrão).
async fn ler_colunas(State(state): State<AppState>, Path(tabela): Path<String>) -> Result<Json<Value>, AppError> {
    tabela_valida(&tabela)?;
    let config: Option<Value> = sqlx::query_scalar("SELECT config FROM panorama_colunas WHERE tabela = $1")
        .bind(&tabela)
        .fetch_optional(state.db()?)
        .await?;
    Ok(Json(config.unwrap_or(Value::Null)))
}

/// PUT /api/panorama/colunas/{tabela} — corpo `{"colunas": [["Material", true], ...]}`.
async fn gravar_colunas(
    State(state): State<AppState>,
    Path(tabela): Path<String>,
    headers: HeaderMap,
    Json(config): Json<Value>,
) -> Result<StatusCode, AppError> {
    tabela_valida(&tabela)?;
    if !config["colunas"].is_array() {
        return Err(AppError::BadRequest("Envie {\"colunas\": [[\"Nome\", true], ...]}.".into()));
    }
    sqlx::query(
        "INSERT INTO panorama_colunas (tabela, config) VALUES ($1, $2)
         ON CONFLICT (tabela) DO UPDATE SET config = EXCLUDED.config, atualizado_em = now()",
    )
    .bind(&tabela)
    .bind(&config)
    .execute(state.db()?)
    .await?;
    state.hub.dados_mudaram("colunas", origem(&headers));
    Ok(StatusCode::NO_CONTENT)
}

// ---- Simulações ------------------------------------------------------------

#[derive(Serialize, sqlx::FromRow)]
struct Simulacao {
    id: i64,
    cod_produto: i64,
    quantidade: f64,
}

#[derive(Deserialize)]
struct NovaSimulacao {
    cod_produto: i64,
    quantidade: f64,
}

/// GET /api/panorama/simulacoes — na ordem em que foram criadas (é a ordem em
/// que usam o estoque).
async fn listar_simulacoes(State(state): State<AppState>) -> Result<Json<Vec<Simulacao>>, AppError> {
    let lista = sqlx::query_as("SELECT id, cod_produto, quantidade FROM panorama_simulacoes ORDER BY id")
        .fetch_all(state.db()?)
        .await?;
    Ok(Json(lista))
}

/// POST /api/panorama/simulacoes — corpo `{"cod_produto": 1, "quantidade": 10}`.
async fn criar_simulacao(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(nova): Json<NovaSimulacao>,
) -> Result<(StatusCode, Json<Simulacao>), AppError> {
    if !(nova.quantidade.is_finite() && nova.quantidade > 0.0) {
        return Err(AppError::BadRequest("A quantidade deve ser maior que zero.".into()));
    }
    let s: Simulacao = sqlx::query_as(
        "INSERT INTO panorama_simulacoes (cod_produto, quantidade) VALUES ($1, $2)
         RETURNING id, cod_produto, quantidade",
    )
    .bind(nova.cod_produto)
    .bind(nova.quantidade)
    .fetch_one(state.db()?)
    .await?;
    state.hub.dados_mudaram("simulacoes", origem(&headers));
    Ok((StatusCode::CREATED, Json(s)))
}

/// DELETE /api/panorama/simulacoes/{id}
async fn excluir_simulacao(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Result<StatusCode, AppError> {
    let r = sqlx::query("DELETE FROM panorama_simulacoes WHERE id = $1").bind(id).execute(state.db()?).await?;
    if r.rows_affected() > 0 {
        state.hub.dados_mudaram("simulacoes", origem(&headers));
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---- Realizado parcial -------------------------------------------------------

#[derive(Serialize, Deserialize, sqlx::FromRow)]
struct Lancamento {
    op: i64,
    /// Posição do item na OP (0 = primeiro).
    linha: i32,
    cod_produto: i64,
    realizado: f64,
}

/// GET /api/panorama/parciais
async fn listar_parciais(State(state): State<AppState>) -> Result<Json<Vec<Lancamento>>, AppError> {
    let lista = sqlx::query_as("SELECT op, linha, cod_produto, realizado FROM panorama_parciais ORDER BY op, linha")
        .fetch_all(state.db()?)
        .await?;
    Ok(Json(lista))
}

/// PUT /api/panorama/parciais — corpo `{"op", "linha", "cod_produto", "realizado"}`.
async fn gravar_parcial(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(l): Json<Lancamento>,
) -> Result<StatusCode, AppError> {
    if !(l.realizado.is_finite() && l.realizado > 0.0) || l.linha < 0 {
        return Err(AppError::BadRequest("O realizado deve ser maior que zero (para apagar, use DELETE).".into()));
    }
    sqlx::query(
        "INSERT INTO panorama_parciais (op, linha, cod_produto, realizado) VALUES ($1, $2, $3, $4)
         ON CONFLICT (op, linha, cod_produto) DO UPDATE SET realizado = EXCLUDED.realizado, atualizado_em = now()",
    )
    .bind(l.op)
    .bind(l.linha)
    .bind(l.cod_produto)
    .bind(l.realizado)
    .execute(state.db()?)
    .await?;
    state.hub.dados_mudaram("parciais", origem(&headers));
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /api/panorama/parciais/{op}/{linha}/{cod_produto}
async fn excluir_parcial(
    State(state): State<AppState>,
    Path((op, linha, cod_produto)): Path<(i64, i32, i64)>,
    headers: HeaderMap,
) -> Result<StatusCode, AppError> {
    let r = sqlx::query("DELETE FROM panorama_parciais WHERE op = $1 AND linha = $2 AND cod_produto = $3")
        .bind(op)
        .bind(linha)
        .bind(cod_produto)
        .execute(state.db()?)
        .await?;
    if r.rows_affected() > 0 {
        state.hub.dados_mudaram("parciais", origem(&headers));
    }
    Ok(StatusCode::NO_CONTENT)
}
