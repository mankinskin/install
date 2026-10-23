//! Selective Git object transfer, isolated so deadlines also cancel transport children.

mod config;
mod exchange;
mod objects;
mod process;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceivedObject {
    pub oid: String,
    pub kind: String,
    pub data: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct Request {
    source: String,
    operation: Operation,
}

#[derive(Serialize, Deserialize)]
pub(super) enum Operation {
    Head,
    Metadata(String),
    Blob(String),
}

#[derive(Serialize, Deserialize)]
pub(super) enum Response {
    Head(String),
    Objects(Vec<ReceivedObject>),
}

pub struct SelectiveTransport {
    source: String,
    head: String,
}

impl SelectiveTransport {
    pub fn connect(source: &str) -> Result<Self, String> {
        match process::run(Request {
            source: source.to_owned(),
            operation: Operation::Head,
        })? {
            Response::Head(head) => Ok(Self {
                source: source.to_owned(),
                head,
            }),
            Response::Objects(_) => Err("selective transport: invalid HEAD response".into()),
        }
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
            Response::Head(_) => Err("selective transport: invalid object response".into()),
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
