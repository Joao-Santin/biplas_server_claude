-- Histórico das ações feitas pelos usuários nos programas da rede
-- (realizado parcial, simulações, pedidos de sincronização, colunas...).
-- Quem fez vem dos headers x-programa, x-usuario, x-maquina e x-cliente-id;
-- o IP é o da conexão.
CREATE TABLE historico (
    id         BIGSERIAL   PRIMARY KEY,
    em         TIMESTAMPTZ NOT NULL DEFAULT now(),
    programa   TEXT,
    usuario    TEXT,
    maquina    TEXT,
    ip         TEXT,
    cliente_id TEXT,
    -- Ex.: 'parcial', 'simulacao_criada', 'simulacao_excluida', 'sync', 'colunas'.
    acao       TEXT        NOT NULL,
    -- O que mudou, conforme a ação (ex.: {"op": 1, "anterior": 10, "realizado": 12}).
    detalhe    JSONB       NOT NULL DEFAULT '{}'
);

CREATE INDEX historico_id_idx ON historico (id DESC);
CREATE INDEX historico_usuario_idx ON historico (usuario, id DESC);
CREATE INDEX historico_cliente_idx ON historico (cliente_id, id DESC);
