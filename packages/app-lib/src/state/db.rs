use crate::state::DirectoryInfo;
use sha2::{Digest, Sha384};
use sqlx::migrate::{Migration, Migrator};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions,
};
use sqlx::{Pool, Sqlite};
use std::path::Path;
use std::time::Duration;

static MIGRATOR: Migrator = sqlx::migrate!();

const INITIAL_MIGRATION_VERSION: i64 = 20240711194701;

// This migration was changed by the launcher rebrand after it had already
// shipped. Keep the checksums of the original LF and CRLF variants so existing
// installations can move to the current canonical migration without losing
// their database.
const LEGACY_INITIAL_MIGRATION_CHECKSUMS: &[&str] = &[
    "49364b3e1b0d0169579ed93eb1f8e215216b84300a816891d0d922d3e03c69101e17e2bbe91ac1f54234c77cbd6b8bc3",
    "d95bfef1c3b2b530d2efd810202c85f93a9342ab40497b15653eea9b129806333cf610eebcecfa91accaa53a14bfc5df",
];

pub(crate) async fn connect(
    app_identifier: &str,
) -> crate::Result<Pool<Sqlite>> {
    let settings_dir = DirectoryInfo::initial_settings_dir_path(app_identifier)
        .ok_or(crate::ErrorKind::FSError(
            "Could not find valid config dir".to_string(),
        ))?;

    crate::util::io::create_dir_all(&settings_dir).await?;

    let db_path = settings_dir.join("app.db");

    connect_app_db(&db_path).await
}

async fn connect_app_db(db_path: &Path) -> crate::Result<Pool<Sqlite>> {
    super::db_backup::maybe_backup_existing_app_db(db_path).await?;
    open_migrated_app_db(db_path).await
}

async fn open_migrated_app_db(db_path: &Path) -> crate::Result<Pool<Sqlite>> {
    let pool = open_app_db_pool(db_path).await?;

    if let Err(err) = stale_data_cleanup(&pool).await {
        tracing::warn!(
            "Failed to clean up stale data from state database before migrations: {err}"
        );
    }

    reconcile_compatible_migration_checksums(&pool).await?;
    MIGRATOR.run(&pool).await?;
    record_current_app_version(&pool).await?;

    if let Err(err) = stale_data_cleanup(&pool).await {
        tracing::warn!(
            "Failed to clean up stale data from state database: {err}"
        );
    }

    Ok(pool)
}

/// Reconciles historical migration checksums that differ only because of line
/// endings, plus the known pre-rebrand form of the initial migration.
///
/// SQLx hashes the raw migration bytes, so an otherwise identical migration
/// built with LF, CRLF, or mixed line endings receives a different checksum.
/// Unknown checksums are deliberately left untouched for SQLx to reject.
async fn reconcile_compatible_migration_checksums(
    pool: &Pool<Sqlite>,
) -> crate::Result<()> {
    let has_migrations_table: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations')",
    )
    .fetch_one(pool)
    .await?;

    if !has_migrations_table {
        return Ok(());
    }

    let applied_migrations: Vec<(i64, Vec<u8>)> =
        sqlx::query_as("SELECT version, checksum FROM _sqlx_migrations")
            .fetch_all(pool)
            .await?;

    for (version, applied_checksum) in applied_migrations {
        let Some(migration) = MIGRATOR
            .iter()
            .find(|migration| migration.version == version)
        else {
            continue;
        };
        let current_checksum: &[u8] = migration.checksum.as_ref();

        if applied_checksum.as_slice() == current_checksum {
            continue;
        }

        if !is_compatible_migration_checksum(
            version,
            &applied_checksum,
            migration,
        ) {
            continue;
        }

        sqlx::query(
            "UPDATE _sqlx_migrations SET checksum = ? WHERE version = ?",
        )
        .bind(current_checksum)
        .bind(version)
        .execute(pool)
        .await?;

        tracing::warn!(
            version,
            "Reconciled a compatible historical migration checksum"
        );
    }

    Ok(())
}

fn is_compatible_migration_checksum(
    version: i64,
    applied_checksum: &[u8],
    migration: &Migration,
) -> bool {
    let normalized_lf = migration.sql.replace("\r\n", "\n").replace('\r', "\n");
    let normalized_crlf = normalized_lf.replace('\n', "\r\n");

    if checksum_matches(applied_checksum, normalized_lf.as_bytes())
        || checksum_matches(applied_checksum, normalized_crlf.as_bytes())
    {
        return true;
    }

    version == INITIAL_MIGRATION_VERSION
        && LEGACY_INITIAL_MIGRATION_CHECKSUMS
            .contains(&checksum_as_hex(applied_checksum).as_str())
}

fn checksum_matches(checksum: &[u8], contents: &[u8]) -> bool {
    let calculated: [u8; 48] = Sha384::digest(contents).into();
    checksum == calculated
}

fn checksum_as_hex(checksum: &[u8]) -> String {
    use std::fmt::Write;

    checksum.iter().fold(
        String::with_capacity(checksum.len() * 2),
        |mut output, byte| {
            let _ = write!(output, "{byte:02x}");
            output
        },
    )
}

async fn open_app_db_pool(db_path: &Path) -> crate::Result<Pool<Sqlite>> {
    let conn_options = SqliteConnectOptions::new()
        .filename(db_path)
        .busy_timeout(Duration::from_secs(30))
        .journal_mode(SqliteJournalMode::Wal)
        .optimize_on_close(true, None)
        .create_if_missing(true);

    Ok(SqlitePoolOptions::new()
        .max_connections(100)
        .connect_with(conn_options)
        .await?)
}

async fn record_current_app_version(pool: &Pool<Sqlite>) -> crate::Result<()> {
    sqlx::query!(
        "
		INSERT INTO app_metadata (key, value, updated_at)
		VALUES ('app_version', ?, unixepoch())
		ON CONFLICT(key) DO UPDATE SET
			value = excluded.value,
			updated_at = excluded.updated_at
		",
        env!("CARGO_PKG_VERSION"),
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Cleans up data from the database that is no longer referenced, but must be
/// kept around for a little while to allow users to recover from accidental
/// deletions.
async fn stale_data_cleanup(pool: &Pool<Sqlite>) -> crate::Result<()> {
    let mut tx = pool.begin().await?;

    let has_skin_tables = sqlx::query!(
		"SELECT COUNT(*) AS \"count!: i64\" FROM sqlite_master WHERE type = 'table' AND name IN ('custom_minecraft_skins', 'minecraft_users')",
	)
	.fetch_one(&mut *tx)
	.await?
	.count == 2;

    if has_skin_tables {
        sqlx::query!(
			"DELETE FROM custom_minecraft_skins WHERE minecraft_user_uuid NOT IN (SELECT uuid FROM minecraft_users)"
		)
		.execute(&mut *tx)
		.await?;
    }

    tx.commit().await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn initial_migration() -> &'static Migration {
        MIGRATOR
            .iter()
            .find(|migration| migration.version == INITIAL_MIGRATION_VERSION)
            .expect("initial migration should be embedded")
    }

    fn checksum(contents: &[u8]) -> Vec<u8> {
        Sha384::digest(contents).to_vec()
    }

    fn decode_hex(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let pair = std::str::from_utf8(pair).expect("ASCII hex");
                u8::from_str_radix(pair, 16).expect("valid hex")
            })
            .collect()
    }

    #[test]
    fn accepts_lf_and_crlf_variants_of_the_same_migration() {
        let migration = initial_migration();
        let lf = migration.sql.replace("\r\n", "\n").replace('\r', "\n");
        let crlf = lf.replace('\n', "\r\n");

        assert!(is_compatible_migration_checksum(
            migration.version,
            &checksum(lf.as_bytes()),
            migration,
        ));
        assert!(is_compatible_migration_checksum(
            migration.version,
            &checksum(crlf.as_bytes()),
            migration,
        ));
    }

    #[test]
    fn accepts_only_the_known_legacy_initial_migration() {
        let migration = initial_migration();
        let legacy_checksum = decode_hex(LEGACY_INITIAL_MIGRATION_CHECKSUMS[0]);

        assert!(is_compatible_migration_checksum(
            INITIAL_MIGRATION_VERSION,
            &legacy_checksum,
            migration,
        ));
        assert!(!is_compatible_migration_checksum(
            INITIAL_MIGRATION_VERSION + 1,
            &legacy_checksum,
            migration,
        ));
    }

    #[test]
    fn rejects_an_unknown_content_change() {
        let migration = initial_migration();
        let changed_checksum = checksum(
            format!("{}\n-- unknown change", migration.sql).as_bytes(),
        );

        assert!(!is_compatible_migration_checksum(
            migration.version,
            &changed_checksum,
            migration,
        ));
    }
}
