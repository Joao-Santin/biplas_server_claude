use std::time::Duration;

use axum::{
    body::{Body, Bytes},
    http::{header::CONTENT_TYPE, HeaderMap, StatusCode},
    response::Response,
};
use reqwest::{Client, Method};

use super::auth::AuthManager;
use crate::error::AppError;

/// Cliente que fala com a API do eGestor em nome de todas as rotas.
///
/// A ideia central é a de um proxy fiel: recebemos método, path, query
/// string, headers e corpo já prontos (vindos da requisição original do
/// usuário do nosso servidor) e apenas:
///   1. injetamos o header `Authorization: Bearer <access_token>`;
///   2. repassamos tudo para a URL correspondente no eGestor;
///   3. devolvemos a resposta do eGestor (status, content-type e corpo)
///      exatamente como veio.
///
/// Isso vale tanto para respostas de sucesso quanto de erro (400/401/404/
/// 410/422/429 etc — o eGestor já documenta o formato `ErrorResponse` para
/// esses casos, não há necessidade de reinterpretar), e também para
/// respostas binárias (XML de NF-e, PDF de DANFE) e uploads multipart
/// (fotos de produto, disco virtual), já que nunca tentamos decodificar o
/// corpo — só repassamos os bytes brutos com o Content-Type original.
pub struct EgestorClient {
    http: Client,
    auth: AuthManager,
    base_url: String,
}

impl EgestorClient {
    pub fn new(base_url: String, personal_token: String) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("falha ao construir o cliente HTTP (reqwest)");

        let auth = AuthManager::new(http.clone(), base_url.clone(), personal_token);

        Self { http, auth, base_url }
    }

    /// Encaminha uma requisição para `{base_url}{upstream_path}?{raw_query}`.
    /// Se a primeira tentativa voltar 401, força a renovação do token e
    /// tenta mais uma única vez (cobre o caso de o token expirar bem no
    /// meio da janela entre a checagem local e o envio da requisição).
    pub async fn proxy(
        &self,
        method: Method,
        upstream_path: &str,
        raw_query: Option<&str>,
        incoming_headers: &HeaderMap,
        body: Bytes,
    ) -> Result<Response, AppError> {
        let token = self.auth.get_access_token().await?;
        let resp = self
            .send(&token, method.clone(), upstream_path, raw_query, incoming_headers, body.clone())
            .await?;

        if resp.status().as_u16() == 401 {
            tracing::warn!("eGestor respondeu 401; renovando access_token e tentando novamente (1x)");
            let fresh = self.auth.force_refresh().await?;
            let retried = self
                .send(&fresh, method, upstream_path, raw_query, incoming_headers, body)
                .await?;
            return Self::into_axum_response(retried).await;
        }

        Self::into_axum_response(resp).await
    }

    async fn send(
        &self,
        token: &str,
        method: Method,
        upstream_path: &str,
        raw_query: Option<&str>,
        incoming_headers: &HeaderMap,
        body: Bytes,
    ) -> Result<reqwest::Response, AppError> {
        let mut url = format!("{}{}", self.base_url, upstream_path);
        if let Some(q) = raw_query {
            if !q.is_empty() {
                url.push('?');
                url.push_str(q);
            }
        }

        tracing::debug!("{method} {url}");

        let mut builder = self
            .http
            .request(method, url)
            .header("Authorization", format!("Bearer {token}"));

        // Repassa o Content-Type original — essencial para uploads
        // multipart/form-data (o boundary vai dentro desse header).
        if let Some(ct) = incoming_headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok()) {
            builder = builder.header("Content-Type", ct);
        }

        if !body.is_empty() {
            builder = builder.body(body);
        }

        builder.send().await.map_err(AppError::upstream("o eGestor"))
    }

    /// Converte a `reqwest::Response` do eGestor numa `axum::response::Response`,
    /// preservando status, Content-Type e corpo (e os headers de rate limit,
    /// quando presentes) sem tentar interpretar o conteúdo.
    async fn into_axum_response(resp: reqwest::Response) -> Result<Response, AppError> {
        let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let rl_limit = resp
            .headers()
            .get("x-ratelimit-limit")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let rl_remaining = resp
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);

        let bytes = resp.bytes().await.map_err(AppError::upstream("o eGestor"))?;

        let mut builder = Response::builder().status(status);
        if let Some(ct) = content_type {
            builder = builder.header(CONTENT_TYPE, ct);
        }
        if let Some(v) = rl_limit {
            builder = builder.header("x-ratelimit-limit", v);
        }
        if let Some(v) = rl_remaining {
            builder = builder.header("x-ratelimit-remaining", v);
        }

        builder
            .body(Body::from(bytes))
            .map_err(|e| AppError::Internal(format!("falha ao montar resposta: {e}")))
    }
}
