//! Configuração do servidor, lida do ambiente (.env).
//!
//! Cada integração é opcional: se as variáveis dela não estiverem definidas,
//! o servidor sobe mesmo assim e as rotas daquela integração respondem 503
//! explicando o que falta configurar.

pub struct Config {
    /// Endereço de escuta. `0.0.0.0` = acessível pelos computadores da rede.
    pub host: String,
    pub port: u16,
    /// Se definida, toda rota sob `/api` exige o header `x-api-key` com este valor.
    pub api_key: Option<String>,
    /// URL de conexão do PostgreSQL (ex.: postgres://usuario:senha@localhost/biplas).
    pub database_url: Option<String>,
    pub egestor: Option<EgestorConfig>,
    pub trello: Option<TrelloConfig>,
    /// De quantos em quantos minutos o servidor sincroniza sozinho cada fonte
    /// (`None` = só quando um programa pedir).
    pub sync_egestor_min: Option<u64>,
    pub sync_trello_min: Option<u64>,
}

pub struct EgestorConfig {
    /// O `personal_token` (JWT) do eGestor — o que vai em TOKENEGESTOR.
    pub personal_token: String,
    /// URL base da API (ex.: https://v4.egestor.com.br/api), sem barra final.
    pub base_url: String,
}

pub struct TrelloConfig {
    /// API key do Trello (https://trello.com/power-ups/admin).
    pub key: String,
    /// Token de usuário do Trello, gerado a partir da key.
    pub token: String,
    /// Nome do quadro (comparado sem diferenciar maiúsculas nem acentos).
    pub board_name: String,
    /// ID do quadro, opcional. Quando definido, pula a busca pelo nome.
    pub board_id: Option<String>,
    /// Listas servidas, na ordem em que aparecem na resposta.
    pub lists: Vec<String>,
}

const DEFAULT_BOARD: &str = "Programação Máquinas";
const DEFAULT_LISTS: &str = "Aguardando Máquina;Em andamento;Concluído";

impl Config {
    pub fn from_env() -> Result<Self, String> {
        // Carrega o .env se existir; se não existir, não é erro (variáveis
        // podem já estar exportadas no ambiente).
        let _ = dotenvy::dotenv();

        let host = optional("HOST").unwrap_or_else(|| "0.0.0.0".to_string());
        let port = match optional("PORT") {
            Some(v) => v.parse::<u16>().map_err(|_| format!("PORT inválida: \"{v}\""))?,
            None => 3000,
        };

        let egestor = optional("TOKENEGESTOR").map(|personal_token| EgestorConfig {
            personal_token,
            base_url: optional("EGESTOR_BASE_URL")
                .unwrap_or_else(|| "https://v4.egestor.com.br/api".to_string())
                .trim_end_matches('/')
                .to_string(),
        });

        let trello = match (optional("TRELLO_KEY"), optional("TRELLO_TOKEN")) {
            (Some(key), Some(token)) => {
                let lists: Vec<String> = optional("TRELLO_LISTS")
                    .unwrap_or_else(|| DEFAULT_LISTS.to_string())
                    .split(';')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
                if lists.is_empty() {
                    return Err("TRELLO_LISTS está vazio. Informe as listas separadas por ';'.".into());
                }
                Some(TrelloConfig {
                    key,
                    token,
                    board_name: optional("TRELLO_BOARD").unwrap_or_else(|| DEFAULT_BOARD.to_string()),
                    board_id: optional("TRELLO_BOARD_ID"),
                    lists,
                })
            }
            (None, None) => None,
            _ => return Err("Defina TRELLO_KEY e TRELLO_TOKEN juntos (ou nenhum dos dois).".into()),
        };

        Ok(Self {
            sync_egestor_min: minutos("SYNC_EGESTOR_MIN", 30)?,
            sync_trello_min: minutos("SYNC_TRELLO_MIN", 5)?,
            host,
            port,
            api_key: optional("API_KEY"),
            database_url: optional("DATABASE_URL"),
            egestor,
            trello,
        })
    }
}

/// Intervalo em minutos; 0 desliga a sincronização automática.
fn minutos(name: &str, padrao: u64) -> Result<Option<u64>, String> {
    let n = match optional(name) {
        Some(v) => v.parse::<u64>().map_err(|_| format!("{name} inválido: \"{v}\" (use minutos; 0 desliga)"))?,
        None => padrao,
    };
    Ok((n > 0).then_some(n))
}

fn optional(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}
