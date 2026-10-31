//! Foreground credential acquisition for the isolated transport worker.
//!
//! Git may prompt on the controlling terminal, which only the foreground
//! parent can read. The worker runs in its own process group under deadlines,
//! so it reports that authentication is required and the parent asks Git.

use super::{Credentials, Request, Response, config, process};
use std::{
    io::Write,
    process::{Command, Stdio},
};

pub(super) fn exchange(
    request: Request,
    credentials: &mut Option<Credentials>,
) -> Result<Response, String> {
    let source = &request.source;
    match process::run(&request, credentials.as_ref())? {
        Response::AuthenticationRequired => {
            if credentials.is_some() {
                return Err(
                    "selective transport: server requested authentication again after accepting credentials"
                        .into(),
                );
            }
            let source = config::source(source)?;
            let account = fill(&source)?;
            match process::run(&request, Some(&account))? {
                Response::AuthenticationRequired => Err(
                    "selective transport: server did not accept the supplied authentication".into(),
                ),
                Response::AuthenticationRejected(message) => {
                    Err(reject(&source, &account, message))
                }
                response => {
                    finish(&source, &account, "approve")?;
                    *credentials = Some(account);
                    Ok(response)
                }
            }
        }
        Response::AuthenticationRejected(message) => match credentials.take() {
            Some(account) => Err(reject(&config::source(source)?, &account, message)),
            None => Err(message),
        },
        response => Ok(response),
    }
}

fn reject(source: &str, account: &Credentials, mut message: String) -> String {
    if let Err(error) = finish(source, account, "reject") {
        message.push_str(&format!("; additionally, {error}"));
    }
    message
}

fn fill(source: &str) -> Result<Credentials, String> {
    if source.contains(['\r', '\n']) {
        return Err("selective transport: invalid source URL".into());
    }
    // Inherit stderr and the controlling terminal so Git's prompts and errors stay visible.
    let mut child = Command::new("git")
        .args(["credential", "fill"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|_| "selective transport: cannot start Git credential helper")?;
    write!(
        child
            .stdin
            .take()
            .ok_or("selective transport: missing credential input")?,
        "url={source}\n\n"
    )
    .map_err(|_| "selective transport: credential helper input failed")?;
    let output = child
        .wait_with_output()
        .map_err(|_| "selective transport: credential helper failed")?;
    if !output.status.success() {
        return Err(
            "selective transport: authentication required; configure a Git credential helper or enable terminal prompts"
                .into(),
        );
    }
    let text = std::str::from_utf8(&output.stdout)
        .map_err(|_| "selective transport: credential helper returned invalid text")?;
    let mut username = None;
    let mut password = None;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("username=") {
            username = Some(value.to_owned());
        }
        if let Some(value) = line.strip_prefix("password=") {
            password = Some(value.to_owned());
        }
    }
    Ok(Credentials {
        username: username
            .ok_or("selective transport: credential helper did not provide a username")?,
        password: password
            .ok_or("selective transport: credential helper did not provide a password")?,
    })
}

fn finish(source: &str, account: &Credentials, action: &str) -> Result<(), String> {
    let mut child = Command::new("git")
        .args(["credential", action])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|_| "selective transport: cannot update Git credential helper")?;
    write!(
        child
            .stdin
            .take()
            .ok_or("selective transport: missing credential helper input")?,
        "url={source}\nusername={}\npassword={}\n\n",
        account.username,
        account.password
    )
    .map_err(|_| "selective transport: cannot send credential helper outcome")?;
    let status = child
        .wait()
        .map_err(|_| "selective transport: credential helper did not terminate")?;
    if !status.success() {
        return Err(
            "selective transport: Git credential helper could not record authentication outcome"
                .into(),
        );
    }
    Ok(())
}
