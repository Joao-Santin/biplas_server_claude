//! Conexão com o PostgreSQL.
//!
//! As migrations ficam em `migrations/` e são embutidas no binário em tempo
//! de compilação (`sqlx::migrate!`). Ao subir, o servidor aplica as que
//! ainda não rodaram — basta criar um novo arquivo
//! `migrations/AAAAMMDDHHMMSS_descricao.sql` para evoluir o esquema.
//! Nunca edite uma migration que já rodou: crie outra.

use std::time::Duration;

use sqlx::{PgPool, postgres::PgPoolOptions};

pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(5))
        .connect(database_url)
        .await?;

    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}
