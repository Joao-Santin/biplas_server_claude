//! Formato servido para os outros programas: quadro → listas → cartões.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    error::AppError,
    trello::client::RawBoard,
};

#[derive(Debug, Serialize)]
pub struct Quadro {
    pub id: String,
    pub nome: String,
    pub url: String,
    pub listas: Vec<Lista>,
}

#[derive(Debug, Serialize)]
pub struct Lista {
    pub id: String,
    pub nome: String,
    /// Identificador usado na rota `/api/listas/{slug}` (ex.: "em-andamento").
    pub slug: String,
    pub total: usize,
    pub cartoes: Vec<Cartao>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Cartao {
    pub id: String,
    pub nome: String,
    pub descricao: String,
    pub url: String,
    /// Nome da lista em que o cartão está.
    pub lista: String,
    /// Datas em ISO 8601 (UTC), como o Trello devolve.
    pub inicio: Option<String>,
    pub prazo: Option<String>,
    pub prazo_concluido: bool,
    pub ultima_atividade: Option<String>,
    pub etiquetas: Vec<Etiqueta>,
    pub membros: Vec<Membro>,
    pub checklists: Vec<Checklist>,
    /// Campos personalizados: nome do campo → valor (texto, número, data ou booleano).
    pub campos: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Etiqueta {
    pub nome: String,
    pub cor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Membro {
    pub id: String,
    pub nome: String,
    pub usuario: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Checklist {
    pub nome: String,
    pub total: usize,
    pub concluidos: usize,
    pub itens: Vec<ItemChecklist>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ItemChecklist {
    pub nome: String,
    pub concluido: bool,
}

/// Situação de um cartão para cruzar com as OPs do ERP (`GET /api/ops`).
#[derive(Debug, Serialize)]
pub struct CartaoOp {
    /// Números das OPs do título, ex.: "(OP-981, 1046)" → [981, 1046].
    /// Vazio quando o cartão ainda não tem OP.
    pub ops: Vec<u64>,
    pub cartao: String,
    pub url: String,
    pub lista: String,
    pub slug: String,
    /// Separador "MAQUINA N" mais próximo acima do cartão na lista.
    pub maquina: Option<String>,
    /// Slug da lista → última vez (ISO 8601, UTC) em que o cartão entrou nela.
    pub entradas: BTreeMap<String, String>,
    /// Etiquetas do cartão (ex.: "Urgente", vermelha).
    pub etiquetas: Vec<Etiqueta>,
}

/// Última entrada de cada cartão em cada lista: id do cartão → (slug da
/// lista → data ISO 8601). Vem do histórico gravado no banco.
pub type Entradas = HashMap<String, BTreeMap<String, String>>;

/// Cartões do quadro com OPs, máquina e datas de entrada em cada lista.
/// Os separadores "MAQUINA N" não entram na resposta.
pub fn ops(quadro: Quadro, entradas: &Entradas) -> Vec<CartaoOp> {
    let mut out = Vec::new();
    for lista in &quadro.listas {
        let mut maquina = None;
        for c in &lista.cartoes {
            if is_machine(&c.nome) {
                maquina = Some(c.nome.trim().to_string());
                continue;
            }
            out.push(CartaoOp {
                ops: op_numbers(&c.nome),
                cartao: c.nome.trim().to_string(),
                url: c.url.clone(),
                lista: lista.nome.clone(),
                slug: lista.slug.clone(),
                maquina: maquina.clone(),
                entradas: entradas.get(&c.id).cloned().unwrap_or_default(),
                etiquetas: c.etiquetas.clone(),
            });
        }
    }
    out
}

/// Cartão separador de máquina: "MAQUINA 1", "Máquina 10"...
fn is_machine(name: &str) -> bool {
    slug(name)
        .strip_prefix("maquina-")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Números do último "(OP-...)" do título. "(OP-NT)" e "(OP-" sem número → vazio.
fn op_numbers(name: &str) -> Vec<u64> {
    let upper = name.to_uppercase();
    let Some(start) = upper.rfind("(OP") else { return Vec::new() };
    let rest = &upper[start + 3..];
    let inner = rest.split(')').next().unwrap_or(rest);
    inner
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|n| n.parse().ok())
        .collect()
}

/// Normaliza um nome para comparação e para uso em URL:
/// "Aguardando Máquina" → "aguardando-maquina".
pub fn slug(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        let c = match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            c => c,
        };
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Monta o quadro só com as listas pedidas, na ordem pedida.
pub fn build(raw: RawBoard, wanted: &[String]) -> Result<Quadro, AppError> {
    let members: HashMap<&str, Membro> = raw
        .members
        .iter()
        .map(|m| {
            let membro = Membro {
                id: m.id.clone(),
                nome: if m.full_name.is_empty() { m.username.clone() } else { m.full_name.clone() },
                usuario: m.username.clone(),
            };
            (m.id.as_str(), membro)
        })
        .collect();

    let fields: HashMap<&str, &crate::trello::client::RawCustomField> =
        raw.custom_fields.iter().map(|f| (f.id.as_str(), f)).collect();

    let mut checklists_by_card: HashMap<&str, Vec<&crate::trello::client::RawChecklist>> = HashMap::new();
    for cl in &raw.checklists {
        checklists_by_card.entry(cl.id_card.as_str()).or_default().push(cl);
    }

    let mut missing = Vec::new();
    let mut listas = Vec::new();
    for name in wanted {
        let key = slug(name);
        let Some(list) = raw.lists.iter().find(|l| slug(&l.name) == key) else {
            missing.push(name.as_str());
            continue;
        };

        let mut cards: Vec<_> = raw.cards.iter().filter(|c| c.id_list == list.id).collect();
        cards.sort_by(|a, b| a.pos.total_cmp(&b.pos));

        let cartoes: Vec<Cartao> = cards
            .into_iter()
            .map(|c| Cartao {
                id: c.id.clone(),
                nome: c.name.clone(),
                descricao: c.desc.clone(),
                url: c.short_url.clone(),
                lista: list.name.clone(),
                inicio: c.start.clone(),
                prazo: c.due.clone(),
                prazo_concluido: c.due_complete,
                ultima_atividade: c.date_last_activity.clone(),
                etiquetas: c
                    .labels
                    .iter()
                    .map(|l| Etiqueta { nome: l.name.clone(), cor: l.color.clone() })
                    .collect(),
                membros: c
                    .id_members
                    .iter()
                    .filter_map(|id| members.get(id.as_str()))
                    .map(|m| Membro { id: m.id.clone(), nome: m.nome.clone(), usuario: m.usuario.clone() })
                    .collect(),
                checklists: checklists_for(checklists_by_card.get(c.id.as_str())),
                campos: c
                    .custom_field_items
                    .iter()
                    .filter_map(|item| {
                        let field = fields.get(item.id_custom_field.as_str())?;
                        Some((field.name.clone(), custom_value(field, item)?))
                    })
                    .collect(),
            })
            .collect();

        listas.push(Lista {
            id: list.id.clone(),
            nome: list.name.clone(),
            slug: key,
            total: cartoes.len(),
            cartoes,
        });
    }

    if !missing.is_empty() {
        let available: Vec<&str> = raw.lists.iter().map(|l| l.name.as_str()).collect();
        return Err(AppError::Config(format!(
            "Lista(s) {} não encontrada(s) no quadro \"{}\". Listas disponíveis: {}",
            missing.join(", "),
            raw.name,
            available.join(", ")
        )));
    }

    Ok(Quadro { id: raw.id, nome: raw.name, url: raw.url, listas })
}

fn checklists_for(list: Option<&Vec<&crate::trello::client::RawChecklist>>) -> Vec<Checklist> {
    let Some(list) = list else { return Vec::new() };
    let mut list = list.clone();
    list.sort_by(|a, b| a.pos.total_cmp(&b.pos));
    list.into_iter()
        .map(|cl| {
            let mut items: Vec<_> = cl.check_items.iter().collect();
            items.sort_by(|a, b| a.pos.total_cmp(&b.pos));
            let itens: Vec<ItemChecklist> = items
                .into_iter()
                .map(|i| ItemChecklist { nome: i.name.clone(), concluido: i.state == "complete" })
                .collect();
            Checklist {
                nome: cl.name.clone(),
                total: itens.len(),
                concluidos: itens.iter().filter(|i| i.concluido).count(),
                itens,
            }
        })
        .collect()
}

fn custom_value(
    field: &crate::trello::client::RawCustomField,
    item: &crate::trello::client::RawCustomFieldItem,
) -> Option<Value> {
    if let Some(id_value) = &item.id_value {
        let option = field.options.iter().find(|o| &o.id == id_value)?;
        return option.value.get("text").map(|t| Value::String(t.clone()));
    }
    let value = item.value.as_ref()?;
    if let Some(n) = value.get("number") {
        return Some(
            n.parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number)
                .unwrap_or_else(|| Value::String(n.clone())),
        );
    }
    if let Some(b) = value.get("checked") {
        return Some(Value::Bool(b == "true"));
    }
    value.get("text").or_else(|| value.get("date")).map(|s| Value::String(s.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_ignores_case_and_accents() {
        assert_eq!(slug("Aguardando Máquina"), "aguardando-maquina");
        assert_eq!(slug("  Em   andamento "), "em-andamento");
        assert_eq!(slug("Concluído"), "concluido");
        assert_eq!(slug("PROGRAMAÇÃO MÁQUINAS"), slug("programação máquinas"));
    }

    #[test]
    fn builds_requested_lists_in_order() {
        let raw: RawBoard = serde_json::from_value(serde_json::json!({
            "id": "b1", "name": "Programação Máquinas", "url": "https://trello.com/b/x",
            "lists": [
                {"id": "l1", "name": "Concluído", "pos": 3},
                {"id": "l2", "name": "Em andamento", "pos": 2},
                {"id": "l3", "name": "Backlog", "pos": 1}
            ],
            "cards": [
                {"id": "c2", "name": "Segundo", "idList": "l2", "pos": 20,
                 "idMembers": ["m1"], "labels": [{"name": "Urgente", "color": "red"}],
                 "customFieldItems": [
                    {"idCustomField": "f1", "idValue": "o1"},
                    {"idCustomField": "f2", "value": {"number": "12.5"}}
                 ]},
                {"id": "c1", "name": "Primeiro", "idList": "l2", "pos": 10},
                {"id": "c3", "name": "Fora", "idList": "l3", "pos": 1}
            ],
            "members": [{"id": "m1", "fullName": "Fulano", "username": "fulano"}],
            "checklists": [{"idCard": "c2", "name": "Etapas", "checkItems": [
                {"name": "a", "state": "complete", "pos": 1},
                {"name": "b", "state": "incomplete", "pos": 2}
            ]}],
            "customFields": [
                {"id": "f1", "name": "Máquina", "options": [{"id": "o1", "value": {"text": "Torno CNC"}}]},
                {"id": "f2", "name": "Horas"}
            ]
        }))
        .unwrap();

        let q = build(raw, &["em andamento".into(), "CONCLUIDO".into()]).unwrap();
        assert_eq!(q.listas.len(), 2);
        assert_eq!(q.listas[0].nome, "Em andamento");
        assert_eq!(q.listas[0].total, 2);
        assert_eq!(q.listas[0].cartoes[0].nome, "Primeiro");
        let c = &q.listas[0].cartoes[1];
        assert_eq!(c.membros[0].nome, "Fulano");
        assert_eq!(c.etiquetas[0].cor.as_deref(), Some("red"));
        assert_eq!(c.checklists[0].concluidos, 1);
        assert_eq!(c.campos["Máquina"], "Torno CNC");
        assert_eq!(c.campos["Horas"], 12.5);
        assert_eq!(q.listas[1].total, 0);
    }

    #[test]
    fn parses_op_numbers() {
        assert_eq!(op_numbers("ANIMALART: ANEL(PV-733; FALTA X) (OP-1323)"), [1323]);
        assert_eq!(op_numbers("TUBO (ESTOQUE) (OP-1215-1216-1217-1218)"), [1215, 1216, 1217, 1218]);
        assert_eq!(op_numbers("CABIDE(PV-535, 568) (OP-981, 1046)"), [981, 1046]);
        assert_eq!(op_numbers("MORDEDOR (op-1148) **ALTERADO**"), [1148]);
        assert!(op_numbers("BASE DO PINO(PV-À DEFINIR) (OP-NT)").is_empty());
        assert!(op_numbers("TAMPAS(PV-PEDIDO CONTINUO) (OP-").is_empty());
        assert!(op_numbers("ESTOJO GRANDE(PV-727)").is_empty());
    }

    #[test]
    fn ops_use_machine_separators_and_latest_entries() {
        let raw: RawBoard = serde_json::from_value(serde_json::json!({
            "id": "b1", "name": "Q",
            "lists": [{"id": "l1", "name": "Aguardando Máquina"}, {"id": "l2", "name": "Em andamento"}],
            "cards": [
                {"id": "s1", "name": "MAQUINA 1", "idList": "l2", "pos": 1},
                {"id": "c1", "name": "A (OP-10)", "idList": "l2", "pos": 2,
                 "labels": [{"name": "Urgente", "color": "red"}, {"color": "green"}]},
                {"id": "s2", "name": "Máquina 2", "idList": "l2", "pos": 3},
                {"id": "c2", "name": "B (OP-20)", "idList": "l2", "pos": 4},
                {"id": "c3", "name": "C sem OP", "idList": "l1", "pos": 1}
            ]
        }))
        .unwrap();
        let entradas: Entradas = [(
            "c1".to_string(),
            BTreeMap::from([
                ("em-andamento".to_string(), "2026-10-03T10:00:00Z".to_string()),
                ("aguardando-maquina".to_string(), "2026-10-01T10:00:00Z".to_string()),
            ]),
        )]
        .into();

        let quadro = build(raw, &["Aguardando Máquina".into(), "Em andamento".into()]).unwrap();
        let ops = ops(quadro, &entradas);
        assert_eq!(ops.len(), 3);
        assert!(ops[0].ops.is_empty() && ops[0].maquina.is_none());
        assert_eq!(ops[1].ops, [10]);
        assert_eq!(ops[1].maquina.as_deref(), Some("MAQUINA 1"));
        assert_eq!(ops[1].entradas["em-andamento"], "2026-10-03T10:00:00Z");
        assert_eq!(ops[1].entradas["aguardando-maquina"], "2026-10-01T10:00:00Z");
        assert_eq!(ops[2].maquina.as_deref(), Some("Máquina 2"));
        assert_eq!(ops[1].etiquetas.len(), 2);
        assert_eq!(ops[1].etiquetas[0].nome, "Urgente");
        assert_eq!(ops[1].etiquetas[1].cor.as_deref(), Some("green"));
        assert!(ops[2].etiquetas.is_empty());
    }

    #[test]
    fn missing_list_is_an_error() {
        let raw: RawBoard = serde_json::from_value(serde_json::json!({
            "id": "b1", "name": "Q", "lists": [{"id": "l1", "name": "A"}]
        }))
        .unwrap();
        assert!(matches!(build(raw, &["B".into()]), Err(AppError::Config(_))));
    }
}
