//! Quem fez o quê: cada programa se identifica nos headers (`x-programa`,
//! `x-usuario`, `x-maquina`, além do `x-cliente-id`) e o servidor registra as
//! ações feitas pelos usuários na tabela `historico`. Também lista os
//! programas abertos agora (os conectados em `/api/notificacoes`).
//!
//! `x-usuario` e `x-maquina` vêm em percent-encoding (UTF-8), porque headers
//! HTTP só aceitam ASCII: "João" chega como "Jo%C3%A3o".

use std::net::SocketAddr;

use axum::{
    Json, Router,
    extract::{ConnectInfo, FromRequestParts, Query, State},
    http::{HeaderMap, request::Parts},
    routing::get,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    error::AppError,
    sincronizacao::{ClienteConectado, origem},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/historico", get(listar))
        .route("/historico/usuarios", get(usuarios))
        .route("/clientes", get(clientes))
}

/// Identificação de quem fez a requisição.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Autor {
    pub programa: Option<String>,
    pub usuario: Option<String>,
    pub maquina: Option<String>,
    pub ip: Option<String>,
    pub cliente_id: Option<String>,
}

impl Autor {
    pub fn dos_headers(headers: &HeaderMap, addr: Option<SocketAddr>) -> Self {
        let texto = |nome: &str| {
            headers
                .get(nome)
                .and_then(|v| v.to_str().ok())
                .map(decodificar)
                .map(|s| s.trim().chars().take(80).collect::<String>())
                .filter(|s| !s.is_empty())
        };
        Self {
            programa: texto("x-programa"),
            usuario: texto("x-usuario"),
            maquina: texto("x-maquina"),
            ip: addr.map(|a| a.ip().to_canonical().to_string()),
            cliente_id: origem(headers),
        }
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Autor {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let addr = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
        Ok(Self::dos_headers(&parts.headers, addr))
    }
}

/// Desfaz o percent-encoding ("Jo%C3%A3o" → "João"). Sequência inválida fica como veio.
pub fn decodificar(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(alto), Some(baixo)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push((alto * 16 + baixo) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// Grava a ação no histórico e avisa os programas (recurso `historico`).
/// Uma falha aqui só vai para o log: não desfaz a ação, que já foi feita.
pub async fn registrar(state: &AppState, autor: &Autor, acao: &str, detalhe: Value) {
    let Ok(db) = state.db() else { return };
    let r = sqlx::query(
        "INSERT INTO historico (programa, usuario, maquina, ip, cliente_id, acao, detalhe)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(&autor.programa)
    .bind(&autor.usuario)
    .bind(&autor.maquina)
    .bind(&autor.ip)
    .bind(&autor.cliente_id)
    .bind(acao)
    .bind(&detalhe)
    .execute(db)
    .await;
    match r {
        Ok(_) => state.hub.dados_mudaram("historico", None),
        Err(e) => tracing::error!("Não consegui registrar \"{acao}\" no histórico: {e}"),
    }
}

/// Como `registrar`, mas junta mudanças seguidas no mesmo item feitas pelo
/// mesmo programa em até 2 minutos: digitar "1", "12", "120" no realizado vira
/// um registro só, "de X para 120". `chave` identifica o item (ex.:
/// `{"op": 1, "linha": 0, "cod_produto": 5}`); `inicial`/`final_` são os campos
/// do valor de antes e de depois. Se voltar ao valor inicial, o registro some.
pub async fn registrar_agrupado(
    state: &AppState,
    autor: &Autor,
    acao: &str,
    chave: Value,
    (inicial, final_): (&str, &str),
    mut detalhe: Value,
) {
    let (Ok(db), Some(cliente)) = (state.db(), &autor.cliente_id) else {
        return registrar(state, autor, acao, detalhe).await;
    };
    let anterior: Result<Option<(i64, Value)>, _> = sqlx::query_as(
        "SELECT id, detalhe -> $4 FROM historico
         WHERE id = (SELECT max(id) FROM historico WHERE cliente_id = $1)
           AND acao = $2 AND detalhe @> $3 AND em > now() - interval '2 minutes'",
    )
    .bind(cliente)
    .bind(acao)
    .bind(&chave)
    .bind(inicial)
    .fetch_optional(db)
    .await;
    let Ok(Some((id, valor_inicial))) = anterior else {
        return registrar(state, autor, acao, detalhe).await;
    };
    detalhe[inicial] = valor_inicial;
    // 120 e 120.0 são o mesmo valor (o banco pode devolver um ou outro).
    let (a, b) = (&detalhe[inicial], &detalhe[final_]);
    let voltou = a == b || a.as_f64().is_some_and(|x| Some(x) == b.as_f64());
    let r = if voltou {
        sqlx::query("DELETE FROM historico WHERE id = $1").bind(id).execute(db).await
    } else {
        sqlx::query("UPDATE historico SET detalhe = $2, em = now() WHERE id = $1")
            .bind(id)
            .bind(&detalhe)
            .execute(db)
            .await
    };
    match r {
        Ok(_) => state.hub.dados_mudaram("historico", None),
        Err(e) => tracing::error!("Não consegui atualizar \"{acao}\" no histórico: {e}"),
    }
}

#[derive(Serialize, sqlx::FromRow)]
struct Registro {
    id: i64,
    em: DateTime<Utc>,
    programa: Option<String>,
    usuario: Option<String>,
    maquina: Option<String>,
    ip: Option<String>,
    acao: String,
    detalhe: Value,
}

#[derive(Deserialize)]
struct Filtro {
    usuario: Option<String>,
    programa: Option<String>,
    /// Só os registros com id menor (para paginar: "carregar mais").
    antes_de: Option<i64>,
    limite: Option<i64>,
}

/// GET /api/historico?usuario=&programa=&antes_de=&limite=100 — mais recentes primeiro.
async fn listar(State(state): State<AppState>, Query(f): Query<Filtro>) -> Result<Json<Vec<Registro>>, AppError> {
    let lista = sqlx::query_as(
        "SELECT id, em, programa, usuario, maquina, ip, acao, detalhe FROM historico
         WHERE ($1::text IS NULL OR usuario = $1)
           AND ($2::text IS NULL OR programa = $2)
           AND ($3::bigint IS NULL OR id < $3)
         ORDER BY id DESC LIMIT $4",
    )
    .bind(f.usuario.filter(|s| !s.is_empty()))
    .bind(f.programa.filter(|s| !s.is_empty()))
    .bind(f.antes_de)
    .bind(f.limite.unwrap_or(100).clamp(1, 1000))
    .fetch_all(state.db()?)
    .await?;
    Ok(Json(lista))
}

#[derive(Deserialize)]
struct FiltroPrograma {
    programa: Option<String>,
}

/// GET /api/historico/usuarios?programa= — nomes que já aparecem no histórico.
async fn usuarios(
    State(state): State<AppState>,
    Query(f): Query<FiltroPrograma>,
) -> Result<Json<Vec<String>>, AppError> {
    let nomes = sqlx::query_scalar(
        "SELECT DISTINCT usuario FROM historico
         WHERE usuario IS NOT NULL AND ($1::text IS NULL OR programa = $1) ORDER BY usuario",
    )
    .bind(f.programa.filter(|s| !s.is_empty()))
    .fetch_all(state.db()?)
    .await?;
    Ok(Json(nomes))
}

/// GET /api/clientes?programa= — programas abertos agora (conectados aos avisos).
async fn clientes(State(state): State<AppState>, Query(f): Query<FiltroPrograma>) -> Json<Vec<ClienteConectado>> {
    let programa = f.programa.filter(|s| !s.is_empty());
    Json(
        state
            .hub
            .clientes()
            .into_iter()
            .filter(|c| programa.is_none() || c.autor.programa == programa)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodifica_percent_encoding() {
        assert_eq!(decodificar("Jo%C3%A3o%20Silva"), "João Silva");
        assert_eq!(decodificar("sem-codigo"), "sem-codigo");
        assert_eq!(decodificar("100%"), "100%");
        assert_eq!(decodificar("%zz"), "%zz");
    }
}
