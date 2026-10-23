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
    fs,
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
};

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
    .map_err(
        |_| "selective transport: cannot connect; verify source URL and transport configuration",
    )?;
    transport
        .configure(&config::http(&source)?)
        .map_err(|_| "selective transport: HTTP configuration failed")?;

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
            Err(true)
        }
        Err(_) => Err(false),
    };
    let capabilities = match handshake {
        Ok(capabilities) => capabilities,
        Err(needs_authentication) => {
            if !needs_authentication {
                return Err("selective transport handshake failed; verify repository access and server availability".into());
            }
            let account = credentials(&source)?;
            transport.set_identity(account.clone()).map_err(
                |_| "selective transport: server does not support configured authentication",
            )?;
            let response = match transport.handshake(Service::UploadPack, &[]) {
                Ok(response) => {
                    finish_credentials(&source, &account, "approve")?;
                    response
                }
                Err(error) => {
                    if matches!(error, client::Error::Io(ref error) if error.kind() == std::io::ErrorKind::PermissionDenied)
                    {
                        finish_credentials(&source, &account, "reject")?;
                    }
                    return Err("selective transport: authentication failed; check the Git credential helper".into());
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
            Ok(Response::Head(oid.to_string()))
        }
        operation => {
            let (oid, metadata) = match operation {
                Operation::Metadata(oid) => (oid, true),
                Operation::Blob(oid) => (oid, false),
                Operation::Head => return Err("selective transport: unexpected operation".into()),
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
