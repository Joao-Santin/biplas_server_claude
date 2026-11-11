-- Realizado parcial lançado por volumes: o total é volumes × quantidade por
-- volume. O lançamento é sempre do total; cada confirmação fica registrada
-- com o total anterior, para mostrar quanto entrou de cada vez.

ALTER TABLE panorama_parciais
    ADD COLUMN volumes    DOUBLE PRECISION CHECK (volumes >= 0),
    ADD COLUMN qtd_volume DOUBLE PRECISION CHECK (qtd_volume > 0);

CREATE TABLE panorama_parciais_lancamentos (
    id          BIGSERIAL        PRIMARY KEY,
    op          BIGINT           NOT NULL,
    linha       INTEGER          NOT NULL,
    cod_produto BIGINT           NOT NULL,
    em          TIMESTAMPTZ      NOT NULL DEFAULT now(),
    volumes     DOUBLE PRECISION,
    qtd_volume  DOUBLE PRECISION,
    -- Total depois desta confirmação (0 = lançamento apagado) e antes dela.
    realizado   DOUBLE PRECISION NOT NULL,
    anterior    DOUBLE PRECISION NOT NULL DEFAULT 0,
    usuario     TEXT,
    maquina     TEXT
);

CREATE INDEX panorama_parciais_lancamentos_op_idx ON panorama_parciais_lancamentos (op, linha, cod_produto, id);
