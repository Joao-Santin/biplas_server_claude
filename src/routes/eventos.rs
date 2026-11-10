//! Recebimento de dados de qualquer fonte (webhooks, máquinas, outros
//! programas da rede) e consulta do que foi recebido. Tudo no PostgreSQL.

use std::net::SocketAddr;

use axum::{
    Json, Router,
    extract::{ConnectInfo, Path, Query, State},
    http::StatusCode,
    routing::{get, post, put},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{error::AppError, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/fontes", get(listar_fontes))
        .route("/eventos", get(listar_eventos))
        .route("/eventos/{fonte}", post(receber))
        .route("/eventos/{id}/processado", put(marcar_processado))
}

#[derive(Serialize, sqlx::FromRow)]
struct Fonte {
    id: i16,
    nome: String,
    descricao: String,
    ativa: bool,
    criado_em: DateTime<Utc>,
}

#[derive(Serialize, sqlx::FromRow)]
struct Evento {
    id: i64,
    fonte: String,
    tipo: String,
    payload: Value,
    origem: Option<String>,
    recebido_em: DateTime<Utc>,
    processado_em: Option<DateTime<Utc>>,
    erro: Option<String>,
}

/// GET /api/fontes
async fn listar_fontes(State(state): State<AppState>) -> Result<Json<Vec<Fonte>>, AppError> {
    let fontes = sqlx::query_as::<_, Fonte>(
        "SELECT id, nome, descricao, ativa, criado_em FROM fontes ORDER BY nome",
    )
    .fetch_all(state.db()?)
    .await?;
    Ok(Json(fontes))
}

#[derive(Deserialize)]
struct ReceberQuery {
    #[serde(default)]
    tipo: String,
}

/// POST /api/eventos/{fonte}?tipo=... — guarda o corpo JSON como um evento.
/// A fonte é cadastrada automaticamente no primeiro envio.
async fn receber(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(fonte): Path<String>,
    Query(q): Query<ReceberQuery>,
    Json(payload): Json<Value>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let fonte = fonte.to_lowercase();
    let valido = (1..=64).contains(&fonte.len())
        && fonte.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if !valido {
        return Err(AppError::BadRequest(
            "Nome de fonte inválido: use até 64 letras minúsculas, dígitos, '-' ou '_'.".into(),
        ));
    }

    let db = state.db()?;
    let mut tx = db.begin().await?;

    let (fonte_id, ativa): (i16, bool) = sqlx::query_as(
        "INSERT INTO fontes (nome) VALUES ($1)
         ON CONFLICT (nome) DO UPDATE SET nome = EXCLUDED.nome
         RETURNING id, ativa",
    )
    .bind(&fonte)
    .fetch_one(&mut *tx)
    .await?;
    if !ativa {
        return Err(AppError::BadRequest(format!("A fonte \"{fonte}\" está desativada.")));
    }

    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO eventos (fonte_id, tipo, payload, origem) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(fonte_id)
    .bind(&q.tipo)
    .bind(&payload)
    .bind(addr.ip().to_string())
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({ "id": id }))))
}

#[derive(Deserialize)]
struct ListarQuery {
    fonte: Option<String>,
    tipo: Option<String>,
    /// `true` = só os ainda não processados.
    #[serde(default)]
    pendentes: bool,
    /// Padrão 50, máximo 500.
    limite: Option<i64>,
}

/// GET /api/eventos?fonte=&tipo=&pendentes=true&limite=50 — mais recentes primeiro.
async fn listar_eventos(
    State(state): State<AppState>,
    Query(q): Query<ListarQuery>,
) -> Result<Json<Vec<Evento>>, AppError> {
    let eventos = sqlx::query_as::<_, Evento>(
        "SELECT e.id, f.nome AS fonte, e.tipo, e.payload, e.origem,
                e.recebido_em, e.processado_em, e.erro
         FROM eventos e JOIN fontes f ON f.id = e.fonte_id
         WHERE ($1::text IS NULL OR f.nome = $1)
           AND ($2::text IS NULL OR e.tipo = $2)
           AND (NOT $3 OR e.processado_em IS NULL)
         ORDER BY e.recebido_em DESC, e.id DESC
         LIMIT $4",
    )
    .bind(q.fonte)
    .bind(q.tipo)
    .bind(q.pendentes)
    .bind(q.limite.unwrap_or(50).clamp(1, 500))
    .fetch_all(state.db()?)
    .await?;
    Ok(Json(eventos))
}

#[derive(Deserialize, Default)]
struct ProcessadoBody {
    /// Mensagem de erro, se o processamento falhou.
    erro: Option<String>,
}

/// PUT /api/eventos/{id}/processado — corpo opcional `{"erro": "..."}`.
async fn marcar_processado(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Option<Json<ProcessadoBody>>,
) -> Result<StatusCode, AppError> {
    let erro = body.map(|Json(b)| b).unwrap_or_default().erro;
    let r = sqlx::query("UPDATE eventos SET processado_em = now(), erro = $2 WHERE id = $1")
        .bind(id)
        .bind(erro)
        .execute(state.db()?)
        .await?;
    if r.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("Evento {id} não existe.")));
    }
    Ok(StatusCode::NO_CONTENT)
}
