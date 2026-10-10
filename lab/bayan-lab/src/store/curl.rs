//! HTTP requests through the `curl` program, shared by the S3 and HTTPS stores.
//!
//! Every supported platform has curl (Windows 10 and later include it), so the lab needs no HTTP or TLS library of its own, as xtask's crates.io requests do. Each request runs one curl process that reads no personal configuration file (`--disable`), allows only HTTPS (or plain HTTP to this machine, for tests and local servers), never follows a redirect, expands no URL patterns (`--globoff`), and has a time limit. The response body comes back through a pipe, and this module stops reading, and stops curl, as soon as the body exceeds the request's limit, whatever curl's version; no document is ever written to a temporary file. An upload is read by curl from the file the document was read from. Transient failures (no connection, a timeout, a broken transfer, or HTTP status 408, 429 or 5xx) are retried up to three times, each time from the start, so a failed response can never mix with a later one.
//!
//! **Credentials** come only from the environment variables `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` and, for temporary credentials, `AWS_SESSION_TOKEN`, and only the S3 store uses them. curl signs requests with AWS Signature Version 4 itself (`--aws-sigv4`, curl 7.75 or later): the tool implements no cryptography. The credentials are handed to curl on its standard input (`--config -`), never on its command line, where other users of the machine could see them, and they never appear in messages.

use std::io::{Read, Write as _};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use super::StoreError;

/// How long one request may take, in seconds.
const MAX_SECONDS: &str = "600";
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

/// Runs a request, trying again after transient failures, and returns the response.
///
/// # Errors
///
/// When curl cannot run, the response is larger than the request's limit, or the request still fails after the retries.
pub fn run(request: &Request<'_>) -> Result<Response, StoreError> {
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

/// Runs curl once.
fn run_once(request: &Request<'_>) -> Result<Attempt, StoreError> {
    let shown = format!("{} {}", request.method.name(), request.shown);
    let mut command = Command::new("curl");
    // `--disable` must come first: it stops curl from reading a personal configuration file (.curlrc) that could change what it does.
    command.args([
        "--disable",
        "--silent",
        "--show-error",
        "--globoff",
        "--max-time",
        MAX_SECONDS,
    ]);
    if request.local_http {
        command.args(["--proto", "=http", "--noproxy", "*"]);
    } else {
        command.args(["--proto", "=https", "--tlsv1.2"]);
    }
    match request.method {
        Method::Get => {}
        Method::Head => {
            command.arg("--head");
        }
        Method::Put => {
            command.args(["--request", "PUT"]);
        }
    }
    // Lets curl stop early when a server announces a larger body; the limit below holds anyway. Not for HEAD: its response has no body, and curl would compare the announced size of the object itself.
    if request.method != Method::Head {
        command
            .arg("--max-filesize")
            .arg(request.max_size.to_string());
    }
    if let Some((file, content_type)) = request.upload {
        // An absolute path is never `-`, which curl would read as its standard input.
        let file = std::path::absolute(file).map_err(|error| {
            StoreError(format!("{shown}: cannot find the file to send: {error}"))
        })?;
        command.arg("--upload-file").arg(file);
        command
            .arg("--header")
            .arg(format!("Content-Type: {content_type}"));
    }
    command.args(["--output", "-"]);
    command
        .arg("--write-out")
        .arg(format!("%{{stderr}}\n{STATUS_MARKER}%{{http_code}}\n"));
    if let Some((_, region)) = request.signing {
        command
            .arg("--aws-sigv4")
            .arg(format!("aws:amz:{region}:s3"));
        command.args(["--config", "-"]);
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }
    command.arg("--url").arg(request.url);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| {
        StoreError(format!(
            "cannot run curl, which the S3 and HTTPS stores use: {error}. Install curl."
        ))
    })?;
    if let Some((credentials, _)) = request.signing
        && let Some(mut stdin) = child.stdin.take()
    {
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
        // Dropping `stdin` at the end of this block closes it, so curl reads the configuration to its end.
        if let Err(error) = stdin.write_all(config.as_bytes()) {
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
    match status.code() {
        Some(0) if code != 0 => {}
        // curl stopped because the server announced a body larger than `--max-filesize`.
        Some(63) => {
            return Err(StoreError(format!(
                "{shown}: the response is larger than the limit of {} bytes",
                request.max_size
            )));
        }
        exit => {
            let error = StoreError(format!(
                "{shown}: curl failed ({status}): {}",
                message.join(" ")
            ));
            // Failures to resolve, connect, send or receive, and timeouts, may pass; anything else (a refused certificate, a malformed address, an unreadable upload) will not.
            return if matches!(exit, Some(5 | 6 | 7 | 28 | 35 | 52 | 55 | 56)) {
                Ok(Attempt::Transient(error))
            } else {
                Err(error)
            };
        }
    }
    if matches!(code, 408 | 429) || (500..600).contains(&code) {
        return Ok(Attempt::Transient(StoreError(format!(
            "{shown}: HTTP status {code}"
        ))));
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
