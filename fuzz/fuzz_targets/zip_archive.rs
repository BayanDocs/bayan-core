//! The ZIP reader (bayan-opc's `ZipArchive`) on arbitrary bytes. Besides never panicking, it must hold to these properties for every archive it accepts: decompressing an entry into memory and verifying it through a window give the same answer, and an entry copied unchanged into a new archive reads back the same.

#![no_main]

use bayan_opc::Limits;
use bayan_opc::zip::{ZipArchive, ZipWriter};
use libfuzzer_sys::fuzz_target;

/// Smaller limits than the defaults, so that every run is quick; the code paths are the same.
const LIMITS: Limits = Limits {
    max_entries: 256,
    max_entry_size: 1 << 20,
    max_total_size: 4 << 20,
    max_metadata_size: 256 << 10,
    ..Limits::DEFAULT
};

fuzz_target!(|data: &[u8]| {
    let Ok(archive) = ZipArchive::new(data, &LIMITS) else {
        return;
    };
    let mut writer = ZipWriter::new();
    for (index, entry) in archive.entries().iter().enumerate() {
        let read = archive.read(index);
        let verified = archive.verify(index);
        assert_eq!(read.as_ref().err(), verified.as_ref().err());
        if let Ok(content) = &read {
            assert_eq!(content.len() as u64, entry.uncompressed_size());
            if !entry.is_folder() {
                let raw = archive.raw_entry(index).unwrap();
                writer.add_raw(entry.name(), &raw).unwrap();
            }
        }
    }
    let copy = writer.finish().unwrap();
    // The copy leaves out what the original may have had besides its entries (comments, padding, extra fields), so it can be smaller and more compressed overall; the ratio is not what is checked here.
    let relaxed = Limits {
        max_compression_ratio: u64::MAX,
        ..LIMITS
    };
    let copied = ZipArchive::new(&copy, &relaxed).unwrap();
    for (index, entry) in copied.entries().iter().enumerate() {
        let original = archive
            .entries()
            .iter()
            .position(|original| original.name() == entry.name())
            .unwrap();
        assert_eq!(
            copied.raw_data(index).unwrap(),
            archive.raw_data(original).unwrap()
        );
        assert_eq!(copied.read(index).unwrap(), archive.read(original).unwrap());
    }
});
