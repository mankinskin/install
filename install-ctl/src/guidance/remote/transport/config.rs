use gix_transport::client::blocking_io::http::{
    Options,
    options::{FollowRedirects, HttpVersion},
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

pub(super) fn initialize_tls(
    source: &str,
    options: &mut Options,
    directory: &Path,
) -> Result<(), String> {
    initialize_tls_with(
        source,
        options,
        directory,
        rustls_native_certs::load_native_certs,
    )
}

fn initialize_tls_with(
    source: &str,
    options: &mut Options,
    directory: &Path,
    load: impl FnOnce() -> rustls_native_certs::CertificateResult,
) -> Result<(), String> {
    if !source.starts_with("https://") || !options.ssl_verify || options.ssl_ca_info.is_some() {
        return Ok(());
    }
    let roots = load();
    if !roots.errors.is_empty() {
        return Err(format!(
            "selective transport: cannot load system TLS trust store: {}",
            roots
                .errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    if roots.certs.is_empty() {
        return Err("selective transport: system TLS trust store contains no certificates; configure GIT_SSL_CAINFO or http.sslCAInfo".into());
    }
    use base64::Engine;
    let mut pem = String::new();
    for certificate in roots.certs {
        pem.push_str("-----BEGIN CERTIFICATE-----\n");
        let encoded = base64::engine::general_purpose::STANDARD.encode(certificate.as_ref());
        for line in encoded.as_bytes().chunks(64) {
            // Base64 output is ASCII.
            pem.push_str(std::str::from_utf8(line).map_err(|error| {
                format!("selective transport: cannot encode system TLS certificate: {error}")
            })?);
            pem.push('\n');
        }
        pem.push_str("-----END CERTIFICATE-----\n");
    }
    let path = directory.join("system-ca.pem");
    fs::write(&path, pem).map_err(|error| {
        format!("selective transport: cannot write system TLS CA bundle: {error}")
    })?;
    options.ssl_ca_info = Some(path);
    Ok(())
}

pub(crate) fn repository_key(source: &str) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut url = gix_url::parse(source)
        .map_err(|_| "selective transport: cannot identify repository URL")?;
    url.set_user(None);
    url.set_password(None);
    if matches!(url.scheme, gix_url::Scheme::Http | gix_url::Scheme::Https) {
        url.path = url.path_query_fragment().path.to_owned();
        while url.path.len() > 1 && url.path.last() == Some(&b'/') {
            url.path.pop();
        }
    }
    url.canonicalize(
        &std::env::current_dir()
            .map_err(|_| "selective transport: cannot resolve source directory")?,
    )
    .map_err(|_| "selective transport: cannot canonicalize repository identity")?;
    if matches!(
        (&url.scheme, url.port),
        (gix_url::Scheme::Http, Some(80))
            | (gix_url::Scheme::Https, Some(443))
            | (gix_url::Scheme::Ssh, Some(22))
            | (gix_url::Scheme::Git, Some(9418))
    ) {
        url.port = None;
    }
    url.serialize_alternative_form = false;
    Ok(format!("{:x}", Sha256::digest(&url.to_bstring()[..])))
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_tls_roots_are_encoded_and_configured_for_https() {
        let directory = tempfile::tempdir().unwrap();
        let mut options = Options::default();
        initialize_tls_with(
            "https://example.invalid/repo",
            &mut options,
            directory.path(),
            || {
                let mut roots = rustls_native_certs::CertificateResult::default();
                roots.certs.push(vec![0; 96].into());
                roots.certs.push(vec![1, 2, 3].into());
                roots
            },
        )
        .unwrap();
        let path = options.ssl_ca_info.unwrap();
        let pem = fs::read_to_string(&path).unwrap();
        assert_eq!(path, directory.path().join("system-ca.pem"));
        assert_eq!(pem.matches("-----BEGIN CERTIFICATE-----").count(), 2);
        assert!(pem.contains("\nAQID\n-----END CERTIFICATE-----\n"));
        assert!(
            pem.lines()
                .filter(|line| !line.starts_with("-----"))
                .all(|line| line.len() <= 64)
        );
        assert!(options.ssl_verify);
        directory.close().unwrap();
        assert!(
            !path.exists(),
            "invocation cleanup must remove the CA bundle"
        );
    }

    #[test]
    fn explicit_ca_bundle_skips_system_tls_discovery() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("custom.pem");
        let mut options = Options {
            ssl_ca_info: Some(path.clone()),
            ..Options::default()
        };
        initialize_tls_with(
            "https://example.invalid/repo",
            &mut options,
            directory.path(),
            || panic!("explicit CA override must not load system roots"),
        )
        .unwrap();
        assert_eq!(options.ssl_ca_info, Some(path));
        assert!(!directory.path().join("system-ca.pem").exists());
    }

    #[test]
    fn non_https_and_disabled_verification_skip_system_tls_discovery() {
        let directory = tempfile::tempdir().unwrap();
        for (source, verify) in [
            ("http://example.invalid/repo", true),
            ("ssh://example.invalid/repo", true),
            ("https://example.invalid/repo", false),
        ] {
            let mut options = Options {
                ssl_verify: verify,
                ..Options::default()
            };
            initialize_tls_with(source, &mut options, directory.path(), || {
                panic!("system roots are not needed for this transport")
            })
            .unwrap();
            assert!(options.ssl_ca_info.is_none());
            assert_eq!(options.ssl_verify, verify);
        }
    }

    #[test]
    fn empty_system_tls_store_fails_explicitly() {
        let directory = tempfile::tempdir().unwrap();
        let mut options = Options::default();
        let error = initialize_tls_with(
            "https://example.invalid/repo",
            &mut options,
            directory.path(),
            rustls_native_certs::CertificateResult::default,
        )
        .unwrap_err();
        assert!(error.contains("contains no certificates"), "{error}");
        assert!(options.ssl_ca_info.is_none());
    }

    #[test]
    fn system_tls_discovery_errors_are_not_silently_ignored() {
        let directory = tempfile::tempdir().unwrap();
        let mut options = Options::default();
        let error = initialize_tls_with(
            "https://example.invalid/repo",
            &mut options,
            directory.path(),
            || {
                let mut roots = rustls_native_certs::CertificateResult::default();
                roots.certs.push(vec![1, 2, 3].into());
                roots.errors.push(rustls_native_certs::Error {
                    context: "reading system roots",
                    kind: rustls_native_certs::ErrorKind::Io {
                        inner: std::io::Error::other("fixture read failure"),
                        path: PathBuf::from("fixture.pem"),
                    },
                });
                roots
            },
        )
        .unwrap_err();
        assert!(error.contains("fixture read failure"), "{error}");
        assert!(error.contains("fixture.pem"), "{error}");
        assert!(options.ssl_ca_info.is_none());
    }

    #[test]
    fn system_tls_bundle_write_errors_are_visible() {
        let directory = tempfile::tempdir().unwrap();
        let mut options = Options::default();
        let error = initialize_tls_with(
            "https://example.invalid/repo",
            &mut options,
            &directory.path().join("missing"),
            || {
                let mut roots = rustls_native_certs::CertificateResult::default();
                roots.certs.push(vec![1, 2, 3].into());
                roots
            },
        )
        .unwrap_err();
        assert!(
            error.contains("cannot write system TLS CA bundle"),
            "{error}"
        );
        assert!(options.ssl_ca_info.is_none());
    }
}
