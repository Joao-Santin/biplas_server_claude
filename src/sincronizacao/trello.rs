//! Sincronização do Trello: listas configuradas, seus cartões e o histórico
//! de movimentações (só as ações novas a cada vez).

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use super::TRELLO;
use crate::{error::AppError, state::AppState, trello::model};

pub async fn sincronizar(state: &AppState) -> Result<Value, AppError> {
    let db = state.db()?;
    let trello = state.trello()?;

    state.hub.progresso(TRELLO, "Lendo o quadro...", 0.1);
    let raw = trello.board().await?;
    let quadro_id = raw.id.clone();

    let since: Option<String> = sqlx::query_scalar(
        "SELECT id FROM trello_movimentos WHERE quadro_id = $1 ORDER BY data DESC, id DESC LIMIT 1",
    )
    .bind(&quadro_id)
    .fetch_optional(db)
    .await?;
    state.hub.progresso(
        TRELLO,
        if since.is_none() { "Baixando todo o histórico de movimentações..." } else { "Lendo movimentações novas..." },
        0.4,
    );
    let moves = trello.moves(&quadro_id, since.as_deref()).await?;
    let quadro = model::build(raw, trello.lists())?;

    state.hub.progresso(TRELLO, "Gravando no banco...", 0.9);
    let (mut m_ids, mut m_cartoes, mut m_listas, mut m_datas) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for a in &moves {
        let (Some((cartao, lista)), Ok(data)) = (a.entry(), DateTime::parse_from_rfc3339(&a.date)) else { continue };
        m_ids.push(a.id.clone());
        m_cartoes.push(cartao.to_string());
        m_listas.push(lista.to_string());
        m_datas.push(data.with_timezone(&Utc));
    }

    let mut tx = db.begin().await?;
    sqlx::query(
        "INSERT INTO trello_quadro (id, nome, url) VALUES ($1, $2, $3)
         ON CONFLICT (unico) DO UPDATE SET id = EXCLUDED.id, nome = EXCLUDED.nome, url = EXCLUDED.url, atualizado_em = now()",
    )
    .bind(&quadro.id)
    .bind(&quadro.nome)
    .bind(&quadro.url)
    .execute(&mut *tx)
    .await?;

    // Listas e cartões são sempre a foto atual do quadro.
    sqlx::query("DELETE FROM trello_cartoes").execute(&mut *tx).await?;
    sqlx::query("DELETE FROM trello_listas").execute(&mut *tx).await?;
    let mut num_cartoes = 0;
    for (ordem, lista) in quadro.listas.iter().enumerate() {
        sqlx::query("INSERT INTO trello_listas (id, nome, slug, ordem) VALUES ($1, $2, $3, $4)")
            .bind(&lista.id)
            .bind(&lista.nome)
            .bind(&lista.slug)
            .bind(ordem as i32)
            .execute(&mut *tx)
            .await?;
        let ids: Vec<&str> = lista.cartoes.iter().map(|c| c.id.as_str()).collect();
        let nomes: Vec<&str> = lista.cartoes.iter().map(|c| c.nome.as_str()).collect();
        let dados: Vec<Value> =
            lista.cartoes.iter().map(|c| serde_json::to_value(c).expect("cartão serializável")).collect();
        sqlx::query(
            "INSERT INTO trello_cartoes (id, lista_id, posicao, nome, dados)
             SELECT id, $4, (ord - 1)::int, nome, dados
             FROM UNNEST($1::text[], $2::text[], $3::jsonb[]) WITH ORDINALITY AS t(id, nome, dados, ord)",
        )
        .bind(&ids)
        .bind(&nomes)
        .bind(&dados)
        .bind(&lista.id)
        .execute(&mut *tx)
        .await?;
        num_cartoes += ids.len();
    }

    let novos = sqlx::query(
        "INSERT INTO trello_movimentos (id, quadro_id, cartao_id, lista_id, data)
         SELECT id, $5, c, l, d FROM UNNEST($1::text[], $2::text[], $3::text[], $4::timestamptz[]) AS t(id, c, l, d)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(&m_ids)
    .bind(&m_cartoes)
    .bind(&m_listas)
    .bind(&m_datas)
    .bind(&quadro_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;

    Ok(json!({
        "listas": quadro.listas.len(),
        "cartoes": num_cartoes,
        "movimentos_novos": novos,
    }))
}
