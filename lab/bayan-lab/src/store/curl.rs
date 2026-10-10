//! HTTP requests through the `curl` program, shared by the S3 and HTTPS stores.
//!
//! Every supported platform has curl (Windows 10 and later include it), so the lab needs no HTTP or TLS library of its own, as xtask's crates.io requests do. Each request runs one curl process that reads no personal configuration file (`--disable`), allows only HTTPS (or plain HTTP to this machine, for tests and local servers), never follows a redirect, expands no URL patterns (`--globoff`), and has time limits: ten minutes in all, 30 seconds to connect, and a minute at most below 1 KiB per second. The response body comes back through a pipe, and this module stops reading, and stops curl, as soon as the body exceeds the request's limit, whatever curl's version; no document is ever written to a temporary file. An upload is read by curl from the file the document was read from. Failures that may pass are retried up to three times, each time from the start, so a failed response can never mix with a later one: no connection, a timeout, a failed TLS handshake, a broken transfer (curl's exit codes 5, 6, 7, 18, 28, 35, 52, 55, 56 and 92), or HTTP status 408, 429 or 5xx. A response larger than its limit is not retried; it would be as large the next time.
//!
//! The module needs curl 7.63 or later, and signed S3 requests need 7.87 or later, which signs an upload's SHA-256 (`x-amz-content-sha256`); it checks the version once, before its first request.
//!
//! **Credentials** come only from the environment variables `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` and, for temporary credentials, `AWS_SESSION_TOKEN`, and only the S3 store uses them. curl signs requests with AWS Signature Version 4 itself (`--aws-sigv4`): the tool implements no cryptography. The credentials are handed to curl on its standard input (`--config -`), never on its command line, where other users of the machine could see them, and they never appear in messages.

use std::ffi::OsString;
use std::io::{Read, Write as _};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use super::StoreError;
use crate::hash::Sha256;

/// How long one request may take, in seconds.
const MAX_SECONDS: &str = "600";
/// How long connecting may take, in seconds.
const CONNECT_SECONDS: &str = "30";
/// The slowest transfer accepted, in bytes per second, for longer than [`SLOW_SECONDS`]: a stalled server ends the attempt, which is then tried again.
const MIN_SPEED: &str = "1024";
/// How long a transfer may stay slower than [`MIN_SPEED`], in seconds.
const SLOW_SECONDS: &str = "60";
/// How often a request that failed transiently is tried again.
const RETRIES: u32 = 3;
/// The most of curl's standard error kept for messages, in bytes.
const MAX_ERROR_OUTPUT: u64 = 64 * 1024;
/// Starts the line on curl's standard error that carries the HTTP status (`--write-out`).
const STATUS_MARKER: &str = "bayan-lab-http-status:";

/// An HTTP method the stores use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Read an object or a listing.
    Get,
    /// Ask whether an object exists.
    Head,
    /// Write an object.
    Put,
}

impl Method {
    /// The method's name, for messages.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Put => "PUT",
        }
    }
}

/// S3 credentials from the environment.
pub struct Credentials {
    access_key: String,
    secret_key: String,
    session_token: Option<String>,
}

/// Reads the S3 credentials from the environment: none, or a complete pair.
///
/// # Errors
///
/// When only one of the pair is set, or a value holds characters it cannot hold.
pub fn credentials() -> Result<Option<Credentials>, StoreError> {
    let read = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let access_key = read("AWS_ACCESS_KEY_ID");
    let secret_key = read("AWS_SECRET_ACCESS_KEY");
    let session_token = read("AWS_SESSION_TOKEN");
    let credentials = match (access_key, secret_key) {
        (None, None) => return Ok(None),
        (Some(access_key), Some(secret_key)) => Credentials {
            access_key,
            secret_key,
            session_token,
        },
        _ => {
            return Err(StoreError(
                "set both AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY, or neither".to_owned(),
            ));
        }
    };
    let has_control = |value: &str| value.chars().any(char::is_control);
    // curl splits `user` at its first colon, so the access key cannot contain one; the secret can.
    if credentials.access_key.contains(':')
        || has_control(&credentials.access_key)
        || has_control(&credentials.secret_key)
        || credentials
            .session_token
            .as_deref()
            .is_some_and(has_control)
    {
        return Err(StoreError(
            "the S3 credentials contain characters they cannot contain".to_owned(),
        ));
    }
    Ok(Some(credentials))
}

/// A value for a double-quoted string of curl's configuration file.
fn quote(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Checks an endpoint such as `https://s3.example.com`, `https://[2001:db8::1]:8443` or `http://127.0.0.1:9000`: a scheme and a host (a name, or an IPv6 address in brackets) with an optional port, nothing else. Returns it without a trailing `/`, and whether it is plain HTTP to this machine.
///
/// # Errors
///
/// What is wrong with it, for a message.
pub fn endpoint(text: &str) -> Result<(String, bool), &'static str> {
    let text = text.trim_end_matches('/');
    let (scheme, host) = text
        .split_once("://")
        .ok_or("the address must start with https://")?;
    let (hostname, port) = match host.strip_prefix('[') {
        Some(rest) => {
            let (address, after) = rest
                .split_once(']')
                .ok_or("an IPv6 address must end with `]`")?;
            if address.is_empty()
                || !address
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() || matches!(byte, b':' | b'.'))
            {
                return Err("the brackets must hold an IPv6 address");
            }
            (&host[..address.len() + 2], after.strip_prefix(':'))
        }
        None => match host.split_once(':') {
            Some((name, port)) => (name, Some(port)),
            None => (host, None),
        },
    };
    let name_ok = hostname.starts_with('[')
        || (!hostname.is_empty()
            && hostname
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')));
    let port_ok = port.is_none_or(|port| {
        !port.is_empty() && port.len() <= 5 && port.bytes().all(|byte| byte.is_ascii_digit())
    });
    let rest_ok = host.len() == hostname.len() + port.map_or(0, |port| port.len() + 1);
    if !name_ok || !port_ok || !rest_ok {
        return Err(
            "the host must be a host name with an optional port, without user names or paths",
        );
    }
    let local = matches!(hostname, "127.0.0.1" | "localhost" | "[::1]");
    match scheme {
        "https" => Ok((text.to_owned(), false)),
        "http" if local => Ok((text.to_owned(), true)),
        _ => Err("the address must use https:// (plain http:// only to this machine)"),
    }
}

/// Whether `path` is a path prefix the stores accept: `/`-separated segments of letters, digits, dots, underscores and hyphens, without `.` or `..` segments. The empty path is accepted.
pub fn is_simple_path(path: &str) -> bool {
    path.is_empty()
        || path.split('/').all(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
}

/// One request.
pub struct Request<'a> {
    /// The method.
    pub method: Method,
    /// The full URL.
    pub url: &'a str,
    /// The URL as messages show it, without parts that need not be shown.
    pub shown: &'a str,
    /// For a PUT: the file to send, and its content type.
    pub upload: Option<(&'a Path, &'a str)>,
    /// For a PUT: the SHA-256 the sent bytes must have, sent as `x-amz-content-sha256`. A signed request carries it in its signature (curl 7.87 or later), so the server refuses the upload if the file changed after its SHA-256 was computed.
    pub payload_sha256: Option<Sha256>,
    /// The largest response body accepted, in bytes.
    pub max_size: u64,
    /// Plain HTTP to this machine instead of HTTPS.
    pub local_http: bool,
    /// Sign the request with AWS Signature Version 4: the credentials and the region.
    pub signing: Option<(&'a Credentials, &'a str)>,
}

/// A response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// The HTTP status.
    pub status: u16,
    /// The body (for a HEAD request, the headers).
    pub body: Vec<u8>,
}

/// How one attempt ended.
enum Attempt {
    /// A response arrived.
    Done(Response),
    /// The request failed in a way that may pass, such as a refused connection or a busy server.
    Transient(StoreError),
}

/// What curl's exit code and the HTTP status say about one attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// A response arrived, with an HTTP status that is not a transient failure.
    Response,
    /// curl stopped because the server announced a body larger than `--max-filesize` (exit 63). Trying again would fetch the same.
    TooLarge,
    /// curl failed in a way that may pass: no connection (exit 5, 6, 7), a timeout (28), a failed TLS handshake (35), or a broken transfer (18, 52, 55, 56, 92).
    TransientFailure,
    /// The server answered 408, 429 or 5xx: busy, or failing for a moment.
    TransientStatus,
    /// curl failed in a way that will not pass, such as a refused certificate, a malformed address or an unreadable upload.
    Failed,
}

/// Classifies one attempt by curl's exit code and the HTTP status (0 if none arrived).
fn classify(exit: Option<i32>, status: u16) -> Class {
    match exit {
        Some(0) if status == 0 => Class::Failed,
        Some(0) if matches!(status, 408 | 429) || (500..600).contains(&status) => {
            Class::TransientStatus
        }
        Some(0) => Class::Response,
        Some(63) => Class::TooLarge,
        Some(5 | 6 | 7 | 18 | 28 | 35 | 52 | 55 | 56 | 92) => Class::TransientFailure,
        _ => Class::Failed,
    }
}

/// Runs a request, trying again after transient failures, and returns the response.
///
/// # Errors
///
/// When curl cannot run or is too old, the response is larger than the request's limit, or the request still fails after the retries.
pub fn run(request: &Request<'_>) -> Result<Response, StoreError> {
    check_version(request.signing.is_some())?;
    let mut delay = Duration::from_secs(1);
    for attempt in 0..=RETRIES {
        match run_once(request)? {
            Attempt::Done(response) => return Ok(response),
            Attempt::Transient(error) if attempt == RETRIES => return Err(error),
            Attempt::Transient(_) => {
                thread::sleep(delay);
                delay = delay.saturating_mul(2);
            }
        }
    }
    Err(StoreError(format!(
        "{} {}: the request failed",
        request.method.name(),
        request.shown
    )))
}

/// The arguments curl runs with for `request`, the credentials excepted: they reach curl on its standard input ([`credentials_config`]), never on its command line, where other users of the machine could read them. Kept apart from running curl, so that tests can check them.
///
/// # Errors
///
/// When the file to upload cannot be found.
fn arguments(request: &Request<'_>) -> Result<Vec<OsString>, StoreError> {
    let mut arguments: Vec<OsString> = Vec::new();
    let mut push = |values: &[&str]| arguments.extend(values.iter().map(OsString::from));
    // `--disable` must come first: it stops curl from reading a personal configuration file (.curlrc) that could change what it does.
    push(&[
        "--disable",
        "--silent",
        "--show-error",
        "--globoff",
        "--max-time",
        MAX_SECONDS,
        "--connect-timeout",
        CONNECT_SECONDS,
        "--speed-limit",
        MIN_SPEED,
        "--speed-time",
        SLOW_SECONDS,
    ]);
    if request.local_http {
        push(&["--proto", "=http", "--noproxy", "*"]);
    } else {
        push(&["--proto", "=https", "--tlsv1.2"]);
    }
    match request.method {
        Method::Get => {}
        Method::Head => push(&["--head"]),
        Method::Put => push(&["--request", "PUT"]),
    }
    // Lets curl stop early when a server announces a larger body; the limit on the pipe holds anyway. Not for HEAD: its response has no body, and curl would compare the announced size of the object itself.
    if request.method != Method::Head {
        push(&["--max-filesize", &request.max_size.to_string()]);
    }
    if let Some((file, content_type)) = request.upload {
        // An absolute path is never `-`, which curl would read as its standard input.
        let file = std::path::absolute(file).map_err(|error| {
            StoreError(format!(
                "{} {}: cannot find the file to send: {error}",
                request.method.name(),
                request.shown
            ))
        })?;
        arguments.push(OsString::from("--upload-file"));
        arguments.push(file.into_os_string());
        arguments.push(OsString::from("--header"));
        arguments.push(OsString::from(format!("Content-Type: {content_type}")));
    }
    if let Some(sha256) = request.payload_sha256 {
        arguments.push(OsString::from("--header"));
        arguments.push(OsString::from(format!("x-amz-content-sha256: {sha256}")));
    }
    arguments.push(OsString::from("--output"));
    arguments.push(OsString::from("-"));
    arguments.push(OsString::from("--write-out"));
    arguments.push(OsString::from(format!(
        "%{{stderr}}\n{STATUS_MARKER}%{{http_code}}\n"
    )));
    if let Some((_, region)) = request.signing {
        arguments.push(OsString::from("--aws-sigv4"));
        arguments.push(OsString::from(format!("aws:amz:{region}:s3")));
        arguments.push(OsString::from("--config"));
        arguments.push(OsString::from("-"));
    }
    arguments.push(OsString::from("--url"));
    arguments.push(OsString::from(request.url));
    Ok(arguments)
}

/// What curl reads on its standard input when it signs a request: the credentials, as a configuration file.
fn credentials_config(credentials: &Credentials) -> String {
    let mut config = format!(
        "user = \"{}:{}\"\n",
        quote(&credentials.access_key),
        quote(&credentials.secret_key)
    );
    if let Some(token) = &credentials.session_token {
        config.push_str(&format!(
            "header = \"x-amz-security-token: {}\"\n",
            quote(token)
        ));
    }
    config
}

/// A curl version as its three numbers.
type Version = (u32, u32, u32);

/// The oldest curl this module works with: it reads the HTTP status from `%{stderr}` in `--write-out` (curl 7.63.0).
const MIN_CURL: Version = (7, 63, 0);
/// The oldest curl that signs an upload with the SHA-256 it is given in `x-amz-content-sha256` (curl 7.87.0); older versions sign it as an unchecked payload.
const MIN_CURL_SIGNING: Version = (7, 87, 0);

/// The version that `curl --version` reports first, such as `curl 8.5.0 (x86_64-pc-linux-gnu) …`.
fn parse_version(text: &str) -> Option<Version> {
    let version = text.split_whitespace().nth(1)?;
    let mut numbers = version
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(str::parse::<u32>);
    let major = numbers.next()?.ok()?;
    let minor = numbers.next()?.ok()?;
    let patch = numbers.next().and_then(Result::ok).unwrap_or(0);
    Some((major, minor, patch))
}

/// Why this curl cannot run a request, if it is too old.
fn version_problem(version: Version, signing: bool) -> Option<String> {
    let (major, minor, patch) = version;
    if version < MIN_CURL {
        return Some(format!(
            "the S3 and HTTPS stores need curl 7.63 or later; this curl is {major}.{minor}.{patch}"
        ));
    }
    if signing && version < MIN_CURL_SIGNING {
        return Some(format!(
            "signed S3 requests need curl 7.87 or later, which signs an upload's SHA-256; this curl is {major}.{minor}.{patch}"
        ));
    }
    None
}

/// Checks once per process that curl is new enough for a request.
fn check_version(signing: bool) -> Result<(), StoreError> {
    static VERSION: OnceLock<Result<Version, String>> = OnceLock::new();
    let version = VERSION
        .get_or_init(|| {
            let output = Command::new("curl")
                .args(["--disable", "--version"])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .map_err(|error| {
                    format!(
                        "cannot run curl, which the S3 and HTTPS stores use: {error}. Install curl."
                    )
                })?;
            parse_version(&String::from_utf8_lossy(&output.stdout))
                .ok_or_else(|| "cannot tell curl's version from `curl --version`".to_owned())
        })
        .clone()
        .map_err(StoreError)?;
    match version_problem(version, signing) {
        Some(problem) => Err(StoreError(problem)),
        None => Ok(()),
    }
}

/// Runs curl once.
fn run_once(request: &Request<'_>) -> Result<Attempt, StoreError> {
    let shown = format!("{} {}", request.method.name(), request.shown);
    let mut command = Command::new("curl");
    command.args(arguments(request)?);
    command.stdin(if request.signing.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| {
        StoreError(format!(
            "cannot run curl, which the S3 and HTTPS stores use: {error}. Install curl."
        ))
    })?;
    if let Some((credentials, _)) = request.signing
        && let Some(mut stdin) = child.stdin.take()
    {
        // Dropping `stdin` at the end of this block closes it, so curl reads the configuration to its end.
        if let Err(error) = stdin.write_all(credentials_config(credentials).as_bytes()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(StoreError(format!(
                "cannot pass the credentials to curl: {error}"
            )));
        }
    }
    // Standard error is read on its own thread, so neither pipe can fill up and stop curl.
    let stderr = child.stderr.take();
    let errors = thread::spawn(move || {
        let mut text = Vec::new();
        if let Some(mut stderr) = stderr {
            let _ = (&mut stderr).take(MAX_ERROR_OUTPUT).read_to_end(&mut text);
            let _ = std::io::copy(&mut stderr, &mut std::io::sink());
        }
        text
    });
    let read = match child.stdout.take() {
        Some(stdout) => read_body(stdout, request.max_size),
        None => Ok(Some(Vec::new())),
    };
    if !matches!(read, Ok(Some(_))) {
        let _ = child.kill();
    }
    let status = child
        .wait()
        .map_err(|error| StoreError(format!("{shown}: curl failed: {error}")))?;
    let errors = errors.join().unwrap_or_default();
    let body = match read {
        Ok(Some(body)) => body,
        Ok(None) => {
            return Err(StoreError(format!(
                "{shown}: the response is larger than the limit of {} bytes",
                request.max_size
            )));
        }
        Err(error) => {
            return Err(StoreError(format!(
                "{shown}: cannot read curl's output: {error}"
            )));
        }
    };
    let errors = String::from_utf8_lossy(&errors);
    let mut code = 0_u16;
    let mut message = Vec::new();
    for line in errors.lines() {
        match line.strip_prefix(STATUS_MARKER) {
            Some(value) => code = value.trim().parse().unwrap_or(0),
            None if !line.trim().is_empty() => message.push(line.trim()),
            None => {}
        }
    }
    // curl's own messages could name the file being uploaded, whose path is the operator's (and, for a private document, could name a person).
    let mut message = message.join(" ");
    if let Some((file, _)) = request.upload
        && let Ok(file) = std::path::absolute(file)
    {
        message = message.replace(&file.display().to_string(), "the file");
    }
    match classify(status.code(), code) {
        Class::Response => {}
        Class::TooLarge => {
            return Err(StoreError(format!(
                "{shown}: the response is larger than the limit of {} bytes",
                request.max_size
            )));
        }
        Class::TransientStatus => {
            return Ok(Attempt::Transient(StoreError(format!(
                "{shown}: HTTP status {code}"
            ))));
        }
        Class::TransientFailure => {
            return Ok(Attempt::Transient(StoreError(format!(
                "{shown}: curl failed ({status}): {message}"
            ))));
        }
        Class::Failed => {
            return Err(StoreError(format!(
                "{shown}: curl failed ({status}): {message}"
            )));
        }
    }
    Ok(Attempt::Done(Response { status: code, body }))
}

/// Reads a response body of at most `max_size` bytes; `None` if it is larger, after reading no more than one byte too many.
fn read_body(reader: impl Read, max_size: u64) -> std::io::Result<Option<Vec<u8>>> {
    let mut body = Vec::new();
    reader
        .take(max_size.saturating_add(1))
        .read_to_end(&mut body)?;
    Ok(u64::try_from(body.len())
        .is_ok_and(|len| len <= max_size)
        .then_some(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request<'a>(
        method: Method,
        upload: Option<(&'a Path, &'a str)>,
        local_http: bool,
        signing: Option<(&'a Credentials, &'a str)>,
    ) -> Request<'a> {
        Request {
            method,
            url: "https://s3.example.invalid/bucket/key",
            shown: "…/bucket/key",
            upload,
            payload_sha256: upload.map(|_| Sha256::of(b"payload")),
            max_size: 1_000,
            local_http,
            signing,
        }
    }

    fn strings(arguments: &[OsString]) -> Vec<String> {
        arguments
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect()
    }

    fn has_pair(arguments: &[String], name: &str, value: &str) -> bool {
        arguments
            .windows(2)
            .any(|pair| pair[0] == name && pair[1] == value)
    }

    #[test]
    fn curl_reads_no_personal_configuration_and_speaks_only_https() {
        let get = strings(&arguments(&request(Method::Get, None, false, None)).unwrap());
        // A .curlrc could change everything curl does, so `--disable` must be the very first argument.
        assert_eq!(get.first().map(String::as_str), Some("--disable"));
        assert!(has_pair(&get, "--proto", "=https"));
        assert!(get.iter().any(|argument| argument == "--globoff"));
        assert!(has_pair(&get, "--max-filesize", "1000"));
        assert!(!get.iter().any(|argument| argument == "--noproxy"));
        let local = strings(&arguments(&request(Method::Get, None, true, None)).unwrap());
        assert!(has_pair(&local, "--proto", "=http"));
        assert!(has_pair(&local, "--noproxy", "*"));
        // A HEAD response has no body; `--max-filesize` would compare the object's own size.
        let head = strings(&arguments(&request(Method::Head, None, false, None)).unwrap());
        assert!(head.iter().any(|argument| argument == "--head"));
        assert!(!head.iter().any(|argument| argument == "--max-filesize"));
    }

    #[test]
    fn credentials_never_reach_the_command_line() {
        let credentials = Credentials {
            access_key: "AKIDEXAMPLE".to_owned(),
            secret_key: "secret\"key\\".to_owned(),
            session_token: Some("session-token".to_owned()),
        };
        let file = Path::new("document.docx");
        let put = strings(
            &arguments(&request(
                Method::Put,
                Some((file, "application/x")),
                false,
                Some((&credentials, "auto")),
            ))
            .unwrap(),
        );
        for argument in &put {
            for secret in ["AKIDEXAMPLE", "secret", "session-token"] {
                assert!(!argument.contains(secret), "{argument}");
            }
        }
        assert!(has_pair(&put, "--config", "-"));
        assert!(has_pair(&put, "--aws-sigv4", "aws:amz:auto:s3"));
        // The upload is the file itself, by an absolute path (never `-`, curl's standard input), and its SHA-256 is signed with it.
        let upload = put
            .windows(2)
            .find(|pair| pair[0] == "--upload-file")
            .map(|pair| pair[1].clone())
            .unwrap();
        assert!(Path::new(&upload).is_absolute());
        assert!(has_pair(
            &put,
            "--header",
            &format!("x-amz-content-sha256: {}", Sha256::of(b"payload"))
        ));
        assert_eq!(
            credentials_config(&credentials),
            "user = \"AKIDEXAMPLE:secret\\\"key\\\\\"\nheader = \"x-amz-security-token: session-token\"\n"
        );
    }

    #[test]
    fn only_failures_that_may_pass_are_tried_again() {
        assert_eq!(classify(Some(0), 200), Class::Response);
        assert_eq!(classify(Some(0), 404), Class::Response);
        assert_eq!(classify(Some(0), 503), Class::TransientStatus);
        assert_eq!(classify(Some(0), 429), Class::TransientStatus);
        assert_eq!(classify(Some(0), 0), Class::Failed);
        // An oversized response would be just as large the next time.
        assert_eq!(classify(Some(63), 0), Class::TooLarge);
        for transient in [5, 6, 7, 18, 28, 35, 52, 55, 56, 92] {
            assert_eq!(
                classify(Some(transient), 0),
                Class::TransientFailure,
                "{transient}"
            );
        }
        // A refused certificate, a malformed address, an unreadable upload, a killed curl.
        for permanent in [Some(3), Some(26), Some(60), None] {
            assert_eq!(classify(permanent, 0), Class::Failed, "{permanent:?}");
        }
    }

    #[test]
    fn curl_versions_are_read_and_checked() {
        assert_eq!(
            parse_version("curl 8.5.0 (x86_64-pc-linux-gnu) libcurl/8.5.0 OpenSSL/3.0.13"),
            Some((8, 5, 0))
        );
        assert_eq!(
            parse_version("curl 7.87.0-DEV (Windows) libcurl/7.87.0-DEV Schannel"),
            Some((7, 87, 0))
        );
        assert_eq!(parse_version("curl 8.10"), Some((8, 10, 0)));
        assert_eq!(parse_version("not curl"), None);
        assert_eq!(version_problem((8, 5, 0), true), None);
        assert_eq!(version_problem((7, 87, 0), true), None);
        assert_eq!(version_problem((7, 81, 0), false), None);
        assert!(version_problem((7, 81, 0), true).is_some_and(|problem| problem.contains("7.87")));
        assert!(version_problem((7, 62, 9), false).is_some_and(|problem| problem.contains("7.63")));
    }

    #[test]
    fn bodies_stop_at_their_limit() {
        assert_eq!(read_body(&b"abc"[..], 3).unwrap(), Some(b"abc".to_vec()));
        assert_eq!(read_body(&b"abcd"[..], 3).unwrap(), None);
        // An endless body is read only to one byte past the limit.
        assert_eq!(read_body(std::io::repeat(7), 1_000).unwrap(), None);
        assert_eq!(read_body(&b""[..], 0).unwrap(), Some(Vec::new()));
    }

    #[test]
    fn endpoints_are_https_or_local() {
        assert_eq!(
            endpoint("https://s3.example.com/"),
            Ok(("https://s3.example.com".to_owned(), false))
        );
        assert_eq!(
            endpoint("http://127.0.0.1:9000"),
            Ok(("http://127.0.0.1:9000".to_owned(), true))
        );
        assert_eq!(
            endpoint("http://[::1]:9000"),
            Ok(("http://[::1]:9000".to_owned(), true))
        );
        assert_eq!(
            endpoint("https://[2001:db8::1]:8443"),
            Ok(("https://[2001:db8::1]:8443".to_owned(), false))
        );
        for bad in [
            "s3.example.com",
            "http://example.com",
            "ftp://example.com",
            "https://user@example.com",
            "https://example.com/path",
            "https://",
            "https://h[1-3].example.com",
            "https://s3-{a,b}.example.com",
            "https://[1-3]",
            "https://[::1]x",
            "https://example.com:",
            "https://example.com:443:1",
            "https://example.com:http",
        ] {
            assert!(endpoint(bad).is_err(), "{bad} was accepted");
        }
    }

    #[test]
    fn paths_and_configuration_values() {
        assert!(is_simple_path(""));
        assert!(is_simple_path("public/v1_2-x.y"));
        for bad in ["/a", "a/", "a//b", "a/../b", ".", "a b", "a%2Fb", "a[1]"] {
            assert!(!is_simple_path(bad), "{bad} was accepted");
        }
        assert_eq!(quote(r#"a"b\c"#), r#"a\"b\\c"#);
    }
}
