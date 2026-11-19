//! Persistência local SQLite em modo WAL (`Write-Ahead Logging`) com esquema versionado.

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use std::str::FromStr;

pub const SCHEMA_VERSION: i64 = 1;

#[derive(Clone)]
pub struct LocalDatabase {
    pool: SqlitePool,
}

impl LocalDatabase {
    pub async fn open(path: &str) -> anyhow::Result<Self> {
        if path != ":memory:" {
            if let Some(parent) = std::path::Path::new(path).parent() {
                if !parent.as_os_str().is_empty() {
                    let _ = std::fs::create_dir_all(parent);
                }
            }
        }
        let conn_url = if path == ":memory:" {
            "sqlite::memory:".to_string()
        } else {
            format!("sqlite://{path}")
        };
        let opts = SqliteConnectOptions::from_str(&conn_url)?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await?;

        let db = Self { pool };
        db.migrate().await?;
        Ok(db)
    }

    async fn migrate(&self) -> anyhow::Result<()> {
        for stmt in [
            "CREATE TABLE IF NOT EXISTS schema_meta (version INTEGER PRIMARY KEY)",
            "INSERT OR IGNORE INTO schema_meta (version) VALUES (1)",
            "CREATE TABLE IF NOT EXISTS zen_points (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                machine_id TEXT NOT NULL,
                char_name TEXT NOT NULL,
                ts_utc_s INTEGER NOT NULL,
                zen INTEGER NOT NULL
            )",
            "CREATE INDEX IF NOT EXISTS idx_zen_points_ts ON zen_points(ts_utc_s)",
            "CREATE TABLE IF NOT EXISTS pk_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts_utc_s INTEGER NOT NULL,
                machine_id TEXT NOT NULL,
                killer TEXT NOT NULL,
                killer_kind TEXT NOT NULL,
                victim TEXT NOT NULL
            )",
            "CREATE TABLE IF NOT EXISTS command_audit (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts_utc_s INTEGER NOT NULL,
                actor TEXT NOT NULL,
                command_summary TEXT NOT NULL
            )",
        ] {
            sqlx::query(stmt).execute(&self.pool).await?;
        }
        Ok(())
    }

    pub async fn record_zen_point(
        &self,
        machine_id: &str,
        char_name: &str,
        ts_utc_s: u64,
        zen: u64,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO zen_points (machine_id, char_name, ts_utc_s, zen) VALUES (?, ?, ?, ?)",
        )
        .bind(machine_id)
        .bind(char_name)
        .bind(ts_utc_s as i64)
        .bind(zen as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn record_pk_event(
        &self,
        ts_utc_s: u64,
        machine_id: &str,
        killer: &str,
        killer_kind: &str,
        victim: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO pk_events (ts_utc_s, machine_id, killer, killer_kind, victim) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(ts_utc_s as i64)
        .bind(machine_id)
        .bind(killer)
        .bind(killer_kind)
        .bind(victim)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn record_audit(
        &self,
        ts_utc_s: u64,
        actor: &str,
        command_summary: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO command_audit (ts_utc_s, actor, command_summary) VALUES (?, ?, ?)",
        )
        .bind(ts_utc_s as i64)
        .bind(actor)
        .bind(command_summary)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn count_zen_points(&self) -> anyhow::Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) FROM zen_points")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get::<i64, _>(0))
    }

    pub async fn count_audits(&self) -> anyhow::Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) FROM command_audit")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get::<i64, _>(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sqlite_wal_migrations_and_inserts_succeed() {
        let db = LocalDatabase::open(":memory:").await.unwrap();
        db.record_zen_point("PC1-PRINCIPAL", "x9901", 1_795_000_000, 850_000_000)
            .await
            .unwrap();
        db.record_audit(1_795_000_005, "desktop-operator", "Parada de emergencia")
            .await
            .unwrap();

        assert_eq!(db.count_zen_points().await.unwrap(), 1);
        assert_eq!(db.count_audits().await.unwrap(), 1);
    }
}
