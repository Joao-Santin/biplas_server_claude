//! Integração com a API REST do eGestor (https://v4.egestor.com.br/www/api/docs/).
//!
//! Gateway fiel: cada rota `/api/egestor/{caminho}` é repassada para
//! `/v1/{caminho}` no eGestor com o mesmo método, query string, Content-Type
//! e corpo, e a resposta volta exatamente como veio (inclusive XML/PDF e
//! erros). A autenticação OAuth2 (personal_token → access_token de 15 min)
//! é feita e renovada automaticamente em `auth.rs`.
//!
//! As rotas aceitas estão na tabela `ROTAS` abaixo — só o que está nela é
//! repassado. Para liberar um endpoint novo do eGestor, basta adicionar uma
//! linha.

mod auth;
mod client;

pub use client::EgestorClient;

use axum::{
    Router,
    body::Bytes,
    extract::{OriginalUri, State},
    http::{HeaderMap, Method},
    response::Response,
    routing::{MethodFilter, on},
};

use crate::{error::AppError, state::AppState};

/// Prefixo sob o qual as rotas do eGestor ficam montadas no servidor.
pub const PREFIX: &str = "/api/egestor";

const GET: MethodFilter = MethodFilter::GET;
const POST: MethodFilter = MethodFilter::POST;
const PUT: MethodFilter = MethodFilter::PUT;
const DELETE: MethodFilter = MethodFilter::DELETE;

/// (caminho, métodos aceitos). O caminho é o mesmo do eGestor sem o `/v1`.
/// Filtros oficiais vão na query string (page, filtro, orderBy, dtIni, dtFim,
/// updatedAfter, fields...) e são repassados sem alteração.
const ROTAS: &[(&str, &[MethodFilter])] = &[
    // Cadastros
    ("/contatos", &[GET, POST]),
    ("/contatos/{codigo}", &[GET, PUT, DELETE]),
    ("/produtos", &[GET, POST]),
    ("/produtos/{codigo}", &[GET, PUT, DELETE]),
    ("/produtos/{codigo}/fotos", &[POST]), // multipart, campo `foto` (até 10 MB)
    ("/servicos", &[GET, POST]),
    ("/servicos/{codigo}", &[GET, PUT, DELETE]),
    ("/categorias", &[GET, POST]),
    ("/categorias/{codigo}", &[GET, PUT, DELETE]),
    ("/usuarios", &[GET]),
    ("/usuarios/{user_id}", &[GET]),
    ("/empresa", &[GET]),
    // Vendas (situacao: 10=Orçamento, 50=Venda; dtTipo: dtVenda|dtCad|dtEntrega)
    ("/vendas", &[GET, POST]),
    ("/vendas/{codigo}", &[GET, PUT, DELETE]),
    ("/vendas/{codigo}/contato", &[GET]),
    ("/vendas/{codigo}/gerarNfse", &[GET]),
    ("/vendas/{codigo}/gerarNfseEnviarEmail", &[GET]),
    ("/vendas/{codigo}/gerarNfe", &[POST]),
    ("/vendas/{codigo}/gerarNfce", &[POST]),
    ("/vendas/{codigo}/nfe", &[GET]),
    ("/vendas/{codigo}/mercadoLivre", &[GET]),
    ("/config/vendas/customizado", &[GET]),
    ("/devolucoes", &[GET, POST]),
    ("/devolucoes/{codigo}", &[GET]),
    // Compras e estoque
    ("/compras", &[GET, POST]),
    ("/compras/{codigo}", &[GET, DELETE]),
    ("/compras/{codigo}/efetivar", &[GET]),
    ("/compras/{codigo}/reabrir", &[GET]),
    ("/compras/{codigo}/nfe", &[GET]),
    ("/ajusteEstoque", &[GET, POST]),
    ("/ajusteEstoque/{codigo}", &[GET]),
    // Produção
    ("/producao", &[GET, POST]),
    ("/producao/{codigo}", &[GET, PUT, DELETE]),
    ("/producao/{codigo}/reabrir", &[GET]),
    // Financeiro
    ("/recebimentos", &[GET, POST]),
    ("/recebimentos/{codigo}", &[GET, PUT, DELETE]),
    ("/recebimentos/{codigo}/receber", &[PUT]),
    ("/pagamentos", &[GET, POST]),
    ("/pagamentos/{codigo}", &[GET, PUT, DELETE]),
    ("/pagamentos/{codigo}/pagar", &[PUT]),
    ("/pagamentos/{codigo}/contato", &[GET]),
    ("/boletos", &[GET, POST]),
    ("/boletos/{codigo}", &[GET]),
    ("/verificarRetornoBoletos", &[GET]),
    ("/pix", &[GET, POST]),
    ("/pix/credencial", &[GET]),
    ("/pix/{codigo}", &[GET]),
    ("/pix/{codigo}/consultarSituacao", &[GET]),
    ("/disponiveis", &[GET, POST]),
    ("/disponiveis/{codigo}", &[GET, PUT, DELETE]),
    ("/planoContas", &[GET, POST]),
    ("/planoContas/{codigo}", &[GET, PUT, DELETE]),
    ("/formasPagamento", &[GET, POST]),
    ("/formasPagamento/{codigo}", &[GET, PUT, DELETE]),
    // Fiscal
    ("/nfe", &[GET]),
    ("/nfe/salvar", &[POST]),
    ("/nfe/status", &[POST]),
    ("/nfe/recebidas", &[GET]),
    ("/nfe/buscarNotas", &[GET]),
    ("/nfe/{codigo}", &[GET]),
    ("/nfe/{codigo}/xml", &[GET]),   // binário
    ("/nfe/{codigo}/danfe", &[GET]), // PDF
    ("/nfe/recebidas/{id}/xml", &[GET]),
    ("/nfe/recebidas/{id}/danfe", &[GET]),
    ("/nfe/recebidas/{id}/manifestar", &[POST]),
    ("/nfe/recebidas/{id}/lancarCompra", &[POST]),
    ("/nfse", &[GET, POST]),
    ("/nfse/{codigo}", &[GET]),
    ("/configTrib", &[GET]),
    ("/configTrib/{codigo}", &[GET]),
    // Outros
    ("/discoVirtual", &[GET, POST]), // POST multipart
    ("/discoVirtual/{codigo}", &[GET, DELETE]),
    ("/webhooks", &[GET, POST]),
    ("/relatorios/{relatorio}", &[POST]), // ex.: vendasResumo, corpo {"de","ate"}
];

/// Router com todas as rotas da tabela, para ser aninhado em `PREFIX`.
pub fn router() -> Router<AppState> {
    ROTAS.iter().fold(Router::new(), |router, (path, methods)| {
        let filter = methods.iter().copied().reduce(MethodFilter::or).expect("rota sem método");
        router.route(path, on(filter, proxy))
    })
}

/// Handler único: o caminho no eGestor é `/v1` + o caminho pedido sem o prefixo.
async fn proxy(
    State(state): State<AppState>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    let egestor = state.egestor()?;
    let path = uri.path().strip_prefix(PREFIX).unwrap_or(uri.path());
    egestor
        .proxy(method, &format!("/v1{path}"), uri.query(), &headers, body)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todas_as_rotas_montam_sem_conflito() {
        // `Router::route` entra em pânico se houver caminhos conflitantes.
        let _ = router();
        let total: usize = ROTAS.iter().map(|(_, m)| m.len()).sum();
        assert_eq!(total, 117, "mesmo número de endpoints do egestor-rust-server");
    }
}
