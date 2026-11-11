//! Servidor geral da Biplas: fornece dados para os computadores da empresa
//! e recebe dados de diversas fontes.
//!
//! - Sincronização: busca eGestor e Trello, grava no PostgreSQL e avisa os
//!   programas conectados (`/api/sync`, `/api/notificacoes`)
//! - Dados gravados: `/api/dados/egestor/*`, `/api/trello/*`, `/api/panorama/*`
//! - eGestor (ERP): gateway da API, em `/api/egestor/*` (para escrita e consultas
//!   que não são sincronizadas)
//! - PostgreSQL: armazenamento próprio (eventos recebidos de outras fontes)

mod config;
mod db;
mod egestor;
mod error;
mod routes;
mod sincronizacao;
mod state;
mod trello;

use std::{net::SocketAddr, sync::Arc, time::Duration};

use axum::extract::DefaultBodyLimit;
use tower_http::{cors::CorsLayer, trace::TraceLayer};

use crate::{
    config::Config, egestor::EgestorClient, sincronizacao::Hub, state::AppState, trello::TrelloClient,
};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env().unwrap_or_else(|e| {
        eprintln!("\n[CONFIG] {e}\n");
        std::process::exit(1);
    });

    let db = match &config.database_url {
        Some(url) => match db::connect(url).await {
            Ok(pool) => {
                tracing::info!("PostgreSQL conectado, migrations aplicadas");
                Some(pool)
            }
            Err(e) => {
                eprintln!("\n[BANCO] Não foi possível conectar/migrar o PostgreSQL: {e}\n");
                std::process::exit(1);
            }
        },
        None => {
            tracing::warn!("DATABASE_URL não definida: rotas de banco responderão 503");
            None
        }
    };

    let egestor = config.egestor.as_ref().map(|c| {
        tracing::info!("eGestor: {}", c.base_url);
        Arc::new(EgestorClient::new(c.base_url.clone(), c.personal_token.clone()))
    });
    if egestor.is_none() {
        tracing::warn!("TOKENEGESTOR não definido: rotas /api/egestor responderão 503");
    }

    let trello = config.trello.as_ref().map(|c| {
        let client = TrelloClient::new(c);
        tracing::info!("Trello: quadro \"{}\" | listas: {}", client.board_name(), client.lists().join(" | "));
        Arc::new(client)
    });
    if trello.is_none() {
        tracing::warn!("TRELLO_KEY/TRELLO_TOKEN não definidos: rotas /api/trello responderão 503");
    }

    if config.api_key.is_none() {
        tracing::warn!("API_KEY não definida: /api está aberta para qualquer um na rede");
    }

    let state = AppState {
        db,
        egestor,
        trello,
        api_key: config.api_key.as_deref().map(Arc::from),
        hub: Arc::new(Hub::default()),
    };

    // Sincronização automática (precisa do banco e da integração configurada).
    if state.db.is_some() {
        let agendas = [
            (sincronizacao::EGESTOR, state.egestor.is_some(), config.sync_egestor_min),
            (sincronizacao::TRELLO, state.trello.is_some(), config.sync_trello_min),
        ];
        for (fonte, configurada, minutos) in agendas {
            match (configurada, minutos) {
                (true, Some(m)) => {
                    tracing::info!("Sincronização de {fonte}: agora e a cada {m} min");
                    sincronizacao::agendar(state.clone(), fonte, Duration::from_secs(m * 60));
                }
                (true, None) => tracing::info!("Sincronização de {fonte}: só quando um programa pedir"),
                (false, _) => {}
            }
        }
    }
    let hub = state.hub.clone();

    let app = routes::build(state)
        // Eleva o limite padrão (2 MB) para caber uploads de foto de produto
        // e disco virtual do eGestor (spec documenta até 10 MB).
        .layer(DefaultBodyLimit::max(20 * 1024 * 1024))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind((config.host.as_str(), config.port))
        .await
        .unwrap_or_else(|e| panic!("Não foi possível abrir {}:{}: {e}", config.host, config.port));

    tracing::info!("🚀 biplas-server ouvindo em http://{}:{}", config.host, config.port);

    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("Encerrando...");
            hub.desligar();
        })
        .await
        .expect("erro fatal servindo requisições");
}
