-- Uma produção pode ter volumes com quantidades diferentes: o realizado passa
-- a ser a soma de vários tipos de volume, [{"volumes": 1, "qtd_volume": 2000},
-- {"volumes": 3, "qtd_volume": 1000}] = 5000. `volumes`/`qtd_volume` ficam
-- preenchidos só quando há um tipo só (e nos lançamentos antigos).

ALTER TABLE panorama_parciais ADD COLUMN grupos JSONB;
ALTER TABLE panorama_parciais_lancamentos ADD COLUMN grupos JSONB;
