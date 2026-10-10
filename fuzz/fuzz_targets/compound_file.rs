//! OLE compound files through bayan-opc's `CompoundFile` (and the cfb crate under it), on arbitrary bytes: opening, listing every entry and reading every stream either works or fails cleanly, and every stream that reads has its listed size.

#![no_main]

use bayan_opc::{CompoundEntryKind, CompoundFile, Limits};
use libfuzzer_sys::fuzz_target;

const LIMITS: Limits = Limits {
    max_entry_size: 1 << 20,
    max_compound_file_entries: 4_096,
    ..Limits::DEFAULT
};

fuzz_target!(|data: &[u8]| {
    let Ok(mut file) = CompoundFile::open(data, &LIMITS) else {
        return;
    };
    let streams: Vec<(Vec<String>, u64)> = file
        .entries()
        .iter()
        .filter(|entry| entry.kind() == CompoundEntryKind::Stream)
        .map(|entry| (entry.path().to_vec(), entry.size()))
        .collect();
    for (path, size) in streams {
        let names: Vec<&str> = path.iter().map(String::as_str).collect();
        if let Ok(data) = file.read_stream(&names) {
            assert_eq!(data.len() as u64, size);
        }
    }
});
