use sqlite::State;
use tracing_batteries::prelude::*;

pub fn run_migrations(connection: &sqlite::Connection) -> Result<(), sqlite::Error> {
    debug!("Preparing to run database schema migrations...");
    ensure_migration_table(connection)?;

    let migrations = [Migration {
        version: 1,
        query: "CREATE TABLE IF NOT EXISTS pages (
          domain TEXT NOT NULL,
          path TEXT NOT NULL,
          likes INTEGER NOT NULL DEFAULT 0,
          views INTEGER NOT NULL DEFAULT 0,
          PRIMARY KEY (domain, path) ON CONFLICT IGNORE
        )",
    }];

    let current_version = get_current_version(connection)?;
    info!(
        "Database schema is currently v{} (latest: v{})",
        current_version,
        migrations
            .iter()
            .map(|m| m.version)
            .max()
            .unwrap_or_default()
    );

    for migration in migrations.iter().filter(|m| m.version > current_version) {
        info!(
            {
                migration.version = migration.version,
                migration.query = &migration.query
            },
            "Applying database schema migration v{}",
            migration.version
        );
        apply_migration(connection, migration)?;
    }

    Ok(())
}

fn ensure_migration_table(connection: &sqlite::Connection) -> Result<(), sqlite::Error> {
    connection.execute(
        "CREATE TABLE IF NOT EXISTS migrations (
        version INTEGER PRIMARY KEY,
        query TEXT NOT NULL
    )",
    )
}

fn get_current_version(connection: &sqlite::Connection) -> Result<i64, sqlite::Error> {
    let mut statement =
        connection.prepare("SELECT version FROM migrations ORDER BY version DESC LIMIT 1")?;

    if State::Row == statement.next()? {
        Ok(statement.read("version")?)
    } else {
        Ok(0)
    }
}

fn apply_migration(
    connection: &sqlite::Connection,
    migration: &Migration,
) -> Result<(), sqlite::Error> {
    connection.execute(migration.query)?;

    let mut statement =
        connection.prepare("INSERT INTO migrations (version, query) VALUES (?, ?)")?;
    statement.bind((1, migration.version))?;
    statement.bind((2, migration.query))?;

    statement.next()?;

    Ok(())
}

struct Migration {
    version: i64,
    query: &'static str,
}
