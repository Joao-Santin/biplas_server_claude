//! Erros do PRÓPRIO servidor. Respostas de erro do eGestor não passam por
//! aqui: são repassadas como vieram (mesmo status e corpo).
//!
//! Todo erro vira `{"erro": "mensagem"}` com o status HTTP adequado.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};

#[derive(Debug)]
pub enum AppError {
    /// Falha de rede/protocolo ao falar com um serviço externo
    /// (timeout, DNS, TLS, conexão recusada...).
    Upstream { servico: &'static str, erro: reqwest::Error },
    /// O Trello respondeu com status de erro.
    Trello { status: u16, body: String },
    /// O eGestor respondeu com status de erro (só na sincronização; o proxy
    /// repassa a resposta como veio).
    Egestor { status: u16, body: String },
    /// Configuração inválida (ex.: quadro ou lista do Trello inexistente,
    /// falha ao autenticar no eGestor).
    Config(String),
    /// Integração ou banco de dados não configurado neste servidor.
    Indisponivel(String),
    /// Recurso pedido ao servidor não existe.
    NotFound(String),
    /// Requisição malformada.
    BadRequest(String),
    /// Falta ou está errado o header `x-api-key`.
    NaoAutorizado,
    /// Erro do PostgreSQL.
    Database(sqlx::Error),
    Internal(String),
}

impl AppError {
    pub fn upstream(servico: &'static str) -> impl FnOnce(reqwest::Error) -> Self {
        move |erro| AppError::Upstream { servico, erro }
    }
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        AppError::Database(e)
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::Upstream { servico, erro } => write!(f, "erro ao comunicar com {servico}: {erro}"),
            AppError::Trello { status: 401, body } => write!(
                f,
                "o Trello recusou a autenticação (401): {body}. \
                 Verifique TRELLO_KEY e TRELLO_TOKEN no .env."
            ),
            AppError::Trello { status, body } => write!(f, "o Trello respondeu {status}: {body}"),
            AppError::Egestor { status, body } => write!(f, "o eGestor respondeu {status}: {body}"),
            AppError::NaoAutorizado => write!(f, "header x-api-key ausente ou inválido"),
            AppError::Database(e) => write!(f, "erro no banco de dados: {e}"),
            AppError::Config(msg)
            | AppError::Indisponivel(msg)
            | AppError::NotFound(msg)
            | AppError::BadRequest(msg)
            | AppError::Internal(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for AppError {}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match &self {
            AppError::Upstream { .. } | AppError::Trello { .. } | AppError::Egestor { .. } => {
                StatusCode::BAD_GATEWAY
            }
            AppError::Indisponivel(_) => StatusCode::SERVICE_UNAVAILABLE,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::NaoAutorizado => StatusCode::UNAUTHORIZED,
            AppError::Config(_) | AppError::Database(_) | AppError::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        if status.is_server_error() {
            tracing::error!("{self}");
        }
        (status, Json(serde_json::json!({ "erro": self.to_string() }))).into_response()
    }
}
