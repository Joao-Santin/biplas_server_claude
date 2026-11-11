-- Colunas do Panorama de Injetados passam a ser de cada usuário (cabeçalho
-- `x-usuario`): uma disposição que serve para um não serve para outro.
-- `panorama_colunas` fica como estava e serve de ponto de partida para quem
-- ainda não configurou as suas.
CREATE TABLE panorama_colunas_usuario (
    usuario       TEXT        NOT NULL,
    tabela        TEXT        NOT NULL CHECK (tabela IN ('necessidades', 'produtos')),
    config        JSONB       NOT NULL,
    atualizado_em TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (usuario, tabela)
);
