//! An example resource, kept in memory until the process ends. Replace it with yours.

use crate::Problem;
use axum::{Json, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};
use warded_server::Params;

/// Every note by its number. Clones share the same notes.
#[derive(Clone, Default)]
pub struct Notes(Arc<Mutex<Stored>>);

#[derive(Default)]
struct Stored {
    last_id: u64,
    notes: BTreeMap<u64, Note>,
}

impl Notes {
    fn lock(&self) -> MutexGuard<'_, Stored> {
        // Nothing here leaves the notes half changed when it panics.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Clone, Serialize)]
pub struct Note {
    id: u64,
    text: String,
}

#[derive(Deserialize)]
pub struct NewNote {
    text: String,
}

/// The captures of `/notes/{id}`.
#[derive(Deserialize)]
pub struct NoteParams {
    id: u64,
}

const NO_SUCH_NOTE: Problem = Problem(StatusCode::NOT_FOUND, "there is no such note");

pub async fn list(State(notes): State<Notes>) -> Json<Vec<Note>> {
    Json(notes.lock().notes.values().cloned().collect())
}

pub async fn create(
    State(notes): State<Notes>,
    Json(new): Json<NewNote>,
) -> (StatusCode, Json<Note>) {
    let mut stored = notes.lock();
    stored.last_id += 1;
    let note = Note {
        id: stored.last_id,
        text: new.text,
    };
    stored.notes.insert(note.id, note.clone());
    (StatusCode::CREATED, Json(note))
}

pub async fn show(
    State(notes): State<Notes>,
    Params(params): Params<NoteParams>,
) -> Result<Json<Note>, Problem> {
    let stored = notes.lock();
    let note = stored.notes.get(&params.id).cloned();
    note.map(Json).ok_or(NO_SUCH_NOTE)
}

pub async fn remove(
    State(notes): State<Notes>,
    Params(params): Params<NoteParams>,
) -> Result<StatusCode, Problem> {
    let removed = notes.lock().notes.remove(&params.id);
    removed.map(|_| StatusCode::NO_CONTENT).ok_or(NO_SUCH_NOTE)
}
