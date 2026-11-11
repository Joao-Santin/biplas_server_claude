# biplas-server

Servidor geral da Biplas (Rust: `tokio` + `axum` + `reqwest` + `sqlx`).
Fornece dados para os computadores da empresa e recebe dados de diversas
fontes. Une o antigo `egestor-rust-server` e o `trello-rust-server`, mais uma
base em PostgreSQL.

**Quem consulta o eGestor e o Trello é só o servidor.** Ele sincroniza os dados
para o banco (sozinho, de tempos em tempos, ou quando um programa pede) e os
programas da rede leem do banco. Quando algo muda, todos os programas
conectados em `/api/notificacoes` são avisados na hora.

## Configuração

```bash
cp .env.example .env   # e preencha
cargo run --release
```

Sobe em `http://0.0.0.0:3000` (acessível pela rede). Cada integração é
opcional: o que não estiver configurado responde `503` explicando o que falta.
`GET /health` mostra o que está ativo.

| Variável | Para quê |
|---|---|
| `HOST`, `PORT` | Endereço de escuta (padrão `0.0.0.0:3000`) |
| `API_KEY` | Se definida, `/api/*` exige o header `x-api-key` |
| `DATABASE_URL` | PostgreSQL, ex.: `postgres://biplas:SENHA@localhost:5432/BiplasBD` |
| `TOKENEGESTOR`, `EGESTOR_BASE_URL` | eGestor (personal_token) |
| `TRELLO_KEY`, `TRELLO_TOKEN`, `TRELLO_BOARD`, `TRELLO_BOARD_ID`, `TRELLO_LISTS` | Trello |
| `SYNC_EGESTOR_MIN`, `SYNC_TRELLO_MIN` | Sincroniza ao subir e depois a cada N minutos (padrão 30 e 5; `0` = só quando pedirem) |

## Rotas

| Rota | O quê |
|---|---|
| `GET /health` | `{"status","banco","egestor","trello"}` |
| `GET /api/sync` | Situação da sincronização de cada fonte: `em_andamento`, `ultima` execução e `ultima_ok_em` (de quando são os dados) |
| `POST /api/sync/{egestor|trello}` | Pede uma sincronização agora (responde 202 na hora; se já estiver rodando, não começa outra) |
| `GET /api/notificacoes` | Server-Sent Events. Cada `data:` é um JSON com `tipo`: `conectado` (recarregue tudo), `atrasado` (idem), `sync` (`fonte`, `estado` andamento/concluido/erro, `etapa`, `fracao`, `erro`) ou `dados` (`recurso`, `origem` = `x-cliente-id` de quem mudou) |
| `GET /api/dados/egestor/{recurso}` | Dados gravados do eGestor, JSON original de cada registro: `produtos`, `categorias`, `composicoes` (uma linha por produto), `producao` (`?abertas=true` = só as OPs em aberto) |
| `GET|PUT /api/panorama/colunas/{necessidades|produtos}` | Colunas do Panorama de Injetados (`{"colunas": [...]}`; `null` = padrão) |
| `GET|POST /api/panorama/simulacoes`, `DELETE /api/panorama/simulacoes/{id}` | Simulações (`{"cod_produto","quantidade"}`) |
| `GET|PUT /api/panorama/parciais`, `DELETE /api/panorama/parciais/{op}/{linha}/{cod_produto}` | Realizado parcial das OPs (`{"op","linha","cod_produto","realizado"}`) |
| `/api/egestor/{recurso}` | Gateway direto da API do eGestor (para gravar no ERP ou consultar o que não é sincronizado): `/api/egestor/X` → `/v1/X`. Métodos, filtros (query), corpo e resposta passam sem alteração (inclui XML/PDF e multipart). Tabela de rotas em `src/egestor/mod.rs`. |
| `GET /api/trello/cartoes` | Quadro com as listas configuradas e seus cartões (do banco) |
| `GET /api/trello/listas/{lista}` | Uma lista: `aguardando-maquina`, `em-andamento`, `concluido` |
| `GET /api/trello/ops` | Cada cartão com OPs do título, máquina e entrada em cada lista |
| `GET /api/fontes` | Fontes de dados cadastradas |
| `POST /api/eventos/{fonte}?tipo=...` | Recebe qualquer JSON de uma fonte e guarda no banco (a fonte é cadastrada no primeiro envio) |
| `GET /api/eventos?fonte=&tipo=&pendentes=true&limite=50` | Eventos recebidos, mais recentes primeiro |
| `PUT /api/eventos/{id}/processado` | Marca como processado; corpo opcional `{"erro": "..."}` |

Exemplos:

```bash
curl "http://servidor:3000/api/dados/egestor/producao?abertas=true"
curl -X POST "http://servidor:3000/api/sync/egestor"
curl -N "http://servidor:3000/api/notificacoes"
curl "http://servidor:3000/api/egestor/producao?dtIni=2025-01-01&dtFim=2025-01-31"
curl "http://servidor:3000/api/egestor/nfe/123/xml" -o nota.xml
curl "http://servidor:3000/api/trello/ops"
curl -X POST "http://servidor:3000/api/eventos/maquina-9?tipo=leitura" \
  -H "Content-Type: application/json" -d '{"pecas": 120}'
```

### Migração dos programas antigos

- `egestor-rust-server` (porta 3000): `/api/X` agora é `/api/egestor/X`.
- `trello-rust-server` (porta 3001): `/api/cartoes` → `/api/trello/cartoes`,
  e o mesmo para `listas` e `ops`; a porta agora é a 3000.

## Banco de dados

Instale o PostgreSQL (https://www.postgresql.org/download/windows/) e crie o
banco:

```sql
CREATE USER biplas WITH PASSWORD 'SENHA';
CREATE DATABASE "BiplasBD" OWNER biplas ENCODING 'UTF8' TEMPLATE template0;
```

Defina `DATABASE_URL` no `.env`. Ao subir, o servidor aplica sozinho as
migrations de `migrations/` (embutidas no binário). Para evoluir o esquema,
crie um novo arquivo `migrations/AAAAMMDDHHMMSS_descricao.sql` — nunca edite
um que já rodou.

Tabelas:

- `fontes`, `eventos`: dados recebidos de outras fontes (JSON bruto, com
  `processado_em`/`erro` para quem for processar).
- `sincronizacoes`: cada execução, com resultado, erro e contagens.
- `egestor_produtos`, `egestor_categorias`, `egestor_composicoes`,
  `egestor_producao`: JSON do eGestor por código. As OPs que fecham continuam
  guardadas com `aberta = false`. Uma sincronização é gravada numa transação só.
- `trello_quadro`, `trello_listas`, `trello_cartoes`: foto atual do quadro;
  `trello_movimentos`: histórico de entradas em listas (só cresce; a cada vez
  baixa apenas as ações novas).
- `panorama_colunas`, `panorama_simulacoes`, `panorama_parciais`: dados do
  Panorama de Injetados. Ao sincronizar o eGestor, os parciais de OPs que
  fecharam (ou de linhas que mudaram de produto) são apagados.

## Estrutura

```
src/
├── main.rs          # inicialização
├── config.rs        # leitura do .env
├── state.rs         # AppState (banco + clientes das integrações)
├── error.rs         # AppError -> {"erro": "..."}
├── db.rs            # pool PostgreSQL + migrations
├── sincronizacao/
│   ├── mod.rs       # agendamento, /api/sync, /api/notificacoes (SSE)
│   ├── egestor.rs   # eGestor → banco (respeita 60 req/min)
│   └── trello.rs    # Trello → banco
├── routes/
│   ├── mod.rs       # router geral, /health, x-api-key
│   ├── dados.rs     # /api/dados/egestor/*
│   ├── panorama.rs  # /api/panorama/*
│   └── eventos.rs   # /api/fontes, /api/eventos
├── egestor/
│   ├── mod.rs       # tabela de rotas + handler de proxy
│   ├── auth.rs      # OAuth2: personal_token -> access_token (cache + renovação)
│   └── client.rs    # cliente HTTP que repassa tudo
└── trello/
    ├── mod.rs       # rotas (leem do banco)
    ├── client.rs    # API do Trello
    └── model.rs     # formato servido (quadro/listas/cartões/OPs)
migrations/          # esquema do PostgreSQL
```
