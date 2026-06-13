//! CRUD for the metadata entities: projects, sources, pixels, and exception triage.

use analytics_api::{Pixel, Project, Source, default_kind};
use chrono::Utc;

use super::Store;
use super::tables::{EXCEPTION_TRIAGE, PIXELS, PROJECTS, SOURCES, triage_key};
use super::triage::ExceptionTriage;
use crate::errors::Result;

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
