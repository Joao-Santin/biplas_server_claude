//! Sincronização do eGestor: OPs em aberto (com o detalhe de cada uma),
//! cadastro de produtos, composições e categorias.
//!
//! Tudo é lido primeiro e gravado no fim, numa transação só: quem lê o banco
//! nunca vê metade de uma sincronização.

use std::{
    collections::{BTreeMap, HashSet},
    time::{Duration, Instant},
};

use reqwest::Method;
use serde_json::{Value, json};
use sqlx::PgConnection;

use super::EGESTOR;
use crate::{egestor::EgestorClient, error::AppError, state::AppState};

/// Situação "em aberto" das OPs no eGestor.
pub const SITUACAO_ABERTA: i64 = 10;
/// O eGestor limita a 60 requisições por minuto.
const INTERVALO_MIN: Duration = Duration::from_millis(1050);
const MAX_TENTATIVAS: u32 = 6;

/// Faz as chamadas respeitando o limite do eGestor e repetindo em caso de
/// 429, erro 5xx ou falha de rede.
struct Coletor<'a> {
    api: &'a EgestorClient,
    ultima: Option<Instant>,
}

impl Coletor<'_> {
    async fn chamar(&mut self, method: Method, caminho: &str, corpo: Option<&Value>) -> Result<(u16, Value), AppError> {
        let mut tentativa = 0;
        loop {
            tentativa += 1;
            if let Some(t) = self.ultima {
                let passou = t.elapsed();
                if passou < INTERVALO_MIN {
                    tokio::time::sleep(INTERVALO_MIN - passou).await;
                }
            }
            self.ultima = Some(Instant::now());

            match self.api.json(method.clone(), caminho, corpo).await {
                Err(AppError::Upstream { erro, .. }) if tentativa < MAX_TENTATIVAS => {
                    tracing::warn!("eGestor: erro de rede em {caminho} ({erro}); tentando de novo...");
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
                Ok((status, _)) if (status == 429 || status >= 500) && tentativa < MAX_TENTATIVAS => {
                    let espera = 10 * u64::from(tentativa);
                    tracing::warn!("eGestor: {status} em {caminho}; aguardando {espera}s...");
                    tokio::time::sleep(Duration::from_secs(espera)).await;
                }
                outro => return outro,
            }
        }
    }

    async fn sucesso(&mut self, method: Method, caminho: &str, corpo: Option<&Value>) -> Result<Value, AppError> {
        let (status, v) = self.chamar(method, caminho, corpo).await?;
        if !(200..300).contains(&status) {
            return Err(AppError::Egestor { status, body: format!("{caminho}: {v}") });
        }
        Ok(v)
    }

    async fn get(&mut self, caminho: &str) -> Result<Value, AppError> {
        self.sucesso(Method::GET, caminho, None).await
    }

    /// Lê todas as páginas de uma listagem (`data`, `last_page`).
    ///
    /// O `last_page` do eGestor conta registros excluídos, então uma página
    /// pode vir com menos itens (ou vazia): vamos até o maior `last_page`
    /// visto, sem parar em página vazia, e tiramos duplicados por `codigo`.
    async fn todas_paginas(
        &mut self,
        caminho: &str,
        mut progresso: impl FnMut(u64, u64, usize),
    ) -> Result<Vec<Value>, AppError> {
        let sep = if caminho.contains('?') { '&' } else { '?' };
        let mut itens = Vec::new();
        let mut vistos = HashSet::new();
        let (mut pagina, mut ultima) = (1u64, 1u64);
        loop {
            let v = self.get(&format!("{caminho}{sep}page={pagina}")).await?;
            ultima = ultima.max(v["last_page"].as_u64().unwrap_or(pagina));
            for item in v["data"].as_array().into_iter().flatten() {
                if item["codigo"].as_i64().is_none_or(|c| vistos.insert(c)) {
                    itens.push(item.clone());
                }
            }
            progresso(pagina, ultima, itens.len());
            if pagina >= ultima {
                return Ok(itens);
            }
            pagina += 1;
        }
    }
}

/// Os relatórios do eGestor devolvem até os códigos como texto.
fn inteiro(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_str()?.trim().parse().ok())
}

/// O relatório pode trazer o mesmo produto em várias linhas, cada uma com
/// parte dos insumos: junta tudo numa linha por produto.
fn agrupar_composicoes(linhas: Vec<Value>) -> BTreeMap<i64, Value> {
    let mut mapa: BTreeMap<i64, Value> = BTreeMap::new();
    for linha in linhas {
        let Some(cod) = inteiro(&linha["cod"]) else { continue };
        let insumos = linha["insumos"].as_array().cloned().unwrap_or_default();
        match mapa.get_mut(&cod) {
            Some(existente) => {
                if let Some(lista) = existente["insumos"].as_array_mut() {
                    lista.extend(insumos);
                }
            }
            None => {
                let mut linha = linha;
                linha["insumos"] = Value::Array(insumos);
                mapa.insert(cod, linha);
            }
        }
    }
    mapa
}

/// Por código, sem repetir (o último vence).
fn por_codigo(itens: impl IntoIterator<Item = Value>) -> BTreeMap<i64, Value> {
    itens.into_iter().filter_map(|v| Some((v["codigo"].as_i64()?, v))).collect()
}

/// Troca o `estoque` de cada produto pelo do relatório "estoque do dia", que
/// tem as casas decimais. Produto fora do relatório fica com o do cadastro.
fn aplicar_estoque_do_dia(produtos: &mut BTreeMap<i64, Value>, linhas: &[Value]) {
    for linha in linhas {
        let Some(cod) = inteiro(&linha["codigo"]) else { continue };
        let e = &linha["estoque"];
        let Some(estoque) = e.as_f64().or_else(|| e.as_str()?.trim().parse().ok()) else { continue };
        if let Some(p) = produtos.get_mut(&cod) {
            p["estoque"] = json!(estoque);
        }
    }
}

/// Produtos e insumos citados nas OPs.
fn citados_nas_ops(ops: &[Value]) -> Vec<i64> {
    let mut codigos: Vec<i64> = ops
        .iter()
        .flat_map(|o| o["produtos"].as_array().into_iter().flatten())
        .flat_map(|p| {
            let insumos = p["insumos"].as_array().into_iter().flatten().filter_map(|i| i["codInsumo"].as_i64());
            p["codProduto"].as_i64().into_iter().chain(insumos)
        })
        .collect();
    codigos.sort_unstable();
    codigos.dedup();
    codigos
}

pub async fn sincronizar(state: &AppState) -> Result<Value, AppError> {
    let db = state.db()?;
    let mut c = Coletor { api: state.egestor()?, ultima: None };
    let prog = |etapa: String, fracao: f32| state.hub.progresso(EGESTOR, etapa, fracao);

    // Chamadas rápidas primeiro.
    prog("Lendo composições cadastradas...".into(), 0.0);
    let filtros = json!({ "codProds": "", "categoria": "", "tags": "" });
    let Value::Array(linhas) = c.sucesso(Method::POST, "/v1/relatorios/composicoes", Some(&filtros)).await? else {
        return Err(AppError::Internal("O relatório de composições do eGestor não devolveu uma lista".into()));
    };
    let composicoes = agrupar_composicoes(linhas);
    prog("Lendo categorias...".into(), 0.01);
    let categorias = por_codigo(c.todas_paginas("/v1/categorias", |_, _, _| {}).await?);

    // Lista de OPs em aberto (2% – 5%).
    let lista = c
        .todas_paginas(&format!("/v1/producao?situacao={SITUACAO_ABERTA}"), |p, u, _| {
            prog(format!("Listando OPs em aberto (página {p}/{u})"), 0.02 + 0.03 * p as f32 / u as f32)
        })
        .await?;
    let mut abertas: Vec<i64> = lista
        .iter()
        .filter(|o| o["situacao"].as_i64() == Some(SITUACAO_ABERTA))
        .filter(|o| o["ativo"].as_i64().unwrap_or(1) == 1)
        .filter_map(|o| o["codigo"].as_i64())
        .collect();
    abertas.sort_unstable();
    abertas.dedup();

    // Detalhe de cada OP (5% – 50%).
    let mut ops = Vec::with_capacity(abertas.len());
    let total = abertas.len().max(1);
    for (i, cod) in abertas.iter().enumerate() {
        prog(
            format!("Lendo composição da OP {cod} ({}/{})", i + 1, abertas.len()),
            0.05 + 0.45 * i as f32 / total as f32,
        );
        ops.push(c.get(&format!("/v1/producao/{cod}")).await?);
    }

    // Cadastro de produtos (50% – 93%).
    let mut produtos = por_codigo(
        c.todas_paginas("/v1/produtos", |p, u, n| {
            prog(format!("Lendo cadastro de produtos (página {p}/{u}, {n} produtos)"), 0.50 + 0.43 * p as f32 / u as f32)
        })
        .await?,
    );

    // Produtos das OPs que a listagem não trouxe (ex.: inativos) (93% – 95%).
    let faltantes: Vec<i64> = citados_nas_ops(&ops).into_iter().filter(|c| !produtos.contains_key(c)).collect();
    for cod in faltantes {
        prog(format!("Buscando produto {cod}"), 0.94);
        if let Ok(v) = c.get(&format!("/v1/produtos/{cod}")).await {
            produtos.insert(cod, v);
        }
    }

    // O cadastro de produtos traz o estoque truncado (516,9 vem como 516); o
    // relatório "estoque do dia" traz o valor com as casas decimais (95%).
    prog("Lendo estoque do dia...".into(), 0.95);
    let filtros = json!({
        "dia": chrono::Local::now().format("%Y-%m-%d").to_string(),
        "categoria": "",
        "tags": "",
        "semExcluidos": false,
        "semEstNaoControl": false,
        "mostrarEstoqueNegativo": true,
        "mostrarCodProprio": false,
        "apresentarArquivados": true,
    });
    let Value::Array(linhas) = c.sucesso(Method::POST, "/v1/relatorios/estoqueDoDia", Some(&filtros)).await? else {
        return Err(AppError::Internal("O relatório de estoque do dia do eGestor não devolveu uma lista".into()));
    };
    aplicar_estoque_do_dia(&mut produtos, &linhas);

    // OPs que estavam em aberto e saíram da lista: guarda como ficaram (95% – 98%).
    let sairam: Vec<i64> = sqlx::query_scalar("SELECT codigo FROM egestor_producao WHERE aberta AND NOT (codigo = ANY($1))")
        .bind(&abertas)
        .fetch_all(db)
        .await?;
    let mut fechadas = Vec::new();
    let mut sumiram = Vec::new();
    for cod in &sairam {
        prog(format!("Atualizando OP {cod}, que saiu das abertas"), 0.96);
        match c.chamar(Method::GET, &format!("/v1/producao/{cod}"), None).await {
            Ok((200..=299, v)) => fechadas.push((*cod, v)),
            Ok((404 | 410, _)) => sumiram.push(*cod),
            // Sem o detalhe novo, fica o antigo, só marcada como fechada.
            _ => {}
        }
    }

    prog("Gravando no banco...".into(), 0.98);
    let num_composicoes = composicoes.len();
    let mut tx = db.begin().await?;
    substituir(&mut tx, "egestor_composicoes", "cod_produto", composicoes.into_iter().collect()).await?;
    substituir(&mut tx, "egestor_categorias", "codigo", categorias.iter().map(|(k, v)| (*k, v.clone())).collect()).await?;
    substituir(&mut tx, "egestor_produtos", "codigo", produtos.iter().map(|(k, v)| (*k, v.clone())).collect()).await?;
    gravar_ops(&mut tx, &ops, true).await?;
    gravar_ops(&mut tx, &fechadas.iter().map(|(_, v)| v.clone()).collect::<Vec<_>>(), false).await?;
    sqlx::query("UPDATE egestor_producao SET aberta = FALSE, atualizado_em = now() WHERE aberta AND NOT (codigo = ANY($1))")
        .bind(&abertas)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM egestor_producao WHERE codigo = ANY($1)").bind(&sumiram).execute(&mut *tx).await?;
    let descartados = descartar_parciais_invalidos(&mut tx).await?;
    tx.commit().await?;

    if descartados > 0 {
        state.hub.dados_mudaram("parciais", None);
    }
    Ok(json!({
        "ops_abertas": ops.len(),
        "ops_fechadas": sairam.len(),
        "produtos": produtos.len(),
        "composicoes": num_composicoes,
        "categorias": categorias.len(),
        "parciais_descartados": descartados,
    }))
}


/// Deixa a tabela com exatamente estes registros. `atualizado_em` só muda
/// quando o conteúdo mudou.
async fn substituir(tx: &mut PgConnection, tabela: &str, chave: &str, itens: Vec<(i64, Value)>) -> Result<(), sqlx::Error> {
    let (codigos, dados): (Vec<i64>, Vec<Value>) = itens.into_iter().unzip();
    sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM {tabela} WHERE NOT ({chave} = ANY($1))")))
        .bind(&codigos)
        .execute(&mut *tx)
        .await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO {tabela} ({chave}, dados)
         SELECT * FROM UNNEST($1::bigint[], $2::jsonb[])
         ON CONFLICT ({chave}) DO UPDATE SET dados = EXCLUDED.dados, atualizado_em = now()
         WHERE {tabela}.dados IS DISTINCT FROM EXCLUDED.dados"
    )))
    .bind(&codigos)
    .bind(&dados)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

async fn gravar_ops(tx: &mut PgConnection, ops: &[Value], aberta: bool) -> Result<(), sqlx::Error> {
    let porcod = por_codigo(ops.iter().cloned());
    let codigos: Vec<i64> = porcod.keys().copied().collect();
    let situacoes: Vec<Option<i64>> = porcod.values().map(|v| inteiro(&v["situacao"])).collect();
    let dados: Vec<Value> = porcod.into_values().collect();
    sqlx::query(
        "INSERT INTO egestor_producao (codigo, situacao, aberta, dados)
         SELECT c, s, $4, d FROM UNNEST($1::bigint[], $2::bigint[], $3::jsonb[]) AS t(c, s, d)
         ON CONFLICT (codigo) DO UPDATE
           SET situacao = EXCLUDED.situacao, aberta = EXCLUDED.aberta, dados = EXCLUDED.dados, atualizado_em = now()
         WHERE egestor_producao.dados IS DISTINCT FROM EXCLUDED.dados
            OR egestor_producao.aberta IS DISTINCT FROM EXCLUDED.aberta",
    )
    .bind(&codigos)
    .bind(&situacoes)
    .bind(&dados)
    .bind(aberta)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// O realizado parcial só vale enquanto a OP está aberta e a linha tem o
/// mesmo produto. Devolve quantos lançamentos foram apagados.
async fn descartar_parciais_invalidos(tx: &mut PgConnection) -> Result<u64, sqlx::Error> {
    let r = sqlx::query(
        "DELETE FROM panorama_parciais p
         WHERE NOT EXISTS (
            SELECT 1 FROM egestor_producao o
            WHERE o.codigo = p.op AND o.aberta
              AND o.dados -> 'produtos' -> p.linha ->> 'codProduto' = p.cod_produto::text
         )",
    )
    .execute(&mut *tx)
    .await?;
    Ok(r.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composicao_em_varias_linhas_vira_uma_so() {
        let mapa = agrupar_composicoes(vec![
            json!({"cod": "1004", "produto": "PORTA", "insumos": [{"codInsumo": "50"}]}),
            json!({"cod": "1004", "produto": "PORTA", "insumos": [{"codInsumo": "140"}]}),
            json!({"cod": 7, "produto": "SEM", "insumos": []}),
            json!({"cod": "x", "insumos": []}),
        ]);
        assert_eq!(mapa.len(), 2);
        assert_eq!(mapa[&1004]["insumos"], json!([{"codInsumo": "50"}, {"codInsumo": "140"}]));
        assert_eq!(mapa[&7]["produto"], "SEM");
    }

    #[test]
    fn produtos_e_insumos_citados_sem_repetir() {
        let ops = [
            json!({"produtos": [{"codProduto": 3, "insumos": [{"codInsumo": 9}, {"codInsumo": 1}]}]}),
            json!({"produtos": [{"codProduto": 1, "insumos": []}]}),
        ];
        assert_eq!(citados_nas_ops(&ops), [1, 3, 9]);
    }

    #[test]
    fn estoque_do_dia_traz_as_casas_decimais() {
        let mut produtos = por_codigo([
            json!({"codigo": 64, "estoque": 516}),
            json!({"codigo": 65, "estoque": 10}),
            json!({"codigo": 66, "estoque": 3}),
        ]);
        aplicar_estoque_do_dia(
            &mut produtos,
            &[
                json!({"codigo": 64, "estoque": 516.9}),
                json!({"codigo": "65", "estoque": "-2.125"}),
                json!({"codigo": 999, "estoque": 1}),
            ],
        );
        assert_eq!(produtos[&64]["estoque"], json!(516.9));
        assert_eq!(produtos[&65]["estoque"], json!(-2.125));
        assert_eq!(produtos[&66]["estoque"], json!(3));
        assert!(!produtos.contains_key(&999));
    }
}
