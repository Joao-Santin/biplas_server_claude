# biplas-server

Servidor geral da Biplas (Rust: `tokio` + `axum` + `reqwest` + `sqlx`).
Fornece dados para os computadores da empresa e recebe dados de diversas
fontes. Une o antigo `egestor-rust-server` e o `trello-rust-server`, mais uma
base em PostgreSQL.

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

## Rotas

| Rota | O quê |
|---|---|
| `GET /health` | `{"status","banco","egestor","trello"}` |
| `/api/egestor/{recurso}` | Gateway da API do eGestor: `/api/egestor/X` → `/v1/X`. Métodos, filtros (query), corpo e resposta passam sem alteração (inclui XML/PDF e multipart). Tabela de rotas em `src/egestor/mod.rs`. |
| `GET /api/trello/cartoes` | Quadro com as listas configuradas e seus cartões |
| `GET /api/trello/listas/{lista}` | Uma lista: `aguardando-maquina`, `em-andamento`, `concluido` |
| `GET /api/trello/ops` | Cada cartão com OPs do título, máquina e entrada em cada lista |
| `GET /api/fontes` | Fontes de dados cadastradas |
| `POST /api/eventos/{fonte}?tipo=...` | Recebe qualquer JSON de uma fonte e guarda no banco (a fonte é cadastrada no primeiro envio) |
| `GET /api/eventos?fonte=&tipo=&pendentes=true&limite=50` | Eventos recebidos, mais recentes primeiro |
| `PUT /api/eventos/{id}/processado` | Marca como processado; corpo opcional `{"erro": "..."}` |

Exemplos:

```bash
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

Esquema inicial: `fontes` (de onde vêm os dados) e `eventos` (JSON bruto
recebido, com `processado_em`/`erro` para quem for processar).

## Estrutura

```
src/
├── main.rs          # inicialização
├── config.rs        # leitura do .env
├── state.rs         # AppState (banco + clientes das integrações)
├── error.rs         # AppError -> {"erro": "..."}
├── db.rs            # pool PostgreSQL + migrations
├── routes/
│   ├── mod.rs       # router geral, /health, x-api-key
│   └── eventos.rs   # /api/fontes, /api/eventos
├── egestor/
│   ├── mod.rs       # tabela de rotas + handler de proxy
│   ├── auth.rs      # OAuth2: personal_token -> access_token (cache + renovação)
│   └── client.rs    # cliente HTTP que repassa tudo
└── trello/
    ├── mod.rs       # rotas
    ├── client.rs    # API do Trello + cache do histórico
    └── model.rs     # formato servido (quadro/listas/cartões/OPs)
migrations/          # esquema do PostgreSQL
```
