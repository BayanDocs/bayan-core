//! The committed C header, `include/bayan_ffi.h`, is what cbindgen generates from `src/lib.rs`, and it declares exactly the functions of engine protocol §3.1.

#![cfg(test)]

use std::path::PathBuf;

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn generated_header() -> String {
    let config = cbindgen::Config::from_file(crate_dir().join("cbindgen.toml")).unwrap();
    let bindings = cbindgen::Builder::new()
        .with_config(config)
        .with_src(crate_dir().join("src").join("lib.rs"))
        .generate()
        .unwrap();
    let mut header = Vec::new();
    bindings.write(&mut header);
    String::from_utf8(header).unwrap()
}

#[test]
fn the_committed_header_is_current() {
    let path = crate_dir().join("include").join("bayan_ffi.h");
    let generated = generated_header();
    if std::env::var_os("BAYAN_UPDATE_GENERATED").is_some() {
        std::fs::create_dir_all(crate_dir().join("include")).unwrap();
        std::fs::write(&path, &generated).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        committed == generated,
        "{} is out of date with src/lib.rs. Regenerate it with `BAYAN_UPDATE_GENERATED=1 cargo test -p bayan-ffi --test header` and review the difference.",
        path.display()
    );
}

#[test]
fn the_header_declares_exactly_the_functions_of_the_specification() {
    let header = generated_header();
    let declared: Vec<&str> = header
        .lines()
        .filter(|line| {
            !line.starts_with(' ')
                && !line.starts_with('*')
                && !line.starts_with('/')
                && line.contains("bayan_")
                && line.contains('(')
        })
        .filter_map(|line| {
            let name_end = line.find('(')?;
            line[..name_end].rsplit([' ', '*']).next()
        })
        .collect();
    assert_eq!(
        declared,
        [
            "bayan_version",
            "bayan_engine_new",
            "bayan_engine_free",
            "bayan_engine_set_callback",
            "bayan_engine_post",
            "bayan_blob_put",
            "bayan_blob_get",
            "bayan_blob_release",
            "bayan_render_tile",
        ]
    );
    for status in [
        "#define BAYAN_STATUS_OK 0",
        "#define BAYAN_STATUS_INVALID_ARGUMENT 1",
        "#define BAYAN_STATUS_NOT_FOUND 2",
        "#define BAYAN_STATUS_BUFFER_TOO_SMALL 3",
        "#define BAYAN_STATUS_INTERNAL_ERROR 4",
        "#define BAYAN_STATUS_WRONG_THREAD 5",
        "typedef struct BayanEngine BayanEngine;",
        "typedef uint64_t BayanBlobId;",
        "typedef int32_t BayanStatus;",
        "typedef void (*BayanMessageCallback)(void *user_data, const uint8_t *json, size_t json_len);",
    ] {
        assert!(header.contains(status), "the header lacks `{status}`");
    }
}
