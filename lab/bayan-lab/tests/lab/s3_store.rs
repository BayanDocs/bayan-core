//! The S3-compatible store and the read-only HTTPS store, end to end: the `bayan-lab` binary keeps a corpus in a fake S3 server on this machine, through the real `curl` program (7.75 or later, as the S3 store requires), and reads it back at the bucket's plain address.
//!
//! The fake server answers the four requests the store makes (PUT, GET and HEAD of an object, and `ListObjectsV2` in pages of two keys) and records every request, so the tests can check what reached the wire: requests signed with AWS Signature Version 4 when credentials are set, unsigned ones without, and the secret key never sent. It does not check signatures: computing them is curl's job, and the store implements no cryptography.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
use std::thread;

use bayan_lab::hash::Sha256;

use crate::support::{Docx, temporary_folder};

const BUCKET: &str = "corpus-test";
const ACCESS_KEY: &str = "AKIDEXAMPLE";
const SECRET_KEY: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
const SESSION_TOKEN: &str = "session-token-for-the-test";

/// One request as the fake server received it.
#[derive(Debug, Clone)]
struct Request {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(found, _)| found == name)
            .map(|(_, value)| value.as_str())
    }
}

type Objects = Arc<Mutex<BTreeMap<String, Vec<u8>>>>;
type Requests = Arc<Mutex<Vec<Request>>>;

/// A fake S3 server with one bucket, on a free port of 127.0.0.1.
struct FakeS3 {
    port: u16,
    objects: Objects,
    requests: Requests,
}

impl FakeS3 {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let objects = Objects::default();
        let requests = Requests::default();
        let (shared_objects, shared_requests) = (Arc::clone(&objects), Arc::clone(&requests));
        // The thread serves until the test program ends.
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let _ = serve(stream, &shared_objects, &shared_requests);
            }
        });
        Self {
            port,
            objects,
            requests,
        }
    }

    fn address(&self) -> String {
        format!(
            "s3://{BUCKET}/public/v1?endpoint=http://127.0.0.1:{}&region=auto",
            self.port
        )
    }

    fn take_requests(&self) -> Vec<Request> {
        std::mem::take(&mut *self.requests.lock().unwrap())
    }
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::new();
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        let hex = bytes
            .get(index + 1..index + 3)
            .and_then(|pair| std::str::from_utf8(pair).ok())
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match (byte, hex) {
            (b'%', Some(value)) => {
                decoded.push(value);
                index += 3;
            }
            _ => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).unwrap()
}

fn respond(
    stream: &mut TcpStream,
    status: &str,
    body: &[u8],
    send_body: bool,
) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/xml\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    if send_body {
        stream.write_all(body)?;
    }
    stream.flush()
}

fn serve(mut stream: TcpStream, objects: &Objects, requests: &Requests) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut words = line.split_whitespace();
    let method = words.next().unwrap_or_default().to_owned();
    let target = words.next().unwrap_or_default().to_owned();
    let mut headers = Vec::new();
    loop {
        let mut header = String::new();
        reader.read_line(&mut header)?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
        }
    }
    let request = Request {
        method,
        target,
        headers,
        body: Vec::new(),
    };
    if request
        .header("expect")
        .is_some_and(|value| value.eq_ignore_ascii_case("100-continue"))
    {
        stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
    }
    let length: usize = request
        .header("content-length")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let request = Request { body, ..request };
    requests.lock().unwrap().push(request.clone());

    let (path, query) = request
        .target
        .split_once('?')
        .unwrap_or((request.target.as_str(), ""));
    let bucket_root = format!("/{BUCKET}");
    if path == bucket_root && request.method == "GET" {
        let parameters: BTreeMap<String, String> = query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .map(|(name, value)| (name.to_owned(), percent_decode(value)))
            .collect();
        let prefix = parameters.get("prefix").cloned().unwrap_or_default();
        let start: usize = parameters
            .get("continuation-token")
            .and_then(|token| token.strip_prefix("page-"))
            .and_then(|number| number.parse().ok())
            .unwrap_or(0);
        let keys: Vec<String> = objects
            .lock()
            .unwrap()
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .cloned()
            .collect();
        let page: Vec<&String> = keys.iter().skip(start).take(2).collect();
        let truncated = start + page.len() < keys.len();
        let mut xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Name>{BUCKET}</Name><Prefix>{prefix}</Prefix><KeyCount>{}</KeyCount><MaxKeys>2</MaxKeys><IsTruncated>{truncated}</IsTruncated>"#,
            page.len()
        );
        if truncated {
            xml.push_str(&format!(
                "<NextContinuationToken>page-{}</NextContinuationToken>",
                start + page.len()
            ));
        }
        for key in page {
            xml.push_str(&format!("<Contents><Key>{key}</Key></Contents>"));
        }
        xml.push_str("</ListBucketResult>");
        return respond(&mut stream, "200 OK", xml.as_bytes(), true);
    }
    let Some(key) = path.strip_prefix(&format!("{bucket_root}/")) else {
        return respond(&mut stream, "400 Bad Request", b"", true);
    };
    let key = percent_decode(key);
    match request.method.as_str() {
        "PUT" => {
            objects.lock().unwrap().insert(key, request.body);
            respond(&mut stream, "200 OK", b"", true)
        }
        "GET" | "HEAD" => {
            let found = objects.lock().unwrap().get(&key).cloned();
            match found {
                Some(bytes) => respond(&mut stream, "200 OK", &bytes, request.method == "GET"),
                None => respond(&mut stream, "404 Not Found", b"", true),
            }
        }
        _ => respond(&mut stream, "405 Method Not Allowed", b"", true),
    }
}

/// Runs the `bayan-lab` binary with `args` and exactly the S3 credentials in `credentials`.
fn bayan_lab(args: &[&str], credentials: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bayan-lab"));
    command.args(args);
    for name in [
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "BAYAN_LAB_MANIFEST",
        "BAYAN_LAB_STORE",
    ] {
        command.env_remove(name);
    }
    for (name, value) in credentials {
        command.env(name, value);
    }
    command.output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn path(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn a_corpus_kept_in_an_s3_bucket() {
    let server = FakeS3::start();
    let store = server.address();
    let folder = temporary_folder("s3");
    let manifest = folder.join("manifest.json");
    let license = folder.join("LICENSE");
    fs::write(&license, "MIT License\n").unwrap();
    let mut documents = Vec::new();
    for word in ["one", "two", "three"] {
        let bytes = Docx::new(&format!("<w:p><w:r><w:t>{word}</w:t></w:r></w:p>")).bytes();
        fs::write(folder.join(format!("{word}.docx")), &bytes).unwrap();
        documents.push(Sha256::of(&bytes));
    }
    let signed = [
        ("AWS_ACCESS_KEY_ID", ACCESS_KEY),
        ("AWS_SECRET_ACCESS_KEY", SECRET_KEY),
        ("AWS_SESSION_TOKEN", SESSION_TOKEN),
    ];
    let license_file = format!("LICENSE={}", path(&license));
    let common = ["--manifest", path(&manifest), "--store", store.as_str()];
    let mut source_add = vec![
        "corpus",
        "source",
        "add",
        "example",
        "--url",
        "https://example.invalid/r",
        "--revision",
        "1",
        "--license",
        "MIT",
        "--copyright",
        "Copyright (c) Example",
        "--license-file",
        &license_file,
    ];
    source_add.extend_from_slice(&common);
    let added = bayan_lab(&source_add, &signed);
    assert!(added.status.success(), "{}", stderr(&added));
    let root = path(&folder).to_owned();
    let mut add = vec!["corpus", "add", "--source", "example", "--root", &root];
    add.extend_from_slice(&common);
    add.extend_from_slice(&["--", "one.docx", "two.docx", "three.docx"]);
    let added = bayan_lab(&add, &signed);
    assert!(added.status.success(), "{}", stderr(&added));
    let mut verify = vec!["corpus", "verify"];
    verify.extend_from_slice(&common);
    let verified = bayan_lab(&verify, &signed);
    assert!(
        verified.status.success(),
        "{}{}",
        stdout(&verified),
        stderr(&verified)
    );
    assert!(
        stdout(&verified).contains("3 of 3 documents and 1 license texts present with matching SHA-256; 0 unexpected objects"),
        "{}",
        stdout(&verified)
    );

    // The objects are where the store's layout says, under the prefix, with the bytes their keys name.
    {
        let objects = server.objects.lock().unwrap();
        assert_eq!(objects.len(), 4);
        for sha256 in &documents {
            let hex = sha256.to_string();
            let key = format!("public/v1/objects/{}/{}.docx", &hex[..2], &hex[2..]);
            assert_eq!(
                objects.get(&key).map(|bytes| Sha256::of(bytes)),
                Some(*sha256)
            );
        }
        assert!(objects.contains_key(&format!(
            "public/v1/licenses/{}.txt",
            Sha256::of(b"MIT License\n")
        )));
    }

    // Every request was signed, carried the session token, and never the secret key.
    let requests = server.take_requests();
    assert!(requests.iter().any(|request| request.method == "PUT"));
    assert!(
        requests
            .iter()
            .any(|request| request.target.contains("list-type=2"))
    );
    assert!(
        requests
            .iter()
            .any(|request| request.target.contains("continuation-token=page-2")),
        "the listing was not followed to its second page"
    );
    for request in &requests {
        let authorization = request.header("authorization").unwrap_or_default();
        assert!(
            authorization.starts_with(&format!("AWS4-HMAC-SHA256 Credential={ACCESS_KEY}/"))
                && authorization.contains("/auto/s3/aws4_request"),
            "{} {}: {authorization}",
            request.method,
            request.target
        );
        assert!(request.header("x-amz-date").is_some());
        assert_eq!(request.header("x-amz-security-token"), Some(SESSION_TOKEN));
        let secret_seen = request.target.contains(SECRET_KEY)
            || request
                .headers
                .iter()
                .any(|(_, value)| value.contains(SECRET_KEY))
            || String::from_utf8_lossy(&request.body).contains(SECRET_KEY);
        assert!(!secret_seen, "the secret key was sent");
    }

    // An object the manifest does not know is reported, also on a later page of the listing.
    server.objects.lock().unwrap().insert(
        "public/v1/objects/zz/unexpected.docx".to_owned(),
        b"x".to_vec(),
    );
    let verified = bayan_lab(&verify, &signed);
    assert_eq!(verified.status.code(), Some(1));
    assert!(
        stderr(&verified).contains("unexpected object in the store: objects/zz/unexpected.docx")
    );
    server
        .objects
        .lock()
        .unwrap()
        .remove("public/v1/objects/zz/unexpected.docx");

    // Without credentials the requests are anonymous, which is enough to read a public bucket.
    server.take_requests();
    let anonymous = bayan_lab(&verify, &[]);
    assert!(anonymous.status.success(), "{}", stderr(&anonymous));
    let requests = server.take_requests();
    assert!(!requests.is_empty());
    assert!(
        requests
            .iter()
            .all(|request| request.header("authorization").is_none())
    );

    // Half a pair of credentials is a mistake, not anonymity.
    let half = bayan_lab(&verify, &[("AWS_ACCESS_KEY_ID", ACCESS_KEY)]);
    assert_eq!(half.status.code(), Some(2));
    assert!(
        stderr(&half).contains("set both AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY, or neither")
    );

    // The same bucket read at its plain address, as a public bucket's custom domain serves it: read-only, never signed, not listable.
    let https = format!("http://127.0.0.1:{}/{BUCKET}/public/v1", server.port);
    let read_only = [
        "corpus",
        "verify",
        "--manifest",
        path(&manifest),
        "--store",
        https.as_str(),
    ];
    server.take_requests();
    let verified = bayan_lab(&read_only, &signed);
    assert!(verified.status.success(), "{}", stderr(&verified));
    assert!(
        stdout(&verified).contains("3 of 3 documents and 1 license texts present with matching SHA-256; unexpected objects not checked (this store cannot be listed)"),
        "{}",
        stdout(&verified)
    );
    let requests = server.take_requests();
    assert_eq!(requests.len(), 4, "one GET per object, and no listing");
    assert!(requests.iter().all(|request| request.method == "GET"
        && request.header("authorization").is_none()
        && request.header("x-amz-security-token").is_none()));
    let new_document = folder.join("four.docx");
    fs::write(&new_document, Docx::new("<w:p/>").bytes()).unwrap();
    let written = bayan_lab(
        &[
            "corpus",
            "add",
            "--manifest",
            path(&manifest),
            "--store",
            https.as_str(),
            "--source",
            "example",
            path(&new_document),
        ],
        &signed,
    );
    assert_eq!(written.status.code(), Some(2));
    assert!(
        stderr(&written).contains("is read-only"),
        "{}",
        stderr(&written)
    );
}
