use super::{Operation, Request, Response, config, objects};
use gix_protocol::fetch::{Arguments, Response as FetchResponse};
use gix_transport::{
    Protocol, Service,
    client::{
        self,
        blocking_io::{Transport, TransportV2Ext},
    },
};
use std::{
    error::Error,
    fs,
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
};

fn diagnostic(
    context: &str,
    error: &dyn Error,
    source: &str,
    account: Option<&client::Account>,
) -> String {
    let mut message = format!("{context}: {error}");
    let mut cause = error.source();
    while let Some(error) = cause {
        message.push_str(&format!(": {error}"));
        cause = error.source();
    }
    if let Ok(mut url) = gix_url::parse(source) {
        let password = url.password().map(str::to_owned);
        url.set_user(None);
        url.set_password(None);
        message = message.replace(source, &url.to_bstring().to_string());
        if let Some(password) = password.filter(|value| !value.is_empty()) {
            message = message.replace(&password, "[REDACTED]");
        }
    } else {
        message = message.replace(source, "[REDACTED SOURCE]");
    }
    if let Some(account) = account {
        for secret in [
            Some(account.password.as_str()),
            account.oauth_refresh_token.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter(|secret| !secret.is_empty())
        {
            message = message.replace(secret, "[REDACTED]");
        }
    }
    message
}

fn credentials(source: &str) -> Result<client::Account, String> {
    if source.contains(['\r', '\n']) {
        return Err("selective transport: invalid source URL".into());
    }
    let mut child = Command::new("git")
        .args(["credential", "fill"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
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
            "selective transport: authentication required; configure a Git credential helper"
                .into(),
        );
    }
    let mut username = None;
    let mut password = None;
    let text = std::str::from_utf8(&output.stdout)
        .map_err(|_| "selective transport: credential helper returned invalid text")?;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("username=") {
            username = Some(value.to_owned());
        }
        if let Some(value) = line.strip_prefix("password=") {
            password = Some(value.to_owned());
        }
    }
    Ok(client::Account {
        username: username
            .ok_or("selective transport: credential helper did not provide a username")?,
        password: password
            .ok_or("selective transport: credential helper did not provide a password")?,
        oauth_refresh_token: None,
    })
}

fn finish_credentials(source: &str, account: &client::Account, action: &str) -> Result<(), String> {
    let mut child = Command::new("git")
        .args(["credential", action])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
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

fn ssh_options() -> Result<client::blocking_io::ssh::connect::Options, String> {
    let command = match std::env::var_os("GIT_SSH_COMMAND") {
        Some(value) => Some(value),
        None => {
            let output = Command::new("git")
                .args(["config", "--get", "core.sshCommand"])
                .output()
                .map_err(|_| "selective transport: cannot read Git SSH configuration")?;
            match output.status.code() {
                Some(0) => Some(
                    String::from_utf8(output.stdout)
                        .map_err(|_| "selective transport: invalid SSH command configuration")?
                        .trim()
                        .into(),
                ),
                Some(1) => std::env::var_os("GIT_SSH"),
                _ => {
                    return Err(
                        "selective transport: Git SSH configuration could not be read".into(),
                    );
                }
            }
        }
    };
    let variant = match std::env::var("GIT_SSH_VARIANT") {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => {
            let output = Command::new("git")
                .args(["config", "--get", "ssh.variant"])
                .output()
                .map_err(|_| "selective transport: cannot read SSH variant")?;
            match output.status.code() {
                Some(0) => Some(
                    String::from_utf8(output.stdout)
                        .map_err(|_| "selective transport: invalid SSH variant configuration")?
                        .trim()
                        .to_owned(),
                ),
                Some(1) => None,
                _ => return Err("selective transport: SSH variant could not be read".into()),
            }
        }
        Err(_) => return Err("selective transport: invalid GIT_SSH_VARIANT".into()),
    };
    let kind = match variant.as_deref() {
        None | Some("auto") => None,
        Some("ssh") => Some(client::blocking_io::ssh::ProgramKind::Ssh),
        Some("plink") => Some(client::blocking_io::ssh::ProgramKind::Plink),
        Some("putty") => Some(client::blocking_io::ssh::ProgramKind::Putty),
        Some("tortoiseplink") => Some(client::blocking_io::ssh::ProgramKind::TortoisePlink),
        Some("simple") => Some(client::blocking_io::ssh::ProgramKind::Simple),
        Some(_) => return Err("selective transport: unsupported GIT_SSH_VARIANT".into()),
    };
    Ok(client::blocking_io::ssh::connect::Options {
        command,
        kind,
        disallow_shell: false,
    })
}

pub(super) fn run(request: Request, directory: &Path) -> Result<Response, String> {
    let source = config::source(&request.source)?;
    let mut transport = client::blocking_io::connect::connect(
        source.as_str(),
        client::blocking_io::connect::Options {
            version: Protocol::V2,
            ssh: ssh_options()?,
            trace: false,
        },
    )
    .map_err(|error| diagnostic("selective transport: cannot connect", &error, &source, None))?;
    let mut http_options = config::http(&source)?;
    config::initialize_tls(&source, &mut http_options, directory)?;
    transport
        .configure(&http_options)
        .map_err(|error| {
            diagnostic(
                "selective transport: HTTP configuration failed",
                &error,
                &source,
                None,
            )
        })?;

    let handshake = match transport.handshake(Service::UploadPack, &[]) {
        Ok(response) => {
            if response.actual_protocol != Protocol::V2 {
                return Err("selective transport requires protocol v2 with fetch filtering".into());
            }
            Ok(response.capabilities)
        }
        Err(client::Error::Io(ref error))
            if error.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            Err(())
        }
        Err(error) => {
            return Err(diagnostic(
                "selective transport handshake failed; verify repository access and server availability",
                &error,
                &source,
                None,
            ));
        }
    };
    let capabilities = match handshake {
        Ok(capabilities) => capabilities,
        Err(()) => {
            let account = credentials(&source)?;
            transport.set_identity(account.clone()).map_err(|error| {
                diagnostic(
                    "selective transport: server does not support configured authentication",
                    &error,
                    &source,
                    Some(&account),
                )
            })?;
            let response = match transport.handshake(Service::UploadPack, &[]) {
                Ok(response) => {
                    finish_credentials(&source, &account, "approve")?;
                    response
                }
                Err(error) => {
                    let mut message = diagnostic(
                        "selective transport: authentication failed; check the Git credential helper",
                        &error,
                        &source,
                        Some(&account),
                    );
                    if matches!(error, client::Error::Io(ref error) if error.kind() == std::io::ErrorKind::PermissionDenied)
                        && let Err(credential_error) =
                            finish_credentials(&source, &account, "reject")
                    {
                        message.push_str(&format!("; additionally, {credential_error}"));
                    }
                    return Err(message);
                }
            };
            if response.actual_protocol != Protocol::V2 {
                return Err("selective transport requires protocol v2 with fetch filtering".into());
            }
            response.capabilities
        }
    };
    if capabilities
        .capability("fetch")
        .and_then(|cap| cap.supports("filter"))
        != Some(true)
    {
        return Err("selective transport unsupported: server must enable upload-pack filtering (uploadpack.allowFilter)".into());
    }
    fs::write(directory.join("connected"), [])
        .map_err(|_| "selective transport: cannot report completed handshake")?;

    let mut features = gix_protocol::Command::Fetch.default_features(Protocol::V2, &capabilities);
    features.retain(|(name, _)| !matches!(*name, "sideband-all" | "packfile-uris"));
    let format = capabilities
        .capability("object-format")
        .and_then(|cap| cap.value())
        .map(|value| value.to_string())
        .unwrap_or_else(|| "sha1".to_owned());
    if !matches!(format.as_str(), "sha1" | "sha256") {
        return Err("selective transport: unsupported Git object format".into());
    }
    match request.operation {
        Operation::Authorize => Ok(Response::Authorized {
            repository_key: config::repository_key(&source)?,
        }),
        Operation::Head => {
            let mut refs = transport
                .invoke(
                    "ls-refs",
                    gix_protocol::Command::LsRefs
                        .default_features(Protocol::V2, &capabilities)
                        .into_iter(),
                    Some(["symrefs".into(), "ref-prefix HEAD".into()].into_iter()),
                    false,
                )
                .map_err(|_| "selective transport: cannot request remote HEAD")?;
            let refs = gix_protocol::handshake::refs::from_v2_refs(&mut refs)
                .map_err(|_| "selective transport: invalid remote HEAD advertisement")?;
            let oid = refs
                .iter()
                .find_map(|reference| {
                    let (name, oid, _) = reference.unpack();
                    (name == "HEAD").then_some(oid).flatten()
                })
                .ok_or("selective transport: repository HEAD is missing or unborn")?;
            Ok(Response::Head {
                oid: oid.to_string(),
                repository_key: config::repository_key(&source)?,
            })
        }
        operation => {
            let (oid, metadata) = match operation {
                Operation::Metadata(oid) => (oid, true),
                Operation::Blob(oid) => (oid, false),
                Operation::Head | Operation::Authorize => {
                    return Err("selective transport: unexpected operation".into());
                }
            };
            let wanted = gix_hash::ObjectId::from_hex(oid.as_bytes())
                .map_err(|_| "selective transport: invalid wanted object ID")?;
            if wanted.kind().to_string() != format {
                return Err(
                    "selective transport: wanted object ID has the wrong hash format".into(),
                );
            }
            let shallow = features.iter().any(|(name, _)| *name == "shallow");
            let mut arguments = Arguments::new(Protocol::V2, features, false);
            arguments.want(wanted);
            arguments.filter("blob:none");
            if metadata && shallow {
                arguments.deepen(1);
            }
            let mut reader = arguments
                .send(&mut transport, true)
                .map_err(|_| "selective transport: server rejected object request")?;
            let response = FetchResponse::from_line_reader(Protocol::V2, &mut reader, true, false)
                .map_err(|_| "selective transport: server rejected selective object want; enable reachable object wants")?;
            if !response.has_pack() {
                return Err("selective transport: server returned no object pack".into());
            }
            reader.set_progress_handler(Some(Box::new(|_, _| std::ops::ControlFlow::Continue(()))));
            let mut pack = Vec::new();
            reader
                .read_to_end(&mut pack)
                .map_err(|_| "selective transport: interrupted object pack")?;
            drop(reader);
            let objects = objects::decode(&pack, &format)?;
            if metadata {
                objects::validate_metadata(&objects, &oid)?;
            } else if objects.len() != 1 || objects[0].oid != oid || objects[0].kind != "blob" {
                return Err("selective transport: unexpected object in selected-blob pack".into());
            }
            Ok(Response::Objects(objects))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_preserves_tls_cause_hidden_by_transport_display() {
        let cause = "failed to build client config: no server certificate verifier was configured on the client config builder";
        let error = client::Error::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            cause,
        ));
        let message = diagnostic(
            "selective transport handshake failed",
            &error,
            "https://example.invalid/repo",
            None,
        );
        assert!(message.contains(cause), "{message}");
    }

    #[test]
    fn diagnostic_redacts_url_and_credential_helper_secrets() {
        let source = "https://user:url-secret@example.invalid/repo";
        let error = std::io::Error::other(format!(
            "failed for {source}: url-secret helper-secret refresh-secret"
        ));
        let account = client::Account {
            username: "user".into(),
            password: "helper-secret".into(),
            oauth_refresh_token: Some("refresh-secret".into()),
        };
        let message = diagnostic(
            "selective transport handshake failed",
            &error,
            source,
            Some(&account),
        );
        assert!(
            message.contains("https://example.invalid/repo"),
            "{message}"
        );
        for secret in ["url-secret", "helper-secret", "refresh-secret"] {
            assert!(!message.contains(secret), "{message}");
        }
    }

    #[test]
    fn diagnostic_redacts_source_when_url_parsing_fails() {
        let source = "https://user:url-secret@[invalid/repo";
        let error = std::io::Error::other(format!("invalid URL: {source}"));
        let message = diagnostic("selective transport: cannot connect", &error, source, None);
        assert_eq!(
            message,
            "selective transport: cannot connect: invalid URL: [REDACTED SOURCE]"
        );
    }
}
