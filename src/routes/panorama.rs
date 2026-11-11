//! Dados do Panorama de Injetados guardados no banco e compartilhados por
//! todos os computadores: configuração de colunas (de cada usuário), simulações e realizado
//! parcial das OPs. Toda mudança é avisada em `/api/notificacoes`
//! (`{"tipo":"dados","recurso":...}`), para os outros panoramas abertos
//! recarregarem.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::types::Json as SqlJson;

use crate::{
    error::AppError,
    routes::historico::{Autor, registrar, registrar_agrupado},
    sincronizacao::origem,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/panorama/colunas/{tabela}", get(ler_colunas).put(gravar_colunas))
        .route("/panorama/simulacoes", get(listar_simulacoes).post(criar_simulacao))
        .route("/panorama/simulacoes/{id}", delete(excluir_simulacao))
        .route("/panorama/parciais", get(listar_parciais).put(gravar_parcial))
        .route("/panorama/parciais/lancamentos", get(listar_confirmacoes))
        .route("/panorama/parciais/{op}/{linha}/{cod_produto}", delete(excluir_parcial))
}

// ---- Colunas -----------------------------------------------------------------
// Cada usuário (cabeçalho `x-usuario`) tem a sua disposição. Quem ainda não
// configurou parte da configuração comum de antes (`panorama_colunas`).

fn tabela_valida(tabela: &str) -> Result<(), AppError> {
    match tabela {
        "necessidades" | "produtos" => Ok(()),
        _ => Err(AppError::NotFound(format!("Tabela \"{tabela}\" não existe. Use necessidades ou produtos."))),
    }
}

/// Dono da configuração de colunas: o usuário; sem nome, a máquina.
fn dono_colunas(autor: &Autor) -> String {
    match (&autor.usuario, &autor.maquina) {
        (Some(u), _) => u.clone(),
        (None, Some(m)) => format!("@{m}"),
        (None, None) => String::new(),
    }
}

async fn config_colunas(state: &AppState, dono: &str, tabela: &str) -> Result<Option<Value>, AppError> {
    let propria: Option<Value> =
        sqlx::query_scalar("SELECT config FROM panorama_colunas_usuario WHERE usuario = $1 AND tabela = $2")
            .bind(dono)
            .bind(tabela)
            .fetch_optional(state.db()?)
            .await?;
    if propria.is_some() {
        return Ok(propria);
    }
    Ok(sqlx::query_scalar("SELECT config FROM panorama_colunas WHERE tabela = $1")
        .bind(tabela)
        .fetch_optional(state.db()?)
        .await?)
}

/// GET /api/panorama/colunas/{tabela} — `{"colunas": [...]}` de quem pede, ou
/// `null` se ainda não foi configurada (o programa usa o padrão).
async fn ler_colunas(State(state): State<AppState>, Path(tabela): Path<String>, autor: Autor) -> Result<Json<Value>, AppError> {
    tabela_valida(&tabela)?;
    let config = config_colunas(&state, &dono_colunas(&autor), &tabela).await?;
    Ok(Json(config.unwrap_or(Value::Null)))
}

/// PUT /api/panorama/colunas/{tabela} — corpo `{"colunas": [["Material", true], ...]}`.
/// Grava só para quem pede.
async fn gravar_colunas(
    State(state): State<AppState>,
    Path(tabela): Path<String>,
    headers: HeaderMap,
    autor: Autor,
    Json(config): Json<Value>,
) -> Result<StatusCode, AppError> {
    tabela_valida(&tabela)?;
    if !config["colunas"].is_array() {
        return Err(AppError::BadRequest("Envie {\"colunas\": [[\"Nome\", true], ...]}.".into()));
    }
    let dono = dono_colunas(&autor);
    let antes = config_colunas(&state, &dono, &tabela).await?;
    sqlx::query(
        "INSERT INTO panorama_colunas_usuario (usuario, tabela, config) VALUES ($1, $2, $3)
         ON CONFLICT (usuario, tabela) DO UPDATE SET config = EXCLUDED.config, atualizado_em = now()",
    )
    .bind(&dono)
    .bind(&tabela)
    .bind(&config)
    .execute(state.db()?)
    .await?;
    state.hub.dados_mudaram("colunas", origem(&headers));
    // A configuração inteira (ordem, visibilidade e cores), para o histórico mostrar o que mudou.
    let detalhe = json!({ "tabela": tabela, "antes": antes.unwrap_or(Value::Null), "depois": config });
    registrar_agrupado(&state, &autor, "colunas", json!({ "tabela": tabela }), ("antes", "depois"), detalhe).await;
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
    autor: Autor,
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
    let detalhe = json!({ "id": s.id, "cod_produto": s.cod_produto, "quantidade": s.quantidade });
    registrar(&state, &autor, "simulacao_criada", detalhe).await;
    Ok((StatusCode::CREATED, Json(s)))
}

/// DELETE /api/panorama/simulacoes/{id}
async fn excluir_simulacao(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    autor: Autor,
) -> Result<StatusCode, AppError> {
    let apagada: Option<Simulacao> = sqlx::query_as(
        "DELETE FROM panorama_simulacoes WHERE id = $1 RETURNING id, cod_produto, quantidade",
    )
    .bind(id)
    .fetch_optional(state.db()?)
    .await?;
    if let Some(s) = apagada {
        state.hub.dados_mudaram("simulacoes", origem(&headers));
        let detalhe = json!({ "id": s.id, "cod_produto": s.cod_produto, "quantidade": s.quantidade });
        registrar(&state, &autor, "simulacao_excluida", detalhe).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---- Realizado parcial -------------------------------------------------------

/// Um tipo de volume: quantos volumes e quantas peças vão em cada um.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Grupo {
    volumes: f64,
    qtd_volume: f64,
}

#[derive(Serialize, Deserialize, sqlx::FromRow)]
struct Lancamento {
    op: i64,
    /// Posição do item na OP (0 = primeiro).
    linha: i32,
    cod_produto: i64,
    /// Total realizado da linha. Com `grupos`, o servidor recalcula: soma de volumes × qtd_volume.
    realizado: f64,
    /// Só quando há um tipo de volume (e nos lançamentos antigos).
    #[serde(default)]
    volumes: Option<f64>,
    #[serde(default)]
    qtd_volume: Option<f64>,
    #[serde(default)]
    grupos: Option<SqlJson<Vec<Grupo>>>,
}

/// Uma confirmação de realizado: o total novo e o de antes (0 = apagado / não havia).
#[derive(Serialize, sqlx::FromRow)]
struct Confirmacao {
    id: i64,
    op: i64,
    linha: i32,
    cod_produto: i64,
    em: DateTime<Utc>,
    volumes: Option<f64>,
    qtd_volume: Option<f64>,
    grupos: Option<SqlJson<Vec<Grupo>>>,
    realizado: f64,
    anterior: f64,
    usuario: Option<String>,
    maquina: Option<String>,
}

/// GET /api/panorama/parciais
async fn listar_parciais(State(state): State<AppState>) -> Result<Json<Vec<Lancamento>>, AppError> {
    let lista = sqlx::query_as(
        "SELECT op, linha, cod_produto, realizado, volumes, qtd_volume, grupos FROM panorama_parciais ORDER BY op, linha",
    )
    .fetch_all(state.db()?)
    .await?;
    Ok(Json(lista))
}

#[derive(Deserialize)]
struct FiltroOp {
    op: i64,
}

/// GET /api/panorama/parciais/lancamentos?op= — confirmações das linhas da OP, da mais antiga para a mais nova.
async fn listar_confirmacoes(
    State(state): State<AppState>,
    Query(f): Query<FiltroOp>,
) -> Result<Json<Vec<Confirmacao>>, AppError> {
    let lista = sqlx::query_as(
        "SELECT id, op, linha, cod_produto, em, volumes, qtd_volume, grupos, realizado, anterior, usuario, maquina
         FROM panorama_parciais_lancamentos WHERE op = $1 ORDER BY id",
    )
    .bind(f.op)
    .fetch_all(state.db()?)
    .await?;
    Ok(Json(lista))
}

/// Guarda a confirmação (para mostrar os deltas) e registra no histórico geral.
async fn registrar_parcial(state: &AppState, autor: &Autor, l: &Lancamento, anterior: f64) {
    if let Ok(db) = state.db() {
        let r = sqlx::query(
            "INSERT INTO panorama_parciais_lancamentos
                (op, linha, cod_produto, volumes, qtd_volume, grupos, realizado, anterior, usuario, maquina)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(l.op)
        .bind(l.linha)
        .bind(l.cod_produto)
        .bind(l.volumes)
        .bind(l.qtd_volume)
        .bind(&l.grupos)
        .bind(l.realizado)
        .bind(anterior)
        .bind(&autor.usuario)
        .bind(&autor.maquina)
        .execute(db)
        .await;
        if let Err(e) = r {
            tracing::error!("Não consegui guardar a confirmação do realizado da OP {}: {e}", l.op);
        }
    }
    let detalhe = json!({
        "op": l.op,
        "linha": l.linha,
        "cod_produto": l.cod_produto,
        "anterior": (anterior > 0.0).then_some(anterior),
        "realizado": (l.realizado > 0.0).then_some(l.realizado),
        "volumes": l.volumes,
        "qtd_volume": l.qtd_volume,
        "grupos": l.grupos.as_ref().map(|g| &g.0),
    });
    registrar(state, autor, "parcial", detalhe).await;
}

/// Confere os tipos de volume, tira os de zero volumes e acerta o total.
fn normalizar(l: &mut Lancamento) -> Result<(), AppError> {
    let Some(SqlJson(grupos)) = &mut l.grupos else { return Ok(()) };
    let valido = |g: &Grupo| g.volumes.is_finite() && g.volumes >= 0.0 && g.qtd_volume.is_finite() && g.qtd_volume > 0.0;
    if !grupos.iter().all(valido) {
        return Err(AppError::BadRequest(
            "Em cada tipo de volume, volumes deve ser zero ou mais e a quantidade por volume, maior que zero.".into(),
        ));
    }
    grupos.retain(|g| g.volumes > 0.0);
    l.realizado = grupos.iter().map(|g| g.volumes * g.qtd_volume).sum();
    (l.volumes, l.qtd_volume) = match grupos.as_slice() {
        [g] => (Some(g.volumes), Some(g.qtd_volume)),
        _ => (None, None),
    };
    Ok(())
}

/// PUT /api/panorama/parciais — corpo `{"op", "linha", "cod_produto", "realizado", "grupos"?: [{"volumes", "qtd_volume"}]}`.
/// O realizado é sempre o total da linha (com `grupos`, a soma deles).
async fn gravar_parcial(
    State(state): State<AppState>,
    headers: HeaderMap,
    autor: Autor,
    Json(mut l): Json<Lancamento>,
) -> Result<StatusCode, AppError> {
    normalizar(&mut l)?;
    if !(l.realizado.is_finite() && l.realizado > 0.0) || l.linha < 0 {
        return Err(AppError::BadRequest("O realizado deve ser maior que zero (para apagar, use DELETE).".into()));
    }
    if l.volumes.is_some_and(|v| !(v.is_finite() && v >= 0.0)) || l.qtd_volume.is_some_and(|q| !(q.is_finite() && q > 0.0)) {
        return Err(AppError::BadRequest("Volumes deve ser zero ou mais e a quantidade por volume, maior que zero.".into()));
    }
    type Salvo = (f64, Option<f64>, Option<f64>, Option<SqlJson<Vec<Grupo>>>);
    let anterior: Option<Salvo> = sqlx::query_as(
        "SELECT realizado, volumes, qtd_volume, grupos FROM panorama_parciais
         WHERE op = $1 AND linha = $2 AND cod_produto = $3",
    )
    .bind(l.op)
    .bind(l.linha)
    .bind(l.cod_produto)
    .fetch_optional(state.db()?)
    .await?;
    sqlx::query(
        "INSERT INTO panorama_parciais (op, linha, cod_produto, realizado, volumes, qtd_volume, grupos)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (op, linha, cod_produto) DO UPDATE SET realizado = EXCLUDED.realizado,
             volumes = EXCLUDED.volumes, qtd_volume = EXCLUDED.qtd_volume, grupos = EXCLUDED.grupos,
             atualizado_em = now()",
    )
    .bind(l.op)
    .bind(l.linha)
    .bind(l.cod_produto)
    .bind(l.realizado)
    .bind(l.volumes)
    .bind(l.qtd_volume)
    .bind(&l.grupos)
    .execute(state.db()?)
    .await?;
    state.hub.dados_mudaram("parciais", origem(&headers));
    let grupos_novos = l.grupos.as_ref().map(|g| &g.0);
    let mudou = anterior
        .as_ref()
        .is_none_or(|(r, v, q, g)| (*r, *v, *q, g.as_ref().map(|g| &g.0)) != (l.realizado, l.volumes, l.qtd_volume, grupos_novos));
    if mudou {
        registrar_parcial(&state, &autor, &l, anterior.map_or(0.0, |a| a.0)).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /api/panorama/parciais/{op}/{linha}/{cod_produto}
async fn excluir_parcial(
    State(state): State<AppState>,
    Path((op, linha, cod_produto)): Path<(i64, i32, i64)>,
    headers: HeaderMap,
    autor: Autor,
) -> Result<StatusCode, AppError> {
    let apagado: Option<f64> = sqlx::query_scalar(
        "DELETE FROM panorama_parciais WHERE op = $1 AND linha = $2 AND cod_produto = $3 RETURNING realizado",
    )
    .bind(op)
    .bind(linha)
    .bind(cod_produto)
    .fetch_optional(state.db()?)
    .await?;
    if let Some(anterior) = apagado {
        state.hub.dados_mudaram("parciais", origem(&headers));
        let zerado = Lancamento {
            op,
            linha,
            cod_produto,
            realizado: 0.0,
            volumes: None,
            qtd_volume: None,
            grupos: Some(SqlJson(Vec::new())),
        };
        registrar_parcial(&state, &autor, &zerado, anterior).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lanc(grupos: Vec<(f64, f64)>) -> Lancamento {
        Lancamento {
            op: 1,
            linha: 0,
            cod_produto: 5,
            realizado: 999.0,
            volumes: None,
            qtd_volume: None,
            grupos: Some(SqlJson(grupos.into_iter().map(|(volumes, qtd_volume)| Grupo { volumes, qtd_volume }).collect())),
        }
    }

    #[test]
    fn total_e_a_soma_dos_tipos_de_volume() {
        let mut l = lanc(vec![(1.0, 2000.0), (3.0, 1000.0), (0.0, 500.0)]);
        normalizar(&mut l).unwrap();
        assert_eq!(l.realizado, 5000.0, "o total enviado é ignorado");
        assert_eq!(l.grupos.as_ref().unwrap().0.len(), 2, "tipo com zero volumes sai");
        assert_eq!((l.volumes, l.qtd_volume), (None, None));

        let mut um = lanc(vec![(8.0, 500.0)]);
        normalizar(&mut um).unwrap();
        assert_eq!((um.realizado, um.volumes, um.qtd_volume), (4000.0, Some(8.0), Some(500.0)));

        assert!(normalizar(&mut lanc(vec![(1.0, 0.0)])).is_err());
        assert!(normalizar(&mut lanc(vec![(-1.0, 10.0)])).is_err());
    }
}
