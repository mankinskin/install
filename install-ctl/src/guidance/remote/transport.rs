//! Selective Git object transfer, isolated so deadlines also cancel transport children.

pub(super) mod config;
mod exchange;
mod objects;
mod process;
#[cfg(test)]
pub(super) mod tests;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceivedObject {
    pub oid: String,
    pub kind: String,
    pub data: Vec<u8>,
}

impl ReceivedObject {
    pub(super) fn verify(&self) -> Result<(), String> {
        objects::verify(self)
    }
}

pub(super) fn validate_metadata(objects: &[ReceivedObject], root: &str) -> Result<(), String> {
    objects::validate_metadata(objects, root)
}

#[derive(Serialize, Deserialize)]
pub(super) struct Request {
    source: String,
    operation: Operation,
}

#[derive(Serialize, Deserialize)]
pub(super) enum Operation {
    Head,
    Authorize,
    Metadata(String),
    Blob(String),
}

#[derive(Serialize, Deserialize)]
pub(super) enum Response {
    Head { oid: String, repository_key: String },
    Authorized { repository_key: String },
    Objects(Vec<ReceivedObject>),
}

pub struct SelectiveTransport {
    source: String,
    head: String,
    repository_key: String,
}

impl SelectiveTransport {
    pub fn connect(source: &str) -> Result<Self, String> {
        match process::run(Request {
            source: source.to_owned(),
            operation: Operation::Head,
        })? {
            Response::Head {
                oid: head,
                repository_key,
            } => Ok(Self {
                source: source.to_owned(),
                head,
                repository_key,
            }),
            _ => Err("selective transport: invalid HEAD response".into()),
        }
    }

    pub fn pinned(source: &str, revision: &str) -> Result<Self, String> {
        let revision = gix_hash::ObjectId::from_hex(revision.as_bytes())
            .map_err(|_| "selective transport: invalid pinned revision")?
            .to_string();
        match process::run(Request {
            source: source.to_owned(),
            operation: Operation::Authorize,
        })? {
            Response::Authorized { repository_key } => Ok(Self {
                source: source.to_owned(),
                head: revision,
                repository_key,
            }),
            _ => Err("selective transport: invalid authorization response".into()),
        }
    }

    pub fn repository_key(&self) -> &str {
        &self.repository_key
    }

    pub fn head_oid(&self) -> &str {
        &self.head
    }

    pub fn fetch_metadata(&mut self, oid: &str) -> Result<Vec<ReceivedObject>, String> {
        self.fetch(Operation::Metadata(oid.to_owned()))
    }

    pub fn fetch_blob(&mut self, oid: &str) -> Result<ReceivedObject, String> {
        let mut objects = self.fetch(Operation::Blob(oid.to_owned()))?;
        if objects.len() != 1 {
            return Err("selective transport: expected exactly one selected blob".into());
        }
        objects
            .pop()
            .ok_or_else(|| "selective transport: missing selected blob".into())
    }

    fn fetch(&self, operation: Operation) -> Result<Vec<ReceivedObject>, String> {
        match process::run(Request {
            source: self.source.clone(),
            operation,
        })? {
            Response::Objects(objects) => Ok(objects),
            _ => Err("selective transport: invalid object response".into()),
        }
    }
}

pub fn run_worker_if_requested() -> bool {
    if std::env::args().nth(1).as_deref() != Some("--__guidance-object-worker") {
        return false;
    }
    process::worker_entry();
    true
}
