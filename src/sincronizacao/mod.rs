//! Sincronização: o servidor busca os dados no eGestor e no Trello, grava no
//! PostgreSQL e os programas da rede leem do banco (nunca das APIs).
//!
//! - Roda sozinha a cada `SYNC_EGESTOR_MIN` / `SYNC_TRELLO_MIN` minutos e
//!   quando um programa pede (`POST /api/sync/{fonte}`).
//! - Só uma sincronização de cada fonte por vez; um pedido durante a execução
//!   não começa outra.
//! - Andamento e conclusão são avisados a todos os programas conectados em
//!   `GET /api/notificacoes` (Server-Sent Events), assim como as mudanças nos
//!   dados gravados pelos próprios programas (simulações, configurações...).

pub mod egestor;
pub mod trello;

use std::{
    collections::HashMap,
    convert::Infallible,
    sync::Mutex,
    time::Duration,
};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use futures_util::{Stream, StreamExt, stream};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{broadcast, watch};

use crate::{error::AppError, state::AppState};

pub const EGESTOR: &str = "egestor";
pub const TRELLO: &str = "trello";
const FONTES: [&str; 2] = [EGESTOR, TRELLO];

/// O que é enviado em `GET /api/notificacoes`, como JSON no campo `data`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "tipo", rename_all = "snake_case")]
pub enum Notificacao {
    /// Primeira mensagem de cada conexão: quem (re)conecta deve recarregar tudo,
    /// pois pode ter perdido avisos enquanto estava desconectado.
    Conectado,
    /// O programa ficou para trás e avisos foram descartados: recarregue tudo.
    Atrasado,
    /// Andamento de uma sincronização.
    Sync {
        fonte: &'static str,
        estado: Estado,
        etapa: String,
        /// 0.0 ..= 1.0
        fracao: f32,
        #[serde(skip_serializing_if = "Option::is_none")]
        erro: Option<String>,
    },
    /// Um recurso gravado no banco mudou; quem o mostra deve recarregá-lo.
    Dados {
        recurso: &'static str,
        /// `x-cliente-id` de quem fez a mudança (ele mesmo pode ignorar o aviso).
        #[serde(skip_serializing_if = "Option::is_none")]
        origem: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Estado {
    Andamento,
    Concluido,
    Erro,
}

#[derive(Debug, Clone, Serialize)]
pub struct Andamento {
    pub etapa: String,
    pub fracao: f32,
}

/// Central de avisos e controle de quem está sincronizando.
pub struct Hub {
    tx: broadcast::Sender<Notificacao>,
    em_andamento: Mutex<HashMap<&'static str, Andamento>>,
    desligar: watch::Sender<bool>,
}

impl Default for Hub {
    fn default() -> Self {
        Self {
            tx: broadcast::channel(512).0,
            em_andamento: Mutex::default(),
            desligar: watch::channel(false).0,
        }
    }
}

impl Hub {
    pub fn notificar(&self, n: Notificacao) {
        // Erro = ninguém conectado no momento; não é problema.
        let _ = self.tx.send(n);
    }

    pub fn dados_mudaram(&self, recurso: &'static str, origem: Option<String>) {
        self.notificar(Notificacao::Dados { recurso, origem });
    }

    /// Atualiza o andamento de uma sincronização em execução.
    pub fn progresso(&self, fonte: &'static str, etapa: impl Into<String>, fracao: f32) {
        let a = Andamento { etapa: etapa.into(), fracao: fracao.clamp(0.0, 1.0) };
        self.em_andamento.lock().expect("lock").insert(fonte, a.clone());
        self.notificar(Notificacao::Sync { fonte, estado: Estado::Andamento, etapa: a.etapa, fracao: a.fracao, erro: None });
    }

    fn andamento(&self, fonte: &str) -> Option<Andamento> {
        self.em_andamento.lock().expect("lock").get(fonte).cloned()
    }

    /// Marca a fonte como em sincronização. `false` se já estava.
    fn iniciar(&self, fonte: &'static str) -> bool {
        let mut m = self.em_andamento.lock().expect("lock");
        if m.contains_key(fonte) {
            return false;
        }
        m.insert(fonte, Andamento { etapa: "Iniciando...".into(), fracao: 0.0 });
        drop(m);
        self.notificar(Notificacao::Sync {
            fonte,
            estado: Estado::Andamento,
            etapa: "Iniciando...".into(),
            fracao: 0.0,
            erro: None,
        });
        true
    }

    fn terminar(&self, fonte: &'static str, erro: Option<String>) {
        self.em_andamento.lock().expect("lock").remove(fonte);
        let (estado, etapa) = match &erro {
            None => (Estado::Concluido, "Concluído"),
            Some(_) => (Estado::Erro, "Falhou"),
        };
        self.notificar(Notificacao::Sync { fonte, estado, etapa: etapa.into(), fracao: 1.0, erro });
    }

    /// Avisa as conexões de notificação para encerrarem (senão o desligamento
    /// gracioso esperaria para sempre por elas).
    pub fn desligar(&self) {
        let _ = self.desligar.send(true);
    }
}

/// Começa a sincronização em segundo plano. `false` se ela já estava rodando.
pub fn disparar(state: &AppState, fonte: &'static str) -> Result<bool, AppError> {
    verificar_configurada(state, fonte)?;
    if !state.hub.iniciar(fonte) {
        return Ok(false);
    }
    tokio::spawn(executar(state.clone(), fonte));
    Ok(true)
}

fn verificar_configurada(state: &AppState, fonte: &str) -> Result<(), AppError> {
    state.db()?;
    match fonte {
        EGESTOR => state.egestor().map(|_| ()),
        TRELLO => state.trello().map(|_| ()),
        _ => Err(AppError::NotFound(format!("Fonte \"{fonte}\" não é sincronizada. Use egestor ou trello."))),
    }
}

/// Roda uma sincronização já marcada com `Hub::iniciar` e registra o resultado.
async fn executar(state: AppState, fonte: &'static str) {
    let inicio = Utc::now();
    tracing::info!("Sincronizando {fonte}...");
    let r = match fonte {
        EGESTOR => egestor::sincronizar(&state).await,
        _ => trello::sincronizar(&state).await,
    };
    let (ok, erro, resumo) = match r {
        Ok(resumo) => {
            tracing::info!("{fonte} sincronizado: {resumo}");
            (true, None, resumo)
        }
        Err(e) => {
            tracing::error!("Falha ao sincronizar {fonte}: {e}");
            (false, Some(e.to_string()), json!({}))
        }
    };
    if let Ok(db) = state.db() {
        let gravado = sqlx::query(
            "INSERT INTO sincronizacoes (fonte, iniciado_em, ok, erro, resumo) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(fonte)
        .bind(inicio)
        .bind(ok)
        .bind(&erro)
        .bind(&resumo)
        .execute(db)
        .await;
        if let Err(e) = gravado {
            tracing::error!("Não consegui registrar a sincronização de {fonte}: {e}");
        }
    }
    state.hub.terminar(fonte, erro);
}

/// Sincroniza agora e depois a cada `intervalo`, para sempre.
pub fn agendar(state: AppState, fonte: &'static str, intervalo: Duration) {
    tokio::spawn(async move {
        let mut relogio = tokio::time::interval(intervalo);
        relogio.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            relogio.tick().await;
            // Se um programa pediu e ela já está rodando, este ciclo é pulado.
            if state.hub.iniciar(fonte) {
                executar(state.clone(), fonte).await;
            }
        }
    });
}

/// `x-cliente-id` enviado pelo programa que fez a requisição.
pub fn origem(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-cliente-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.chars().take(64).collect())
}

// ---- Rotas -----------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sync", get(status))
        .route("/sync/{fonte}", post(pedir))
        .route("/notificacoes", get(notificacoes))
}

/// POST /api/sync/{fonte} — pede uma sincronização. Responde na hora (202);
/// o andamento chega por `/api/notificacoes`.
async fn pedir(
    State(state): State<AppState>,
    Path(fonte): Path<String>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let Some(fonte) = FONTES.into_iter().find(|f| *f == fonte) else {
        return Err(AppError::NotFound(format!("Fonte \"{fonte}\" não é sincronizada. Use egestor ou trello.")));
    };
    let iniciada = disparar(&state, fonte)?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "iniciada": iniciada, "em_andamento": true }))))
}

#[derive(Serialize)]
struct StatusFonte {
    configurada: bool,
    em_andamento: Option<Andamento>,
    /// Última execução (com sucesso ou não).
    ultima: Option<Execucao>,
    /// Quando terminou a última sincronização bem-sucedida: é "de quando" são os dados.
    ultima_ok_em: Option<DateTime<Utc>>,
}

#[derive(Serialize, sqlx::FromRow)]
struct Execucao {
    #[serde(skip)]
    fonte: String,
    iniciado_em: DateTime<Utc>,
    concluido_em: DateTime<Utc>,
    ok: bool,
    erro: Option<String>,
    resumo: Value,
}

/// GET /api/sync — situação da sincronização de cada fonte.
async fn status(State(state): State<AppState>) -> Result<Json<HashMap<&'static str, StatusFonte>>, AppError> {
    let db = state.db()?;
    let ultimas: Vec<Execucao> = sqlx::query_as(
        "SELECT DISTINCT ON (fonte) fonte, iniciado_em, concluido_em, ok, erro, resumo
         FROM sincronizacoes ORDER BY fonte, concluido_em DESC",
    )
    .fetch_all(db)
    .await?;
    let ok_em: Vec<(String, DateTime<Utc>)> =
        sqlx::query_as("SELECT fonte, max(concluido_em) FROM sincronizacoes WHERE ok GROUP BY fonte")
            .fetch_all(db)
            .await?;

    let mut ultimas: HashMap<String, Execucao> = ultimas.into_iter().map(|e| (e.fonte.clone(), e)).collect();
    let ok_em: HashMap<String, DateTime<Utc>> = ok_em.into_iter().collect();
    let saida = FONTES
        .into_iter()
        .map(|f| {
            let s = StatusFonte {
                configurada: verificar_configurada(&state, f).is_ok(),
                em_andamento: state.hub.andamento(f),
                ultima: ultimas.remove(f),
                ultima_ok_em: ok_em.get(f).copied(),
            };
            (f, s)
        })
        .collect();
    Ok(Json(saida))
}

fn evento(n: &Notificacao) -> Result<Event, Infallible> {
    Ok(Event::default().data(serde_json::to_string(n).expect("notificação serializável")))
}

/// GET /api/notificacoes — Server-Sent Events. Cada mensagem é um JSON com
/// `tipo`: `conectado`, `atrasado`, `sync` ou `dados` (ver `Notificacao`).
async fn notificacoes(State(state): State<AppState>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // Assina antes de tirar a foto do andamento, para não perder nada no meio.
    let rx = state.hub.tx.subscribe();
    let em_andamento: Vec<Notificacao> = {
        let m = state.hub.em_andamento.lock().expect("lock");
        m.iter()
            .map(|(fonte, a)| Notificacao::Sync {
                fonte,
                estado: Estado::Andamento,
                etapa: a.etapa.clone(),
                fracao: a.fracao,
                erro: None,
            })
            .collect()
    };
    let inicio = stream::iter(std::iter::once(Notificacao::Conectado).chain(em_andamento)).map(|n| evento(&n));
    let avisos = stream::unfold(rx, |mut rx| async move {
        let n = match rx.recv().await {
            Ok(n) => n,
            Err(broadcast::error::RecvError::Lagged(_)) => Notificacao::Atrasado,
            Err(broadcast::error::RecvError::Closed) => return None,
        };
        Some((evento(&n), rx))
    });

    let mut desligar = state.hub.desligar.subscribe();
    let fim = async move {
        let _ = desligar.wait_for(|d| *d).await;
    };
    Sse::new(inicio.chain(avisos).take_until(Box::pin(fim))).keep_alive(KeepAlive::default())
}
