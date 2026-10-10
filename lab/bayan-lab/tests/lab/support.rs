//! Builders for the tests: ZIP archives written field by field, so that a test can give one any flaw, and `.docx` packages assembled from their parts.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use bayan_lab::scan::crc32::crc32;

/// The namespace declarations of the root element of every main document part the tests build.
pub const NAMESPACES: &str = concat!(
    r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" "#,
    r#"xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" "#,
    r#"xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" "#,
    r#"xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" "#,
    r#"xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" "#,
    r#"xmlns:wpg="http://schemas.microsoft.com/office/word/2010/wordprocessingGroup" "#,
    r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" "#,
    r#"xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture" "#,
    r#"xmlns:v="urn:schemas-microsoft-com:vml" "#,
    r#"xmlns:o="urn:schemas-microsoft-com:office:office" "#,
    r#"xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" "#,
    r#"xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math""#,
);

/// Relationship types, in their Transitional spelling.
pub mod rel {
    const BASE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/";

    /// The relationship type `name` of the officeDocument family.
    pub fn of(name: &str) -> String {
        format!("{BASE}{name}")
    }

    pub const CORE_PROPERTIES: &str =
        "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";
    pub const DIGITAL_SIGNATURE: &str =
        "http://schemas.openxmlformats.org/package/2006/relationships/digital-signature/origin";
}

/// The content type of a document's main part.
pub const DOCUMENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml";

/// One entry of a ZIP archive, with every field a test may want to falsify.
#[derive(Debug, Clone)]
pub struct ZipEntry {
    /// The name in the central directory.
    pub name: Vec<u8>,
    /// The name in the local header, if it is to differ.
    pub local_name: Option<Vec<u8>>,
    /// The bytes as stored: compressed, for DEFLATE.
    pub data: Vec<u8>,
    /// The compression method in the central directory.
    pub method: u16,
    /// The compression method in the local header, if it is to differ.
    pub local_method: Option<u16>,
    /// The general-purpose flags.
    pub flags: u16,
    /// The declared CRC-32.
    pub crc32: u32,
    /// The declared compressed size.
    pub compressed_size: u32,
    /// The declared uncompressed size.
    pub uncompressed_size: u32,
    /// The "version made by" field; its high byte is the host system (3 for Unix).
    pub version_made_by: u16,
    /// The external attributes; on Unix, the high half is the file mode.
    pub external_attributes: u32,
    /// The disk on which the entry starts.
    pub disk: u16,
    /// The local header offset to declare, if not the real one.
    pub offset: Option<u32>,
    /// The extra field of the central-directory header.
    pub extra: Vec<u8>,
    /// Whether the archive holds a local header and data for the entry; without them, only the central directory lists it, at `offset`.
    pub has_local_header: bool,
}

impl ZipEntry {
    /// An entry stored without compression.
    pub fn stored(name: &str, content: &[u8]) -> Self {
        let size = u32::try_from(content.len()).unwrap();
        Self {
            name: name.as_bytes().to_vec(),
            local_name: None,
            data: content.to_vec(),
            method: 0,
            local_method: None,
            flags: 0,
            crc32: crc32(content),
            compressed_size: size,
            uncompressed_size: size,
            version_made_by: 20,
            external_attributes: 0,
            disk: 0,
            offset: None,
            extra: Vec::new(),
            has_local_header: true,
        }
    }

    /// An entry compressed with DEFLATE.
    pub fn deflated(name: &str, content: &[u8]) -> Self {
        let data = miniz_oxide::deflate::compress_to_vec(content, 6);
        Self {
            compressed_size: u32::try_from(data.len()).unwrap(),
            method: 8,
            data,
            ..Self::stored(name, content)
        }
    }
}

/// The fields of an archive's end-of-central-directory record that a test may falsify.
#[derive(Debug, Clone, Default)]
pub struct ZipLayout {
    /// The archive comment.
    pub comment: Vec<u8>,
    /// Bytes after the end record.
    pub trailing: Vec<u8>,
    /// The number of this disk.
    pub disk: u16,
    /// The disk on which the central directory starts.
    pub directory_disk: u16,
    /// The number of entries to declare, if not the real one.
    pub entries: Option<u16>,
    /// The central directory size to declare, if not the real one.
    pub directory_size: Option<u32>,
    /// The central directory offset to declare, if not the real one.
    pub directory_offset: Option<u32>,
    /// Whether to write ZIP64 end records, with the classic fields saturated.
    pub zip64: bool,
}

fn put16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn len16(bytes: &[u8]) -> u16 {
    u16::try_from(bytes.len()).unwrap()
}

/// A ZIP archive of `entries`.
pub fn zip(entries: &[ZipEntry]) -> Vec<u8> {
    zip_with(entries, &ZipLayout::default())
}

/// A ZIP archive of `entries`, with the end records as `layout` says.
pub fn zip_with(entries: &[ZipEntry], layout: &ZipLayout) -> Vec<u8> {
    let mut out = Vec::new();
    let mut offsets = Vec::new();
    for entry in entries {
        offsets.push(u32::try_from(out.len()).unwrap());
        if entry.has_local_header {
            out.extend_from_slice(&local_header(entry));
            out.extend_from_slice(&entry.data);
        }
    }
    let directory_offset = out.len();
    for (entry, offset) in entries.iter().zip(&offsets) {
        put32(&mut out, 0x0201_4b50);
        put16(&mut out, entry.version_made_by);
        put16(&mut out, 20);
        put16(&mut out, entry.flags);
        put16(&mut out, entry.method);
        put16(&mut out, 0);
        put16(&mut out, 0x21);
        put32(&mut out, entry.crc32);
        put32(&mut out, entry.compressed_size);
        put32(&mut out, entry.uncompressed_size);
        put16(&mut out, len16(&entry.name));
        put16(&mut out, len16(&entry.extra));
        put16(&mut out, 0); // comment
        put16(&mut out, entry.disk);
        put16(&mut out, 0); // internal attributes
        put32(&mut out, entry.external_attributes);
        put32(&mut out, entry.offset.unwrap_or(*offset));
        out.extend_from_slice(&entry.name);
        out.extend_from_slice(&entry.extra);
    }
    let directory_size = out.len() - directory_offset;
    let count = u16::try_from(entries.len()).unwrap();
    if layout.zip64 {
        let record = out.len();
        put32(&mut out, 0x0606_4b50);
        put64(&mut out, 44);
        put16(&mut out, 45);
        put16(&mut out, 45);
        put32(&mut out, 0);
        put32(&mut out, 0);
        put64(&mut out, u64::from(layout.entries.unwrap_or(count)));
        put64(&mut out, u64::from(layout.entries.unwrap_or(count)));
        put64(&mut out, u64::try_from(directory_size).unwrap());
        put64(&mut out, u64::try_from(directory_offset).unwrap());
        put32(&mut out, 0x0706_4b50);
        put32(&mut out, 0);
        put64(&mut out, u64::try_from(record).unwrap());
        put32(&mut out, 1);
    }
    put32(&mut out, 0x0605_4b50);
    put16(&mut out, layout.disk);
    put16(&mut out, layout.directory_disk);
    let declared = if layout.zip64 {
        u16::MAX
    } else {
        layout.entries.unwrap_or(count)
    };
    put16(&mut out, declared);
    put16(&mut out, declared);
    let (size, offset) = if layout.zip64 {
        (u32::MAX, u32::MAX)
    } else {
        (
            layout
                .directory_size
                .unwrap_or_else(|| u32::try_from(directory_size).unwrap()),
            layout
                .directory_offset
                .unwrap_or_else(|| u32::try_from(directory_offset).unwrap()),
        )
    };
    put32(&mut out, size);
    put32(&mut out, offset);
    put16(&mut out, len16(&layout.comment));
    out.extend_from_slice(&layout.comment);
    out.extend_from_slice(&layout.trailing);
    out
}

/// The local file header of `entry`, without its data.
pub fn local_header(entry: &ZipEntry) -> Vec<u8> {
    let mut out = Vec::new();
    let local_name = entry.local_name.as_ref().unwrap_or(&entry.name);
    put32(&mut out, 0x0403_4b50);
    put16(&mut out, 20);
    put16(&mut out, entry.flags);
    put16(&mut out, entry.local_method.unwrap_or(entry.method));
    put16(&mut out, 0); // time
    put16(&mut out, 0x21); // date: 1980-01-01
    put32(&mut out, entry.crc32);
    put32(&mut out, entry.compressed_size);
    put32(&mut out, entry.uncompressed_size);
    put16(&mut out, len16(local_name));
    put16(&mut out, 0);
    out.extend_from_slice(local_name);
    out
}

/// The XML of a main document part whose body holds `body`.
pub fn document_xml(body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {NAMESPACES}><w:body>{body}</w:body></w:document>"#
    )
}

/// A relationship: its type, its target, and whether the target is outside the package.
#[derive(Debug, Clone)]
struct Relationship {
    rel_type: String,
    target: String,
    external: bool,
}

/// A `.docx` package assembled from parts.
#[derive(Debug, Clone)]
pub struct Docx {
    /// The parts, by name without the leading `/`, with their content types.
    parts: Vec<(String, String, Vec<u8>)>,
    /// The relationships, by source part (`""` for the package).
    relationships: Vec<(String, Relationship)>,
}

impl Docx {
    /// A package whose main document part (`word/document.xml`) has `body` in its body.
    pub fn new(body: &str) -> Self {
        Self::with_main(DOCUMENT_TYPE, document_xml(body).as_bytes())
    }

    /// A package whose main part has this content type and content.
    pub fn with_main(content_type: &str, content: &[u8]) -> Self {
        Self {
            parts: vec![(
                "word/document.xml".to_owned(),
                content_type.to_owned(),
                content.to_vec(),
            )],
            relationships: vec![(
                String::new(),
                Relationship {
                    rel_type: rel::of("officeDocument"),
                    target: "word/document.xml".to_owned(),
                    external: false,
                },
            )],
        }
    }

    /// Adds a part related to the main document part by a relationship of type `rel_type` (a name of the officeDocument family, such as `styles`).
    #[must_use]
    pub fn part(self, name: &str, rel_type: &str, xml: &str) -> Self {
        self.part_from("word/document.xml", name, &rel::of(rel_type), xml)
    }

    /// Adds a part related to the part `source` (`""` for the package) by a relationship of type `rel_type`.
    #[must_use]
    pub fn part_from(mut self, source: &str, name: &str, rel_type: &str, xml: &str) -> Self {
        self.parts.push((
            name.to_owned(),
            "application/xml".to_owned(),
            xml.as_bytes().to_vec(),
        ));
        let target = relative_target(source, name);
        self.relationship(source, rel_type, &target, false)
    }

    /// Adds a relationship of the part `source` (`""` for the package).
    #[must_use]
    pub fn relationship(
        mut self,
        source: &str,
        rel_type: &str,
        target: &str,
        external: bool,
    ) -> Self {
        self.relationships.push((
            source.to_owned(),
            Relationship {
                rel_type: rel_type.to_owned(),
                target: target.to_owned(),
                external,
            },
        ));
        self
    }

    /// The archive's entries: the content types, then every part, then every relationships part.
    pub fn entries(&self) -> Vec<ZipEntry> {
        let mut types = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>"#,
        );
        for (name, content_type, _) in &self.parts {
            types.push_str(&format!(
                r#"<Override PartName="/{name}" ContentType="{content_type}"/>"#
            ));
        }
        types.push_str("</Types>");
        let mut entries = vec![ZipEntry::deflated("[Content_Types].xml", types.as_bytes())];
        for (name, _, content) in &self.parts {
            entries.push(ZipEntry::deflated(name, content));
        }
        let mut sources: Vec<&str> = self
            .relationships
            .iter()
            .map(|(source, _)| source.as_str())
            .collect();
        sources.sort_unstable();
        sources.dedup();
        for source in sources {
            let mut xml = String::from(
                r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
            );
            for (index, (_, relationship)) in self
                .relationships
                .iter()
                .filter(|(from, _)| from == source)
                .enumerate()
            {
                let mode = if relationship.external {
                    r#" TargetMode="External""#
                } else {
                    ""
                };
                xml.push_str(&format!(
                    r#"<Relationship Id="rId{}" Type="{}" Target="{}"{mode}/>"#,
                    index + 1,
                    relationship.rel_type,
                    relationship.target
                ));
            }
            xml.push_str("</Relationships>");
            let name = match source.rsplit_once('/') {
                Some((folder, file)) => format!("{folder}/_rels/{file}.rels"),
                None if source.is_empty() => "_rels/.rels".to_owned(),
                None => format!("_rels/{source}.rels"),
            };
            entries.push(ZipEntry::deflated(&name, xml.as_bytes()));
        }
        entries
    }

    /// The package's bytes.
    pub fn bytes(&self) -> Vec<u8> {
        zip(&self.entries())
    }
}

/// The target of a relationship from `source` to `part`, relative to the source's folder when they share it.
fn relative_target(source: &str, part: &str) -> String {
    let folder = source.rsplit_once('/').map_or("", |(folder, _)| folder);
    if folder.is_empty() {
        return part.to_owned();
    }
    match part.strip_prefix(&format!("{folder}/")) {
        Some(rest) => rest.to_owned(),
        None => format!("/{part}"),
    }
}

/// A new, empty folder for one test, below Cargo's folder for the temporary files of integration tests.
pub fn temporary_folder(name: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let number = COUNTER.fetch_add(1, Ordering::Relaxed);
    let folder = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("bayan-lab")
        .join(format!("{name}-{}-{number}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    folder
}
