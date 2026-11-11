-- Dados que o servidor busca no eGestor e no Trello e guarda para servir aos
-- programas da rede (que não consultam mais as APIs diretamente), e dados
-- próprios do Panorama de Injetados.

-- ---- Sincronizações --------------------------------------------------------
-- Uma linha por execução (automática ou pedida por um programa).
CREATE TABLE sincronizacoes (
    id           BIGSERIAL   PRIMARY KEY,
    fonte        TEXT        NOT NULL REFERENCES fontes (nome),
    iniciado_em  TIMESTAMPTZ NOT NULL,
    concluido_em TIMESTAMPTZ NOT NULL DEFAULT now(),
    ok           BOOLEAN     NOT NULL,
    erro         TEXT,
    -- Contagens do que foi gravado (ex.: {"produtos": 1520, "ops_abertas": 42}).
    resumo       JSONB       NOT NULL DEFAULT '{}'
);

CREATE INDEX sincronizacoes_fonte_idx ON sincronizacoes (fonte, concluido_em DESC);

-- ---- eGestor ---------------------------------------------------------------
-- `dados` é o JSON exatamente como o eGestor devolve.

CREATE TABLE egestor_produtos (
    codigo        BIGINT      PRIMARY KEY,
    dados         JSONB       NOT NULL,
    atualizado_em TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE egestor_categorias (
    codigo        BIGINT      PRIMARY KEY,
    dados         JSONB       NOT NULL,
    atualizado_em TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Relatório "composicoes": uma linha por produto, com todos os insumos
-- (o eGestor pode devolver o mesmo produto em várias linhas; aqui já vêm somadas).
CREATE TABLE egestor_composicoes (
    cod_produto   BIGINT      PRIMARY KEY,
    dados         JSONB       NOT NULL,
    atualizado_em TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Ordens de produção, com o detalhe completo (produtos e insumos).
-- `aberta` = estava na lista de OPs em aberto (situação 10, ativa) na última
-- sincronização. As que fecham continuam guardadas, com aberta = false.
CREATE TABLE egestor_producao (
    codigo        BIGINT      PRIMARY KEY,
    situacao      INTEGER,
    aberta        BOOLEAN     NOT NULL,
    dados         JSONB       NOT NULL,
    atualizado_em TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX egestor_producao_abertas_idx ON egestor_producao (codigo) WHERE aberta;

-- ---- Trello ----------------------------------------------------------------

-- Quadro sincronizado (uma linha só).
CREATE TABLE trello_quadro (
    unico         BOOLEAN     PRIMARY KEY DEFAULT TRUE CHECK (unico),
    id            TEXT        NOT NULL,
    nome          TEXT        NOT NULL,
    url           TEXT        NOT NULL,
    atualizado_em TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Listas servidas (TRELLO_LISTS), na ordem da resposta.
CREATE TABLE trello_listas (
    id    TEXT    PRIMARY KEY,
    nome  TEXT    NOT NULL,
    slug  TEXT    NOT NULL UNIQUE,
    ordem INTEGER NOT NULL
);

-- Cartões dessas listas. `dados` é o cartão completo no formato servido
-- (etiquetas, membros, checklists, campos personalizados).
CREATE TABLE trello_cartoes (
    id       TEXT    PRIMARY KEY,
    lista_id TEXT    NOT NULL REFERENCES trello_listas (id) ON DELETE CASCADE,
    posicao  INTEGER NOT NULL,
    nome     TEXT    NOT NULL,
    dados    JSONB   NOT NULL
);

CREATE INDEX trello_cartoes_lista_idx ON trello_cartoes (lista_id, posicao);

-- Histórico de entradas de cartões em listas (criação, cópia, movimentação).
-- Só cresce: a cada sincronização entram apenas as ações novas.
CREATE TABLE trello_movimentos (
    id        TEXT        PRIMARY KEY,
    quadro_id TEXT        NOT NULL,
    cartao_id TEXT        NOT NULL,
    lista_id  TEXT        NOT NULL,
    data      TIMESTAMPTZ NOT NULL
);

CREATE INDEX trello_movimentos_cartao_idx ON trello_movimentos (cartao_id, lista_id, data DESC);
CREATE INDEX trello_movimentos_quadro_idx ON trello_movimentos (quadro_id, data DESC);

-- ---- Panorama de Injetados -------------------------------------------------

-- Colunas visíveis e ordem de cada tabela configurável
-- ('necessidades', 'produtos'). `config` = {"colunas": [["Material", true], ...]}.
CREATE TABLE panorama_colunas (
    tabela        TEXT        PRIMARY KEY CHECK (tabela IN ('necessidades', 'produtos')),
    config        JSONB       NOT NULL,
    atualizado_em TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Simulações de produção (produtos que ainda não viraram OP). A ordem conta:
-- o estoque é usado na ordem do id.
CREATE TABLE panorama_simulacoes (
    id          BIGSERIAL        PRIMARY KEY,
    cod_produto BIGINT           NOT NULL,
    quantidade  DOUBLE PRECISION NOT NULL CHECK (quantidade > 0),
    criado_em   TIMESTAMPTZ      NOT NULL DEFAULT now()
);

-- Realizado parcial de uma linha de OP em aberto. A linha é a posição do item
-- na OP; se ela passar a ter outro produto, o lançamento deixa de valer.
CREATE TABLE panorama_parciais (
    op            BIGINT           NOT NULL,
    linha         INTEGER          NOT NULL CHECK (linha >= 0),
    cod_produto   BIGINT           NOT NULL,
    realizado     DOUBLE PRECISION NOT NULL CHECK (realizado > 0),
    atualizado_em TIMESTAMPTZ      NOT NULL DEFAULT now(),
    PRIMARY KEY (op, linha, cod_produto)
);
