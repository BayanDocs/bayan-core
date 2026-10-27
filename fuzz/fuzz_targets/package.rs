//! Opening a whole package (bayan-opc's `Package`) from arbitrary bytes: the ZIP container, part names, the content types stream and every relationships part. For every package it accepts, every part reads or fails cleanly, relationships resolve or fail cleanly, and writing the package back unchanged gives a package with the same parts and byte-identical stored data.

#![no_main]

use bayan_opc::{Limits, Package, PackageWriter, RelationshipSource};
use libfuzzer_sys::fuzz_target;

const LIMITS: Limits = Limits {
    max_entries: 256,
    max_entry_size: 1 << 20,
    max_total_size: 4 << 20,
    max_metadata_size: 256 << 10,
    ..Limits::DEFAULT
};

fuzz_target!(|data: &[u8]| {
    let Ok(package) = Package::open(data, &LIMITS) else {
        return;
    };
    let mut readable = true;
    for part in package.parts() {
        let _ = package.content_type(part);
        readable &= package.read_part(part).is_ok();
        let source = RelationshipSource::Part(part.clone());
        for relationships in [
            package.relationships(&source),
            package.relationships(&RelationshipSource::Package),
        ]
        .into_iter()
        .flatten()
        {
            for relationship in relationships.iter() {
                let _ = relationship.resolve(&source);
            }
        }
    }
    let _ = package.core_properties();
    // Writing verifies every part before copying it, so it fails exactly when a part cannot be read.
    let written = PackageWriter::from_package(&package).finish();
    assert_eq!(written.is_ok(), readable);
    let Ok(written) = written else {
        return;
    };
    // The writer does not apply the reader's limits (`PackageWriter` in crates/bayan-opc/src/package.rs), so the copy is read back with each limit that writing a package unchanged can exceed, raised by exactly what the writer can add; BayanDocs/bayan-core#27 makes the writer keep within the reader's limits and brings back the reading with LIMITS here. The copy leaves out entries that are not parts, comments and padding, so it can be smaller and more compressed overall: the ratio is not checked. It writes every byte of a part name beyond ASCII percent-encoded, as three, so an entry name can be three times as long. And it gives a content types stream without a default for the `rels` extension one, indented like the stream's first entry: up to the stream's size again, plus under 1 KiB for the element itself (with the root element's prefix, in UTF-16 at worst), in four XML nodes (the indentation, the element and its two attributes).
    let added = LIMITS.max_metadata_size + 1024;
    let relaxed = Limits {
        max_compression_ratio: u64::MAX,
        max_name_length: 3 * LIMITS.max_name_length,
        max_total_size: LIMITS.max_total_size + added as u64,
        max_metadata_size: LIMITS.max_metadata_size + added,
        max_metadata_total_size: LIMITS.max_metadata_total_size + added,
        max_xml_nodes: LIMITS.max_xml_nodes + 4,
        ..LIMITS
    };
    let reopened = Package::open(&written, &relaxed).unwrap();
    let before: Vec<_> = package.parts().collect();
    let after: Vec<_> = reopened.parts().collect();
    assert_eq!(before, after);
    for part in package.parts() {
        let original = package
            .zip()
            .raw_data(package.entry_of(part).unwrap())
            .unwrap();
        let copy = reopened
            .zip()
            .raw_data(reopened.entry_of(part).unwrap())
            .unwrap();
        assert_eq!(original, copy);
    }
});
