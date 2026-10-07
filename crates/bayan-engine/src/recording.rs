//! Record and replay ([engine protocol specification][spec] §10).
//!
//! A recording holds the session state when it started, every message the engine handled since (or the `bayan_render_tile` call), each with the time it arrived and a digest of what the engine produced, and every blob those messages used. Replaying it starts a new engine from that state, handles every entry again and compares the digests, so a session reproduces exactly on any platform with the same engine version. Recordings contain document content: they are created only when a shell asks for one and never leave the machine by themselves.
//!
//! Recordings are untrusted input when they are replayed: they are read with the limits of spec §13, every identifier and offset in them is checked, and the replay cannot itself replay.
//!
//! [spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/engine-protocol.md

use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;
use std::sync::Arc;

use serde::de::{Error as _, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::base64;
use crate::blobs::{BlobId, BlobStore};
use crate::config::Config;
use crate::digest;
use crate::engine::{ENGINE_VERSION, Engine, Snapshot};
use crate::limits::{MAX_BLOBS, MAX_RECORDING_BYTES, MAX_RECORDING_ENTRIES};
use crate::protocol::{ErrorCode, ErrorInfo, LAYOUT_EPOCH, PROTOCOL_VERSION, ReplayReport};

/// The `format` field of every recording.
const FORMAT: &str = "bayan-engine-recording";

/// The version of the recording format.
const FORMAT_VERSION: u32 = 1;

/// Room for the fields of a recording outside its entries, blobs and initial state.
const HEADER_ALLOWANCE: usize = 512;

/// What an entry of a recording records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EntryKind {
    /// A message.
    Message,
    /// A `bayan_render_tile` call.
    Tile,
}

/// One entry of a recording.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    /// Milliseconds since the recording started, from the host's clock.
    t_ms: u64,
    kind: EntryKind,
    /// The message, or the tile request, exactly as it arrived, if it was UTF-8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    /// The message as base64, if it was not UTF-8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base64: Option<String>,
    /// A tile's width in pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    width: Option<u32>,
    /// A tile's height in pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    height: Option<u32>,
    /// What handling it produced (spec §10).
    digest: String,
}

/// A blob of a recording.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RecordedBlob {
    id: BlobId,
    base64: String,
}

/// A recording as JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct RecordingFile {
    format: String,
    format_version: u32,
    engine_version: String,
    protocol_version: u32,
    layout_epoch: u32,
    initial_state: Snapshot,
    #[serde(deserialize_with = "bounded")]
    blobs: Vec<RecordedBlob>,
    #[serde(deserialize_with = "bounded")]
    entries: Vec<Entry>,
    truncated: bool,
}

/// Reads a list of at most [`MAX_RECORDING_ENTRIES`] items, refusing longer ones while reading, before they take memory.
fn bounded<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Bounded<T>(PhantomData<T>);

    impl<'de, T: Deserialize<'de>> Visitor<'de> for Bounded<T> {
        type Value = Vec<T>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a list of at most 100,000 items")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Vec<T>, A::Error> {
            let mut items = Vec::new();
            while let Some(item) = sequence.next_element()? {
                if items.len() >= MAX_RECORDING_ENTRIES {
                    return Err(A::Error::custom("too many items"));
                }
                items.push(item);
            }
            Ok(items)
        }
    }

    deserializer.deserialize_seq(Bounded(PhantomData))
}

/// A recording being made.
#[derive(Debug)]
pub(crate) struct Recorder {
    initial_state: Snapshot,
    /// The host time of the first entry; times are relative to it.
    start_ms: Option<u64>,
    entries: Vec<Entry>,
    blobs: BTreeMap<BlobId, Arc<[u8]>>,
    /// An upper bound of the recording's size as JSON.
    size: usize,
    truncated: bool,
}

/// The size of a value as JSON.
fn json_size<T: Serialize>(value: &T) -> usize {
    serde_json::to_string(value).map_or(usize::MAX, |json| json.len())
}

impl Recorder {
    /// Starts a recording from a session state.
    pub(crate) fn start(initial_state: &Snapshot) -> Result<Self, ErrorInfo> {
        let size = json_size(initial_state).saturating_add(HEADER_ALLOWANCE);
        if size > MAX_RECORDING_BYTES {
            return Err(ErrorInfo::new(ErrorCode::LimitExceeded).with_text("limit", "recording"));
        }
        Ok(Self {
            initial_state: initial_state.clone(),
            start_ms: None,
            entries: Vec::new(),
            blobs: BTreeMap::new(),
            size,
            truncated: false,
        })
    }

    /// Stops the recording early and marks it truncated, when something happened that it cannot hold, such as a message the host answered without handing it to the engine.
    pub(crate) fn stop_early(&mut self) {
        self.truncated = true;
    }

    /// Adds an entry and the blobs it used, unless that would exceed a limit; then the recording stops early and is marked truncated.
    fn push(&mut self, mut entry: Entry, received_ms: u64, blobs: Vec<(BlobId, Arc<[u8]>)>) {
        if self.truncated {
            return;
        }
        let start = *self.start_ms.get_or_insert(received_ms);
        entry.t_ms = received_ms.saturating_sub(start);
        let mut size = self.size.saturating_add(json_size(&entry) + 1);
        let mut new_blobs = Vec::new();
        for (id, bytes) in blobs {
            if self.blobs.contains_key(&id) || new_blobs.iter().any(|(known, _)| *known == id) {
                continue;
            }
            // The base64 text, its identifier and the punctuation around them.
            size = size.saturating_add(bytes.len().div_ceil(3) * 4 + 48);
            new_blobs.push((id, bytes));
        }
        if size > MAX_RECORDING_BYTES
            || self.entries.len() >= MAX_RECORDING_ENTRIES
            || self.blobs.len() + new_blobs.len() > MAX_BLOBS
        {
            self.truncated = true;
            return;
        }
        self.size = size;
        self.entries.push(entry);
        self.blobs.extend(new_blobs);
    }

    /// Records a message and the digest of what handling it produced.
    pub(crate) fn record_message(
        &mut self,
        message: &[u8],
        received_ms: u64,
        digest: String,
        blobs: Vec<(BlobId, Arc<[u8]>)>,
    ) {
        let (text, encoded) = match std::str::from_utf8(message) {
            Ok(text) => (Some(text.to_owned()), None),
            Err(_) => (None, Some(base64::encode(message))),
        };
        let entry = Entry {
            t_ms: 0,
            kind: EntryKind::Message,
            text,
            base64: encoded,
            width: None,
            height: None,
            digest,
        };
        self.push(entry, received_ms, blobs);
    }

    /// Records a `bayan_render_tile` call and the digest of its pixels or its error.
    pub(crate) fn record_tile(
        &mut self,
        request: &[u8],
        width: u32,
        height: u32,
        received_ms: u64,
        digest: String,
    ) {
        let (text, encoded) = match std::str::from_utf8(request) {
            Ok(text) => (Some(text.to_owned()), None),
            Err(_) => (None, Some(base64::encode(request))),
        };
        let entry = Entry {
            t_ms: 0,
            kind: EntryKind::Tile,
            text,
            base64: encoded,
            width: Some(width),
            height: Some(height),
            digest,
        };
        self.push(entry, received_ms, Vec::new());
    }

    /// Ends the recording and returns it as JSON, with its number of entries and whether it was truncated.
    pub(crate) fn finish(self) -> Result<(Vec<u8>, u32, bool), ErrorInfo> {
        let entries = u32::try_from(self.entries.len()).unwrap_or(u32::MAX);
        let file = RecordingFile {
            format: FORMAT.to_owned(),
            format_version: FORMAT_VERSION,
            engine_version: ENGINE_VERSION.to_owned(),
            protocol_version: PROTOCOL_VERSION,
            layout_epoch: LAYOUT_EPOCH,
            initial_state: self.initial_state,
            blobs: self
                .blobs
                .iter()
                .map(|(&id, bytes)| RecordedBlob {
                    id,
                    base64: base64::encode(bytes),
                })
                .collect(),
            entries: self.entries,
            truncated: self.truncated,
        };
        let json = serde_json::to_vec(&file).map_err(|_| ErrorInfo::new(ErrorCode::Internal))?;
        if json.len() > MAX_RECORDING_BYTES {
            return Err(ErrorInfo::new(ErrorCode::LimitExceeded).with_text("limit", "recording"));
        }
        Ok((json, entries, self.truncated))
    }
}

fn invalid() -> ErrorInfo {
    ErrorInfo::new(ErrorCode::InvalidRecording)
}

fn mismatch(field: &str) -> ErrorInfo {
    ErrorInfo::new(ErrorCode::RecordingMismatch).with_text("field", field)
}

/// The bytes of an entry: its text, or its base64.
fn entry_bytes(entry: &Entry) -> Result<Vec<u8>, ErrorInfo> {
    match (&entry.text, &entry.base64) {
        (Some(text), None) => Ok(text.as_bytes().to_vec()),
        (None, Some(encoded)) => base64::decode(encoded).ok_or_else(invalid),
        _ => Err(invalid()),
    }
}

/// Replays a recording with an engine configuration and reports whether every entry produced what it produced when it was recorded (spec §6.7, §10).
///
/// # Errors
///
/// `invalid_recording` if the recording cannot be read or exceeds a limit, and `recording_mismatch` if it comes from another engine version, protocol version or layout epoch.
pub fn replay(config: Config, recording: &[u8]) -> Result<ReplayReport, ErrorInfo> {
    if recording.len() > MAX_RECORDING_BYTES {
        return Err(invalid());
    }
    let file: RecordingFile = serde_json::from_slice(recording).map_err(|_| invalid())?;
    if file.format != FORMAT || file.format_version != FORMAT_VERSION {
        return Err(invalid());
    }
    if file.engine_version != ENGINE_VERSION {
        return Err(mismatch("engine_version"));
    }
    if file.protocol_version != PROTOCOL_VERSION {
        return Err(mismatch("protocol_version"));
    }
    if file.layout_epoch != LAYOUT_EPOCH {
        return Err(mismatch("layout_epoch"));
    }
    if file.blobs.len() > MAX_BLOBS {
        return Err(invalid());
    }
    let blobs = Arc::new(BlobStore::new());
    for blob in &file.blobs {
        let bytes = base64::decode(&blob.base64).ok_or_else(invalid)?;
        if !blobs.insert_recorded(blob.id, bytes) {
            return Err(invalid());
        }
    }
    let mut engine = Engine::for_replay(config, blobs, file.initial_state).ok_or_else(invalid)?;
    let mut first_difference = None;
    for (index, entry) in file.entries.iter().enumerate() {
        let index = u32::try_from(index).map_err(|_| invalid())?;
        engine.set_replay_entry(index);
        let bytes = entry_bytes(entry)?;
        let produced = match entry.kind {
            EntryKind::Message => digest::of_messages(&engine.handle(&bytes, entry.t_ms)),
            EntryKind::Tile => {
                let (Some(width), Some(height)) = (entry.width, entry.height) else {
                    return Err(invalid());
                };
                match engine.render_tile(&bytes, width, height, entry.t_ms).pixels {
                    Ok(pixels) => digest::of_bytes(&pixels),
                    Err(error) => format!("error:{}", error.as_str()),
                }
            }
        };
        if produced != entry.digest && first_difference.is_none() {
            first_difference = Some(index);
        }
    }
    Ok(ReplayReport {
        entries: u32::try_from(file.entries.len()).map_err(|_| invalid())?,
        identical: first_difference.is_none(),
        first_difference,
        tiles: engine.take_replay_tiles(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        Engine::new(Config::default(), Arc::new(BlobStore::new()))
    }

    fn recording_of(messages: &[&str]) -> Vec<u8> {
        let mut engine = engine();
        engine.handle(
            br#"{"v":0,"id":1,"type":"hello","payload":{"protocol_versions":[0]}}"#,
            0,
        );
        engine.handle(br#"{"v":0,"id":2,"type":"diag.record.start"}"#, 0);
        for (index, message) in messages.iter().enumerate() {
            engine.handle(message.as_bytes(), u64::try_from(index).unwrap() * 10);
        }
        let reply = engine.handle(br#"{"v":0,"id":99,"type":"diag.record.stop"}"#, 0);
        let reply: serde_json::Value = serde_json::from_str(&reply[0]).unwrap();
        let blob = reply["payload"]["blob"].as_u64().unwrap();
        engine.blobs().take(blob).unwrap().to_vec()
    }

    #[test]
    fn a_recording_replays_identically() {
        let recording = recording_of(&[
            r#"{"v":0,"id":3,"type":"ui.manifest"}"#,
            r#"{"v":0,"type":"no.such.message"}"#,
        ]);
        let report = replay(Config::default(), &recording).unwrap();
        assert_eq!(report.entries, 2);
        assert!(report.identical);
        assert_eq!(report.first_difference, None);
    }

    #[test]
    fn records_times_relative_to_the_first_entry() {
        let recording = recording_of(&[r#"{"v":0,"type":"a"}"#, r#"{"v":0,"type":"b"}"#]);
        let file: RecordingFile = serde_json::from_slice(&recording).unwrap();
        let times: Vec<u64> = file.entries.iter().map(|entry| entry.t_ms).collect();
        assert_eq!(times, [0, 10]);
    }

    #[test]
    fn a_changed_recording_is_reported_where_it_first_differs() {
        let recording = recording_of(&[r#"{"v":0,"type":"a"}"#, r#"{"v":0,"type":"b"}"#]);
        let mut file: RecordingFile = serde_json::from_slice(&recording).unwrap();
        file.entries[1].digest = "fnv1a64:0000000000000000".to_owned();
        let changed = serde_json::to_vec(&file).unwrap();
        let report = replay(Config::default(), &changed).unwrap();
        assert!(!report.identical);
        assert_eq!(report.first_difference, Some(1));
    }

    #[test]
    fn refuses_recordings_it_cannot_trust() {
        let recording = recording_of(&[]);
        let file: RecordingFile = serde_json::from_slice(&recording).unwrap();
        type Change = fn(&mut RecordingFile);
        let variants: [(Change, ErrorCode); 5] = [
            (
                |file| file.format = "something else".to_owned(),
                ErrorCode::InvalidRecording,
            ),
            (
                |file| file.engine_version = "9.9.9".to_owned(),
                ErrorCode::RecordingMismatch,
            ),
            (|file| file.layout_epoch += 1, ErrorCode::RecordingMismatch),
            (
                |file| {
                    file.blobs.push(RecordedBlob {
                        id: 1,
                        base64: "not base64!".to_owned(),
                    });
                },
                ErrorCode::InvalidRecording,
            ),
            (
                |file| file.initial_state.next_seq = 0,
                ErrorCode::InvalidRecording,
            ),
        ];
        for (change, code) in variants {
            let mut changed = file.clone();
            change(&mut changed);
            let bytes = serde_json::to_vec(&changed).unwrap();
            assert_eq!(replay(Config::default(), &bytes).unwrap_err().code, code);
        }
        assert_eq!(
            replay(Config::default(), b"{").unwrap_err().code,
            ErrorCode::InvalidRecording
        );
    }

    #[test]
    fn refuses_too_many_entries_while_reading() {
        let recording = recording_of(&[]);
        let mut value: serde_json::Value = serde_json::from_slice(&recording).unwrap();
        let entry = serde_json::json!({"t_ms": 0, "kind": "message", "text": "{}", "digest": "x"});
        value["entries"] = serde_json::Value::Array(vec![entry; MAX_RECORDING_ENTRIES + 1]);
        let bytes = serde_json::to_vec(&value).unwrap();
        assert_eq!(
            replay(Config::default(), &bytes).unwrap_err().code,
            ErrorCode::InvalidRecording
        );
    }
}
