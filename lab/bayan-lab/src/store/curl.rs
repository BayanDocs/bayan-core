//! HTTP requests through the `curl` program, shared by the S3 and HTTPS stores.
//!
//! Every supported platform has curl (Windows 10 and later include it), so the lab needs no HTTP or TLS library of its own, as xtask's crates.io requests do. Each request runs one curl process that reads no personal configuration file (`--disable`), allows only HTTPS (or plain HTTP to this machine, for tests and local servers), never follows a redirect, caps the response size and the time, and writes the response body to a temporary file that is deleted afterwards.
//!
//! **Credentials** come only from the environment variables `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` and, for temporary credentials, `AWS_SESSION_TOKEN`, and only the S3 store uses them. curl signs requests with AWS Signature Version 4 itself (`--aws-sigv4`, curl 7.75 or later): the tool implements no cryptography. The credentials are handed to curl on its standard input (`--config -`), never on its command line, where other users of the machine could see them, and they never appear in messages.

use std::fs;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use super::StoreError;

/// How long one request may take, in seconds.
const MAX_SECONDS: &str = "600";

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

/// Checks an endpoint such as `https://s3.example.com` or `http://127.0.0.1:9000`: a scheme and a host with an optional port, nothing else. Returns it without a trailing `/`, and whether it is plain HTTP to this machine.
///
/// # Errors
///
/// What is wrong with it, for a message.
pub fn endpoint(text: &str) -> Result<(String, bool), &'static str> {
    let text = text.trim_end_matches('/');
    let (scheme, host) = text
        .split_once("://")
        .ok_or("the address must start with https://")?;
    let host_ok = !host.is_empty()
        && host.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':' | b'[' | b']')
        });
    if !host_ok {
        return Err("the host must be a host name with an optional port, without user names");
    }
    let hostname = host
        .rsplit_once(':')
        .filter(|(_, port)| port.bytes().all(|byte| byte.is_ascii_digit()))
        .map_or(host, |(name, _)| name);
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

/// A file in the temporary directory, deleted when dropped.
pub struct TempFile {
    path: PathBuf,
}

impl TempFile {
    /// A new, unused name for a temporary file; the file is created by whoever writes it.
    pub fn new(purpose: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let number = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "bayan-lab-{}-{number}-{purpose}",
            std::process::id()
        ));
        Self { path }
    }

    /// Writes the file.
    ///
    /// # Errors
    ///
    /// When it cannot be written.
    pub fn write(&self, bytes: &[u8]) -> Result<(), StoreError> {
        fs::write(&self.path, bytes)
            .map_err(|error| StoreError(format!("cannot stage the upload: {error}")))
    }

    /// Reads the file, which may hold at most `limit` bytes.
    ///
    /// # Errors
    ///
    /// When it cannot be read or is larger than `limit`.
    pub fn read(&self, limit: u64) -> Result<Vec<u8>, StoreError> {
        let bytes = fs::read(&self.path)
            .map_err(|error| StoreError(format!("cannot read curl's output: {error}")))?;
        if !u64::try_from(bytes.len()).is_ok_and(|len| len <= limit) {
            return Err(StoreError("a response is larger than the limit".to_owned()));
        }
        Ok(bytes)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        // Nothing to do if it was never written.
        let _ = fs::remove_file(&self.path);
    }
}

/// One request.
pub struct Request<'a> {
    /// The method.
    pub method: Method,
    /// The full URL.
    pub url: &'a str,
    /// The URL as messages show it, without parts that need not be shown.
    pub shown: &'a str,
    /// Receives the response body.
    pub output: &'a TempFile,
    /// The request body of a PUT and its content type.
    pub upload: Option<(&'a TempFile, &'a str)>,
    /// The largest response body accepted, in bytes.
    pub max_size: u64,
    /// Plain HTTP to this machine instead of HTTPS.
    pub local_http: bool,
    /// Sign the request with AWS Signature Version 4: the credentials and the region.
    pub signing: Option<(&'a Credentials, &'a str)>,
}

/// Runs curl for one request and returns the HTTP status.
///
/// # Errors
///
/// When curl cannot run or the request fails before an HTTP status arrives.
pub fn run(request: &Request<'_>) -> Result<u16, StoreError> {
    let mut command = Command::new("curl");
    // `--disable` must come first: it stops curl from reading a personal configuration file (.curlrc) that could change what it does.
    command.args([
        "--disable",
        "--silent",
        "--show-error",
        "--max-time",
        MAX_SECONDS,
        "--retry",
        "3",
    ]);
    if request.local_http {
        command.args(["--proto", "=http", "--noproxy", "*"]);
    } else {
        command.args(["--proto", "=https", "--tlsv1.2"]);
    }
    command
        .arg("--max-filesize")
        .arg(request.max_size.to_string());
    match request.method {
        Method::Get => {}
        Method::Head => {
            command.arg("--head");
        }
        Method::Put => {
            command.args(["--request", "PUT"]);
        }
    }
    if let Some((file, content_type)) = request.upload {
        command.arg("--upload-file").arg(&file.path);
        command
            .arg("--header")
            .arg(format!("Content-Type: {content_type}"));
    }
    command.arg("--output").arg(&request.output.path);
    command.args(["--write-out", "%{http_code}"]);
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
        stdin
            .write_all(config.as_bytes())
            .map_err(|error| StoreError(format!("cannot pass the credentials to curl: {error}")))?;
    }
    let result = child
        .wait_with_output()
        .map_err(|error| StoreError(format!("curl failed: {error}")))?;
    let status = String::from_utf8_lossy(&result.stdout);
    let code = status.trim().parse::<u16>().unwrap_or(0);
    if !result.status.success() || code == 0 {
        let message = String::from_utf8_lossy(&result.stderr);
        return Err(StoreError(format!(
            "{} {}: curl failed ({}): {}",
            request.method.name(),
            request.shown,
            result.status,
            message.trim()
        )));
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        for bad in [
            "s3.example.com",
            "http://example.com",
            "ftp://example.com",
            "https://user@example.com",
            "https://example.com/path",
            "https://",
        ] {
            assert!(endpoint(bad).is_err(), "{bad} was accepted");
        }
    }

    #[test]
    fn paths_and_configuration_values() {
        assert!(is_simple_path(""));
        assert!(is_simple_path("public/v1_2-x.y"));
        for bad in ["/a", "a/", "a//b", "a/../b", ".", "a b", "a%2Fb"] {
            assert!(!is_simple_path(bad), "{bad} was accepted");
        }
        assert_eq!(quote(r#"a"b\c"#), r#"a\"b\\c"#);
    }
}
