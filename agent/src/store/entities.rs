//! CRUD for the metadata entities: projects, sources, pixels, and exception triage.

use analytics_api::{Pixel, Project, Source, default_kind};
use chrono::Utc;
use duckdb::params;

use super::triage::ExceptionTriage;
use super::{STORAGE_ADVICE, Store};
use crate::errors::{Result, ResultExt};

pub(super) const PROJECTS: &str = "projects";
pub(super) const SOURCES: &str = "sources";
pub(super) const PIXELS: &str = "pixels";
pub(super) const EXCEPTION_TRIAGE: &str = "exception_triage";
pub(super) const META: &str = "meta";

/// Composite key for the triage table. The unit-separator byte cannot appear in
/// ULIDs or hostnames, so it is a safe delimiter.
fn triage_key(project_id: &str, group_id: &str) -> String {
    format!("{project_id}\u{1f}{group_id}")
}

impl Store {
    // ------------------------------------------------------------- projects
    pub fn put_project(&self, project: &Project) -> Result<()> {
        self.put_json(PROJECTS, &project.id, project)
    }
    pub fn get_project(&self, id: &str) -> Result<Option<Project>> {
        self.get_json(PROJECTS, id)
    }
    pub fn list_projects(&self) -> Result<Vec<Project>> {
        self.list_json(PROJECTS)
    }
    pub fn delete_project(&self, id: &str) -> Result<bool> {
        self.delete_key(PROJECTS, id)
    }

    /// Delete a project and everything that referenced it in a single
    /// transaction: its pixels are removed and its sources are unassigned, so a
    /// partial failure can never leave a half-deleted project. Historical events
    /// remain under their (now unassigned) sources. Returns `false` if the project
    /// does not exist.
    pub fn delete_project_cascade(&self, id: &str) -> Result<bool> {
        self.with_conn(|conn| {
            let txn = conn.transaction().or_system_err(STORAGE_ADVICE)?;
            let existed = txn
                .execute("DELETE FROM projects WHERE key = ?", params![id])
                .or_system_err(STORAGE_ADVICE)?
                > 0;
            if existed {
                // Unassign every source pointing at this project. The entity
                // rows are JSON, so mutate through serde rather than SQL.
                let assigned: Vec<(String, String)> = {
                    let mut stmt = txn
                        .prepare("SELECT key, data FROM sources")
                        .or_system_err(STORAGE_ADVICE)?;
                    let rows = stmt
                        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                        .or_system_err(STORAGE_ADVICE)?;
                    let mut out = Vec::new();
                    for row in rows {
                        out.push(row.or_system_err(STORAGE_ADVICE)?);
                    }
                    out
                };
                for (key, data) in assigned {
                    let mut source: Source =
                        serde_json::from_str(&data).or_system_err(STORAGE_ADVICE)?;
                    if source.project_id.as_deref() == Some(id) {
                        source.project_id = None;
                        let data = serde_json::to_string(&source).or_system_err(STORAGE_ADVICE)?;
                        txn.execute(
                            "INSERT OR REPLACE INTO sources VALUES (?, ?)",
                            params![key, data],
                        )
                        .or_system_err(STORAGE_ADVICE)?;
                    }
                }
                // Delete every pixel belonging to this project.
                txn.execute(
                    "DELETE FROM pixels WHERE json_extract_string(data, '$.project_id') = ?",
                    params![id],
                )
                .or_system_err(STORAGE_ADVICE)?;
            }
            txn.commit().or_system_err(STORAGE_ADVICE)?;
            Ok(existed)
        })
    }

    // -------------------------------------------------------------- sources
    pub fn put_source(&self, source: &Source) -> Result<()> {
        self.put_json(SOURCES, &source.uri, source)
    }
    pub fn get_source(&self, uri: &str) -> Result<Option<Source>> {
        self.get_json(SOURCES, uri)
    }
    pub fn list_sources(&self) -> Result<Vec<Source>> {
        self.list_json(SOURCES)
    }
    pub fn delete_source(&self, uri: &str) -> Result<bool> {
        self.delete_key(SOURCES, uri)
    }

    /// Apply `f` to an existing source and persist it in one transaction.
    /// Returns the updated source, or `None` if the URI is unknown.
    pub fn mutate_source<F: FnOnce(&mut Source)>(&self, uri: &str, f: F) -> Result<Option<Source>> {
        self.mutate_json(SOURCES, uri, f)
    }

    /// Register a newly-seen source as unassigned, if it does not already exist.
    /// Called from the single-threaded ingest writer, so the check-then-insert is
    /// race-free.
    pub fn register_source_if_absent(&self, uri: &str) -> Result<()> {
        if self.get_source(uri)?.is_some() {
            return Ok(());
        }
        let now = Utc::now();
        self.put_source(&Source {
            uri: uri.to_string(),
            project_id: None,
            kind: default_kind(uri),
            display_name: None,
            created_at: now,
            first_seen: Some(now),
            last_seen: Some(now),
        })
    }

    // --------------------------------------------------------------- pixels
    pub fn put_pixel(&self, pixel: &Pixel) -> Result<()> {
        self.put_json(PIXELS, &pixel.id, pixel)
    }
    pub fn get_pixel(&self, id: &str) -> Result<Option<Pixel>> {
        self.get_json(PIXELS, id)
    }
    pub fn list_pixels(&self) -> Result<Vec<Pixel>> {
        self.list_json(PIXELS)
    }
    pub fn delete_pixel(&self, id: &str) -> Result<bool> {
        self.delete_key(PIXELS, id)
    }

    // ------------------------------------------------------ exception triage
    pub fn put_triage(
        &self,
        project_id: &str,
        group_id: &str,
        triage: &ExceptionTriage,
    ) -> Result<()> {
        self.put_json(EXCEPTION_TRIAGE, &triage_key(project_id, group_id), triage)
    }
    pub fn get_triage(&self, project_id: &str, group_id: &str) -> Result<Option<ExceptionTriage>> {
        self.get_json(EXCEPTION_TRIAGE, &triage_key(project_id, group_id))
    }

    /// Create-or-update a triage record in a single transaction: `f` is
    /// applied to the existing record, or to a fresh empty one when none exists,
    /// then the result is persisted, so two concurrent edits (e.g. resolving and
    /// muting at once) can't clobber each other's axis. Returns the stored record.
    pub fn update_triage<F: FnOnce(&mut ExceptionTriage)>(
        &self,
        project_id: &str,
        group_id: &str,
        f: F,
    ) -> Result<ExceptionTriage> {
        let key = triage_key(project_id, group_id);
        self.with_conn(|conn| {
            let txn = conn.transaction().or_system_err(STORAGE_ADVICE)?;
            let mut triage: ExceptionTriage = {
                let mut stmt = txn
                    .prepare("SELECT data FROM exception_triage WHERE key = ?")
                    .or_system_err(STORAGE_ADVICE)?;
                let mut rows = stmt.query(params![key]).or_system_err(STORAGE_ADVICE)?;
                match rows.next().or_system_err(STORAGE_ADVICE)? {
                    Some(row) => {
                        let data: String = row.get(0).or_system_err(STORAGE_ADVICE)?;
                        serde_json::from_str(&data).or_system_err(STORAGE_ADVICE)?
                    }
                    None => ExceptionTriage {
                        resolved_at: None,
                        muted_at: None,
                        note: None,
                        updated_at: Utc::now(),
                        updated_by: None,
                    },
                }
            };
            f(&mut triage);
            let data = serde_json::to_string(&triage).or_system_err(STORAGE_ADVICE)?;
            txn.execute(
                "INSERT OR REPLACE INTO exception_triage VALUES (?, ?)",
                params![key, data],
            )
            .or_system_err(STORAGE_ADVICE)?;
            txn.commit().or_system_err(STORAGE_ADVICE)?;
            Ok(triage)
        })
    }
}
