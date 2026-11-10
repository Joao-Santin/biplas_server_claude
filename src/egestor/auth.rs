use std::time::{Duration, Instant};

use reqwest::Client;
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::error::AppError;

/// Margem de segurança: renovamos o token um pouco antes dele expirar de
/// verdade, para não correr o risco de usar um access_token que vence no
/// meio de uma requisição.
const SAFETY_MARGIN_SECS: u64 = 30;

/// Corpo de resposta de `POST /oauth/access_token`.
/// (`expires_in` vem em segundos; no spec, tipicamente 900 = 15 min.)
#[derive(Debug, Deserialize)]
struct OAuthResponse {
    access_token: String,
    #[serde(default)]
    expires_in: Option<u64>,
}

struct TokenState {
    access_token: String,
    expires_at: Instant,
}

/// Troca o `personal_token` (JWT, de longa duração, vindo do .env) por um
/// `access_token` (válido ~15 min) e mantém esse access_token em cache,
/// renovando automaticamente quando necessário. Como a API só documenta o
/// grant_type "personal" (não um grant "refresh_token" explícito), a
/// renovação é feita reautenticando com o próprio personal_token — é simples
/// e sempre funciona, sem depender de um fluxo não documentado.
pub struct AuthManager {
    http: Client,
    base_url: String,
    personal_token: String,
    state: RwLock<Option<TokenState>>,
}

impl AuthManager {
    pub fn new(http: Client, base_url: String, personal_token: String) -> Self {
        Self {
            http,
            base_url,
            personal_token,
            state: RwLock::new(None),
        }
    }

    /// Retorna um access_token válido, reaproveitando o token em cache
    /// sempre que possível.
    pub async fn get_access_token(&self) -> Result<String, AppError> {
        if let Some(token) = self.read_valid_token().await {
            return Ok(token);
        }
        self.refresh(false).await
    }

    /// Descarta o token em cache e força uma nova autenticação — mesmo que,
    /// pelo nosso relógio local, o token em cache ainda parecesse válido.
    /// Usado quando o eGestor responde 401 de qualquer forma (ex.: token
    /// revogado manualmente antes do prazo).
    pub async fn force_refresh(&self) -> Result<String, AppError> {
        self.refresh(true).await
    }

    async fn read_valid_token(&self) -> Option<String> {
        let guard = self.state.read().await;
        guard.as_ref().and_then(|t| {
            if t.expires_at > Instant::now() {
                Some(t.access_token.clone())
            } else {
                None
            }
        })
    }

    async fn refresh(&self, force: bool) -> Result<String, AppError> {
        let mut guard = self.state.write().await;

        // Outra requisição pode ter renovado o token enquanto esperávamos
        // o lock de escrita — evita autenticar em duplicidade. Pulado
        // quando `force=true` (chamada via force_refresh, ex.: após 401),
        // já que nesse caso o token em cache é justamente suspeito.
        if !force {
            if let Some(t) = guard.as_ref() {
                if t.expires_at > Instant::now() {
                    return Ok(t.access_token.clone());
                }
            }
        }

        tracing::info!("Renovando access_token do eGestor via personal_token...");

        let resp = self
            .http
            .post(format!("{}/oauth/access_token", self.base_url))
            .json(&serde_json::json!({
                "grant_type": "personal",
                "personal_token": self.personal_token,
            }))
            .send()
            .await
            .map_err(AppError::upstream("o eGestor"))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::Config(format!(
                "Falha ao autenticar com o eGestor ({status}): {body}. \
                 Verifique se o TOKENEGESTOR no seu .env está correto e ainda válido."
            )));
        }

        let parsed: OAuthResponse = resp.json().await.map_err(AppError::upstream("o eGestor"))?;

        let ttl_secs = parsed.expires_in.unwrap_or(900);
        let ttl_secs = ttl_secs.saturating_sub(SAFETY_MARGIN_SECS).max(30);
        let expires_at = Instant::now() + Duration::from_secs(ttl_secs);

        let token = parsed.access_token.clone();
        *guard = Some(TokenState {
            access_token: parsed.access_token,
            expires_at,
        });

        tracing::info!("access_token renovado (válido por ~{ttl_secs}s)");
        Ok(token)
    }
}
