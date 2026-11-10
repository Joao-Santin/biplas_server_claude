-- Base do banco do servidor Biplas.
--
-- `fontes`: de onde os dados vêm (sistemas externos, máquinas, outros
-- programas da rede). `eventos`: tudo o que alguma fonte enviou ao servidor,
-- guardado como JSON bruto, para ser processado depois e virar dado
-- estruturado em tabelas próprias.

CREATE TABLE fontes (
    id          SMALLSERIAL PRIMARY KEY,
    -- Identificador usado na URL: só minúsculas, dígitos, '-' e '_'.
    nome        TEXT        NOT NULL UNIQUE CHECK (nome ~ '^[a-z0-9_-]{1,64}$'),
    descricao   TEXT        NOT NULL DEFAULT '',
    ativa       BOOLEAN     NOT NULL DEFAULT TRUE,
    criado_em   TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO fontes (nome, descricao) VALUES
    ('egestor', 'ERP eGestor (webhooks e sincronizações)'),
    ('trello',  'Trello - quadro Programação Máquinas');

CREATE TABLE eventos (
    id            BIGSERIAL   PRIMARY KEY,
    fonte_id      SMALLINT    NOT NULL REFERENCES fontes (id),
    -- Livre, definido por quem envia (ex.: 'venda.criada', 'leitura').
    tipo          TEXT        NOT NULL DEFAULT '',
    payload       JSONB       NOT NULL,
    -- IP de quem enviou.
    origem        TEXT,
    recebido_em   TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Preenchidos por quem processar o evento.
    processado_em TIMESTAMPTZ,
    erro          TEXT
);

CREATE INDEX eventos_fonte_recebido_idx ON eventos (fonte_id, recebido_em DESC);
CREATE INDEX eventos_pendentes_idx ON eventos (recebido_em) WHERE processado_em IS NULL;
