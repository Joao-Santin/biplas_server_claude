//! Cliente da API REST do Trello (https://developer.atlassian.com/cloud/trello/rest/).

use std::collections::HashMap;
use std::time::Duration;

use reqwest::{Client, header::AUTHORIZATION};
use serde::{Deserialize, de::DeserializeOwned};
use tokio::sync::{Mutex, RwLock};

use super::model::slug;
use crate::{config::TrelloConfig, error::AppError};

const API: &str = "https://api.trello.com/1";

/// Ações que colocam um cartão numa lista: criação, cópia, vinda de outro
/// quadro e movimentação entre listas.
const MOVE_FILTER: &str =
    "createCard,copyCard,moveCardToBoard,convertToCardFromCheckItem,updateCard:idList";
/// Máximo de ações por página aceito pelo Trello.
const ACTIONS_PAGE: usize = 1000;

/// Tudo o que precisamos do quadro vem numa única chamada a `GET /boards/{id}`.
const BOARD_QUERY: &[(&str, &str)] = &[
    ("fields", "name,url"),
    ("lists", "open"),
    ("list_fields", "name"),
    ("cards", "open"),
    (
        "card_fields",
        "name,desc,idList,pos,start,due,dueComplete,shortUrl,dateLastActivity,idMembers,labels",
    ),
    ("card_customFieldItems", "true"),
    ("customFields", "true"),
    ("members", "all"),
    ("member_fields", "fullName,username"),
    ("checklists", "all"),
];

pub struct TrelloClient {
    http: Client,
    /// Header `Authorization: OAuth ...` — mantém key/token fora das URLs e dos logs.
    auth: String,
    board_name: String,
    /// `true` quando o ID veio do .env (não deve ser redescoberto pelo nome).
    board_id_fixed: bool,
    board_id: RwLock<Option<String>>,
    /// Histórico de movimentações já baixado. Ações do Trello não mudam,
    /// então cada chamada só busca as mais novas que a última em cache.
    moves: Mutex<MovesCache>,
    /// Listas servidas, na ordem da resposta.
    lists: Vec<String>,
}

#[derive(Default)]
struct MovesCache {
    board_id: String,
    /// Da mais nova para a mais antiga, como o Trello devolve.
    actions: Vec<RawAction>,
}

impl TrelloClient {
    pub fn new(config: &TrelloConfig) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("falha ao criar o cliente HTTP");
        Self {
            http,
            auth: format!(
                r#"OAuth oauth_consumer_key="{}", oauth_token="{}""#,
                config.key, config.token
            ),
            board_name: config.board_name.clone(),
            board_id_fixed: config.board_id.is_some(),
            board_id: RwLock::new(config.board_id.clone()),
            moves: Mutex::default(),
            lists: config.lists.clone(),
        }
    }

    /// Todas as vezes em que um cartão entrou numa lista do quadro, da mais
    /// nova para a mais antiga.
    pub async fn moves(&self, board_id: &str) -> Result<Vec<RawAction>, AppError> {
        let mut cache = self.moves.lock().await;
        if cache.board_id != board_id {
            *cache = MovesCache { board_id: board_id.to_string(), actions: Vec::new() };
        }

        let since = cache.actions.first().map(|a| a.id.clone());
        let mut fresh: Vec<RawAction> = Vec::new();
        loop {
            let mut query = vec![
                ("filter", MOVE_FILTER),
                ("fields", "date,data"),
                ("memberCreator", "false"),
                ("limit", "1000"),
            ];
            if let Some(since) = &since {
                query.push(("since", since));
            }
            let before = fresh.last().map(|a| a.id.clone());
            if let Some(before) = &before {
                query.push(("before", before));
            }

            let page: Vec<RawAction> =
                self.get(&format!("/boards/{board_id}/actions"), &query).await?;
            let done = page.len() < ACTIONS_PAGE;
            fresh.extend(page);
            if done {
                break;
            }
        }

        if !fresh.is_empty() {
            tracing::info!("{} movimentação(ões) nova(s) no histórico", fresh.len());
            fresh.append(&mut cache.actions);
            cache.actions = fresh;
        }
        Ok(cache.actions.clone())
    }

    /// Busca o quadro configurado com listas, cartões, membros, checklists e
    /// campos personalizados.
    pub async fn board(&self) -> Result<RawBoard, AppError> {
        let id = self.board_id().await?;
        match self.get(&format!("/boards/{id}"), BOARD_QUERY).await {
            // O ID em cache pode ter ficado inválido (quadro recriado):
            // redescobre pelo nome e tenta mais uma vez.
            Err(AppError::Trello { status: 404, .. }) if !self.board_id_fixed => {
                *self.board_id.write().await = None;
                let id = self.board_id().await?;
                self.get(&format!("/boards/{id}"), BOARD_QUERY).await
            }
            other => other,
        }
    }

    pub fn lists(&self) -> &[String] {
        &self.lists
    }

    pub fn board_name(&self) -> &str {
        &self.board_name
    }

    pub async fn board_id(&self) -> Result<String, AppError> {
        if let Some(id) = self.board_id.read().await.clone() {
            return Ok(id);
        }
        let mut guard = self.board_id.write().await;
        if let Some(id) = guard.clone() {
            return Ok(id);
        }

        let boards: Vec<BoardRef> = self
            .get("/members/me/boards", &[("fields", "name"), ("filter", "open")])
            .await?;
        let wanted = slug(&self.board_name);
        let Some(board) = boards.iter().find(|b| slug(&b.name) == wanted) else {
            let names: Vec<&str> = boards.iter().map(|b| b.name.as_str()).collect();
            return Err(AppError::Config(format!(
                "Quadro \"{}\" não encontrado no Trello. Quadros disponíveis: {}",
                self.board_name,
                names.join(", ")
            )));
        };

        tracing::info!("Quadro \"{}\" encontrado (id {})", board.name, board.id);
        *guard = Some(board.id.clone());
        Ok(board.id.clone())
    }

    async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, AppError> {
        let resp = self
            .http
            .get(format!("{API}{path}"))
            .header(AUTHORIZATION, &self.auth)
            .query(query)
            .send()
            .await
            .map_err(AppError::upstream("o Trello"))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::Trello { status: status.as_u16(), body });
        }
        resp.json().await.map_err(AppError::upstream("o Trello"))
    }
}

// --- Formato bruto devolvido pelo Trello -----------------------------------

#[derive(Deserialize)]
struct BoardRef {
    id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
pub struct RawBoard {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub lists: Vec<RawList>,
    #[serde(default)]
    pub cards: Vec<RawCard>,
    #[serde(default)]
    pub members: Vec<RawMember>,
    #[serde(default)]
    pub checklists: Vec<RawChecklist>,
    #[serde(default, rename = "customFields")]
    pub custom_fields: Vec<RawCustomField>,
}

#[derive(Debug, Deserialize)]
pub struct RawList {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawCard {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub desc: String,
    pub id_list: String,
    #[serde(default)]
    pub pos: f64,
    pub start: Option<String>,
    pub due: Option<String>,
    #[serde(default)]
    pub due_complete: bool,
    #[serde(default)]
    pub short_url: String,
    pub date_last_activity: Option<String>,
    #[serde(default)]
    pub id_members: Vec<String>,
    #[serde(default)]
    pub labels: Vec<RawLabel>,
    #[serde(default)]
    pub custom_field_items: Vec<RawCustomFieldItem>,
}

#[derive(Debug, Deserialize)]
pub struct RawLabel {
    #[serde(default)]
    pub name: String,
    pub color: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawMember {
    pub id: String,
    #[serde(default)]
    pub full_name: String,
    #[serde(default)]
    pub username: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawChecklist {
    pub id_card: String,
    pub name: String,
    #[serde(default)]
    pub pos: f64,
    #[serde(default)]
    pub check_items: Vec<RawCheckItem>,
}

#[derive(Debug, Deserialize)]
pub struct RawCheckItem {
    pub name: String,
    /// "complete" ou "incomplete".
    pub state: String,
    #[serde(default)]
    pub pos: f64,
}

/// Uma ação do histórico do quadro (ver `MOVE_FILTER`).
#[derive(Debug, Clone, Deserialize)]
pub struct RawAction {
    pub id: String,
    /// ISO 8601 (UTC).
    pub date: String,
    pub data: RawActionData,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawActionData {
    pub card: Option<RawRef>,
    /// Lista do cartão nas criações/cópias.
    pub list: Option<RawRef>,
    /// Lista de destino nas movimentações.
    pub list_after: Option<RawRef>,
}

impl RawAction {
    /// (cartão, lista em que ele entrou).
    pub fn entry(&self) -> Option<(&str, &str)> {
        let card = self.data.card.as_ref()?;
        let list = self.data.list_after.as_ref().or(self.data.list.as_ref())?;
        Some((card.id.as_str(), list.id.as_str()))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawRef {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct RawCustomField {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub options: Vec<RawCustomFieldOption>,
}

#[derive(Debug, Deserialize)]
pub struct RawCustomFieldOption {
    pub id: String,
    /// `{"text": "..."}`
    #[serde(default)]
    pub value: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawCustomFieldItem {
    pub id_custom_field: String,
    /// Preenchido nos campos do tipo lista (dropdown).
    pub id_value: Option<String>,
    /// `{"text"|"number"|"date"|"checked": "..."}` nos demais tipos.
    pub value: Option<HashMap<String, String>>,
}
