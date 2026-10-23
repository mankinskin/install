use gix_transport::client::blocking_io::http::{
    Options,
    options::{FollowRedirects, HttpVersion},
};
use std::{path::PathBuf, process::Command, time::Duration};

pub(super) fn source(source: &str) -> Result<String, String> {
    let output = Command::new("git")
        .args(["ls-remote", "--get-url", "--", source])
        .output()
        .map_err(|_| "selective transport: cannot resolve Git URL configuration")?;
    if !output.status.success() {
        return Err("selective transport: invalid Git URL configuration".into());
    }
    let source = String::from_utf8(output.stdout)
        .map_err(|_| "selective transport: configured source URL is not UTF-8")?;
    let source = source.trim_end_matches(['\r', '\n']);
    if source.is_empty() || source.contains(['\r', '\n']) {
        return Err("selective transport: configured source URL is invalid".into());
    }
    Ok(source.to_owned())
}

fn boolean(value: &str) -> Result<bool, String> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" | "" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        _ => Err("selective transport: invalid HTTP boolean in Git configuration".into()),
    }
}

fn extra_headers(source: &str) -> Result<Vec<String>, String> {
    use std::collections::BTreeSet;
    let output = Command::new("git")
        .args(["config", "--null", "--get-regexp", "^http\\..*extraheader$"])
        .output()
        .map_err(|_| "selective transport: cannot read Git HTTP headers")?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err("selective transport: Git HTTP headers could not be read".into());
    }
    let values = std::str::from_utf8(&output.stdout)
        .map_err(|_| "selective transport: Git HTTP headers are not UTF-8")?;
    let records: Vec<_> = values
        .split('\0')
        .filter(|record| !record.is_empty())
        .map(|record| record.split_once('\n').unwrap_or((record, "")))
        .collect();
    let keys: BTreeSet<_> = records.iter().map(|(key, _)| *key).collect();
    let mut matched = BTreeSet::new();
    let marker = format!("install-ctl-header-match-{}", std::process::id());
    // --get-urlmatch returns only the last value. Probe scope precedence with Git,
    // then retain all values in matching scopes, including explicit reset entries.
    for key in keys {
        let output = Command::new("git")
            .args([
                "-c",
                &format!("{key}={marker}"),
                "config",
                "--get-urlmatch",
                "http.extraheader",
                source,
            ])
            .output()
            .map_err(|_| "selective transport: cannot resolve Git HTTP header scope")?;
        if !output.status.success() && output.status.code() != Some(1) {
            return Err("selective transport: Git HTTP header scope could not be resolved".into());
        }
        if output.status.success() && output.stdout == format!("{marker}\n").as_bytes() {
            matched.insert(key);
        }
    }
    let mut headers = Vec::new();
    for (key, value) in records {
        if !matched.contains(key) {
            continue;
        }
        if value.is_empty() {
            headers.clear();
        } else if value.contains(['\r', '\n']) {
            return Err("selective transport: invalid newline in configured HTTP header".into());
        } else {
            headers.push(value.to_owned());
        }
    }
    Ok(headers)
}

pub(super) fn http(source: &str) -> Result<Options, String> {
    let mut options = Options {
        connect_timeout: Some(Duration::from_secs(30)),
        ..Options::default()
    };
    if !source.starts_with("http://") && !source.starts_with("https://") {
        return Ok(options);
    }
    let output = Command::new("git")
        .args(["config", "--null", "--get-urlmatch", "http", source])
        .output()
        .map_err(|_| "selective transport: cannot read Git HTTP configuration")?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err("selective transport: Git HTTP configuration could not be read".into());
    }
    let values = std::str::from_utf8(&output.stdout)
        .map_err(|_| "selective transport: Git HTTP configuration is not UTF-8")?;
    for record in values.split('\0').filter(|record| !record.is_empty()) {
        let (key, value) = record.split_once('\n').unwrap_or((record, ""));
        match key.to_ascii_lowercase().as_str() {
            "http.extraheader" => {}
            "http.proxy" => options.proxy = Some(value.to_owned()),
            "http.sslcainfo" => options.ssl_ca_info = Some(PathBuf::from(value)),
            "http.sslverify" => options.ssl_verify = boolean(value)?,
            "http.useragent" => options.user_agent = Some(value.to_owned()),
            "http.lowspeedlimit" => {
                options.low_speed_limit_bytes_per_second = value
                    .parse()
                    .map_err(|_| "selective transport: invalid http.lowSpeedLimit")?
            }
            "http.lowspeedtime" => {
                options.low_speed_time_seconds = value
                    .parse()
                    .map_err(|_| "selective transport: invalid http.lowSpeedTime")?
            }
            "http.followredirects" => {
                options.follow_redirects = match value {
                    "initial" => FollowRedirects::Initial,
                    _ if boolean(value)? => FollowRedirects::All,
                    _ => FollowRedirects::None,
                }
            }
            "http.version" => {
                options.http_version = Some(match value {
                    "HTTP/1.1" => HttpVersion::V1_1,
                    "HTTP/2" => HttpVersion::V2,
                    _ => return Err("selective transport: unsupported http.version".into()),
                })
            }
            "http.sslcert"
            | "http.sslkey"
            | "http.sslcapath"
            | "http.sslversion"
            | "http.pinnedpubkey"
            | "http.proxysslcert"
            | "http.proxysslkey"
            | "http.proxysslcainfo"
            | "http.proxysslverify" => {
                return Err(format!(
                    "selective transport: configured {key} is not supported by this HTTP backend"
                ));
            }
            _ => {}
        }
    }
    options.extra_headers = extra_headers(source)?;
    if let Some(path) = std::env::var_os("GIT_SSL_CAINFO") {
        options.ssl_ca_info = Some(path.into());
    }
    if std::env::var_os("GIT_SSL_NO_VERIFY").is_some() {
        options.ssl_verify = false;
    }
    for name in [
        "GIT_SSL_CERT",
        "GIT_SSL_KEY",
        "GIT_SSL_VERSION",
        "GIT_SSL_CAPATH",
    ] {
        if std::env::var_os(name).is_some() {
            return Err(format!(
                "selective transport: {name} is not supported by this HTTP backend"
            ));
        }
    }
    Ok(options)
}
