//! CRUD for the metadata entities: projects, sources, pixels, and exception triage.

use analytics_api::{Pixel, Project, Source, default_kind};
use chrono::Utc;
use redb::ReadableTable;

use super::Store;
use super::tables::{EXCEPTION_TRIAGE, PIXELS, PROJECTS, SOURCES, STORAGE_ADVICE, triage_key};
use super::triage::ExceptionTriage;
use crate::errors::{Result, ResultExt};

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

    /// Delete a project and everything that referenced it in a single write
    /// transaction: its pixels are removed and its sources are unassigned, so a
    /// partial failure can never leave a half-deleted project. Historical events
    /// remain under their (now unassigned) sources. Returns `false` if the project
    /// does not exist.
    pub fn delete_project_cascade(&self, id: &str) -> Result<bool> {
        let txn = self.db.begin_write().or_system_err(STORAGE_ADVICE)?;
        let existed = {
            let mut projects = txn.open_table(PROJECTS).or_system_err(STORAGE_ADVICE)?;
            if projects.get(id).or_system_err(STORAGE_ADVICE)?.is_none() {
                false
            } else {
                projects.remove(id).or_system_err(STORAGE_ADVICE)?;
                true
            }
        };
        if existed {
            // Unassign every source pointing at this project.
            {
                let mut sources = txn.open_table(SOURCES).or_system_err(STORAGE_ADVICE)?;
                let mut updates: Vec<(String, Vec<u8>)> = Vec::new();
                for item in sources.iter().or_system_err(STORAGE_ADVICE)? {
                    let (key, value) = item.or_system_err(STORAGE_ADVICE)?;
                    let mut source: Source =
                        serde_json::from_slice(value.value()).or_system_err(STORAGE_ADVICE)?;
                    if source.project_id.as_deref() == Some(id) {
                        source.project_id = None;
                        let bytes = serde_json::to_vec(&source).or_system_err(STORAGE_ADVICE)?;
                        updates.push((key.value().to_string(), bytes));
                    }
                }
                for (key, bytes) in updates {
                    sources.insert(key.as_str(), bytes.as_slice()).or_system_err(STORAGE_ADVICE)?;
                }
            }
            // Delete every pixel belonging to this project.
            {
                let mut pixels = txn.open_table(PIXELS).or_system_err(STORAGE_ADVICE)?;
                let mut to_delete: Vec<String> = Vec::new();
                for item in pixels.iter().or_system_err(STORAGE_ADVICE)? {
                    let (key, value) = item.or_system_err(STORAGE_ADVICE)?;
                    let pixel: Pixel =
                        serde_json::from_slice(value.value()).or_system_err(STORAGE_ADVICE)?;
                    if pixel.project_id == id {
                        to_delete.push(key.value().to_string());
                    }
                }
                for key in to_delete {
                    pixels.remove(key.as_str()).or_system_err(STORAGE_ADVICE)?;
                }
            }
        }
        txn.commit().or_system_err(STORAGE_ADVICE)?;
        Ok(existed)
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

    /// Apply `f` to an existing source and persist it in one write transaction.
    /// Returns the updated source, or `None` if the URI is unknown.
    pub fn mutate_source<F: FnOnce(&mut Source)>(
        &self,
        uri: &str,
        f: F,
    ) -> Result<Option<Source>> {
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
    pub fn get_triage(
        &self,
        project_id: &str,
        group_id: &str,
    ) -> Result<Option<ExceptionTriage>> {
        self.get_json(EXCEPTION_TRIAGE, &triage_key(project_id, group_id))
    }
}
