use std::{
    collections::HashMap,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

pub(crate) fn git_http_backend(
    root: &Path,
    method: &str,
    path: &str,
    headers: &HashMap<String, String>,
    body: &[u8],
) -> Result<(u16, Vec<(String, String)>, Vec<u8>), String> {
    let (path_info, query) = path.split_once('?').unwrap_or((path, ""));
    let mut command = Command::new("git");
    command
        .arg("http-backend")
        .env("GIT_PROJECT_ROOT", root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("REQUEST_METHOD", method)
        .env("PATH_INFO", path_info)
        .env("QUERY_STRING", query)
        .env(
            "CONTENT_TYPE",
            headers.get("content-type").map_or("", String::as_str),
        )
        .env("CONTENT_LENGTH", body.len().to_string())
        .env("SERVER_PROTOCOL", "HTTP/1.1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    if let Some(protocol) = headers.get("git-protocol") {
        command.env("HTTP_GIT_PROTOCOL", protocol);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("start git http-backend: {error}"))?;
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(body)
        .map_err(|error| error.to_string())?;
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    let boundary = output
        .stdout
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "git http-backend did not produce a CGI response".to_owned())?;
    let (header_bytes, payload) = output.stdout.split_at(boundary);
    let payload = &payload[4..];
    let mut status = 200;
    let mut result = Vec::new();
    for line in header_bytes.split(|byte| *byte == b'\n') {
        let line = std::str::from_utf8(line)
            .map_err(|error| error.to_string())?
            .trim_end_matches('\r');
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| format!("invalid CGI header: {line}"))?;
        if name.eq_ignore_ascii_case("status") {
            status = value
                .trim()
                .split_whitespace()
                .next()
                .ok_or("empty CGI status")?
                .parse()
                .map_err(|error| format!("invalid CGI status: {error}"))?;
        } else {
            result.push((name.to_owned(), value.trim().to_owned()));
        }
    }
    Ok((status, result, payload.to_vec()))
}
