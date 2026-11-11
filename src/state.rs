use std::sync::Arc;

use sqlx::PgPool;

use crate::{egestor::EgestorClient, error::AppError, sincronizacao::Hub, trello::TrelloClient};

/// Estado compartilhado entre todas as rotas via `axum::extract::State`.
/// Clonar é barato: só `Arc`s e o `PgPool` (que já é um `Arc` por dentro).
/// Cada peça é `None` quando não foi configurada no `.env`.
#[derive(Clone)]
pub struct AppState {
    pub db: Option<PgPool>,
    pub egestor: Option<Arc<EgestorClient>>,
    pub trello: Option<Arc<TrelloClient>>,
    pub api_key: Option<Arc<str>>,
    /// Avisos aos programas conectados e controle das sincronizações.
    pub hub: Arc<Hub>,
}

impl AppState {
    pub fn db(&self) -> Result<&PgPool, AppError> {
        self.db.as_ref().ok_or_else(|| {
            AppError::Indisponivel(
                "Banco de dados não configurado. Defina DATABASE_URL no .env.".into(),
            )
        })
    }

    pub fn egestor(&self) -> Result<&EgestorClient, AppError> {
        self.egestor.as_deref().ok_or_else(|| {
            AppError::Indisponivel(
                "Integração eGestor não configurada. Defina TOKENEGESTOR no .env.".into(),
            )
        })
    }

    pub fn trello(&self) -> Result<&TrelloClient, AppError> {
        self.trello.as_deref().ok_or_else(|| {
            AppError::Indisponivel(
                "Integração Trello não configurada. Defina TRELLO_KEY e TRELLO_TOKEN no .env."
                    .into(),
            )
        })
    }
}
