//! The font manifest, `fonts/manifest.txt`: the font files the spike accepts, each with the SHA-256 of its file (ADR-0010 §3). `fonts/fetch.sh` reads the same file.

/// The manifest, compiled in, so that every build checks fonts against the same pins.
pub const MANIFEST: &str = include_str!("../fonts/manifest.txt");

/// A font file that the manifest pins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedFont {
    /// The file name, such as `LiberationSans-Regular.ttf`.
    pub file: String,
    /// The SHA-256 of the file.
    pub sha256: [u8; 32],
    /// The size of the file in bytes.
    pub bytes: u64,
    /// Whether a copy is committed in `fonts/`, so that tests can use it without a download.
    pub committed: bool,
}

/// Reads the `font` lines of a manifest; comments, blank lines and `archive` lines are skipped.
///
/// # Errors
///
/// A message naming the line, if a line is malformed or names a file twice.
pub fn parse(text: &str) -> Result<Vec<PinnedFont>, String> {
    let mut fonts: Vec<PinnedFont> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split(' ').collect();
        match fields.first() {
            Some(&"archive") => continue,
            Some(&"font") => {}
            _ => {
                return Err(format!(
                    "manifest line {number}: not a comment, an archive or a font"
                ));
            }
        }
        let [_, file, sha256, bytes, source] = fields[..] else {
            return Err(format!(
                "manifest line {number}: a font line has five fields separated by single spaces"
            ));
        };
        if file.is_empty() || file.starts_with('.') || file.contains(['/', '\\']) {
            return Err(format!(
                "manifest line {number}: `{file}` is not a plain file name"
            ));
        }
        let sha256 = crate::hash::parse_sha256(sha256).ok_or_else(|| {
            format!("manifest line {number}: the SHA-256 is not 64 lowercase hexadecimal digits")
        })?;
        let bytes = bytes
            .parse()
            .map_err(|_| format!("manifest line {number}: the size is not a number of bytes"))?;
        if fonts.iter().any(|font| font.file == file) {
            return Err(format!("manifest line {number}: `{file}` is pinned twice"));
        }
        fonts.push(PinnedFont {
            file: file.to_owned(),
            sha256,
            bytes,
            committed: source == "committed",
        });
    }
    Ok(fonts)
}

/// The fonts that the compiled-in manifest pins.
///
/// # Errors
///
/// A message if the manifest is malformed, which a test rules out.
pub fn pinned_fonts() -> Result<Vec<PinnedFont>, String> {
    parse(MANIFEST)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::{hex, sha256};

    const DIGEST: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn reads_font_lines_and_skips_the_rest() {
        let text = format!(
            "# a comment\n\narchive a.tar.gz {DIGEST} 10 https://example.org/a.tar.gz\nfont A.ttf {DIGEST} 1234 a.tar.gz:a/A.ttf\nfont B.otf {DIGEST} 99 committed\n"
        );
        let fonts = parse(&text).unwrap();
        assert_eq!(fonts.len(), 2);
        assert_eq!(fonts[0].file, "A.ttf");
        assert_eq!(hex(&fonts[0].sha256), DIGEST);
        assert_eq!(fonts[0].bytes, 1234);
        assert!(!fonts[0].committed);
        assert_eq!(fonts[1].file, "B.otf");
        assert!(fonts[1].committed);
    }

    #[test]
    fn refuses_malformed_lines() {
        for line in [
            "font A.ttf".to_owned(),
            format!("font A.ttf {DIGEST} 12"),
            format!("font A.ttf {DIGEST} twelve committed"),
            format!("font A.ttf {} 12 committed", &DIGEST[1..]),
            format!("font A.ttf {DIGEST} 12 committed extra"),
            format!("font ../A.ttf {DIGEST} 12 committed"),
            format!("fonts A.ttf {DIGEST} 12 committed"),
            format!("font  A.ttf {DIGEST} 12 committed"),
        ] {
            let error = parse(&line).unwrap_err();
            assert!(error.contains("line 1"), "{line:?}: {error}");
        }
    }

    #[test]
    fn refuses_a_file_named_twice() {
        let text = format!("font A.ttf {DIGEST} 1 committed\nfont A.ttf {DIGEST} 1 committed\n");
        assert!(parse(&text).unwrap_err().contains("line 2"));
    }

    #[test]
    fn the_committed_manifest_pins_the_nine_fonts_of_the_probe_set() {
        let fonts = pinned_fonts().unwrap();
        let names: Vec<&str> = fonts.iter().map(|font| font.file.as_str()).collect();
        assert_eq!(
            names,
            [
                "LiberationSans-Regular.ttf",
                "LiberationSerif-Regular.ttf",
                "Carlito-Regular.ttf",
                "NotoNaskhArabic-Regular.ttf",
                "NotoSansDevanagari-Regular.ttf",
                "NotoSansHebrew-Regular.ttf",
                "NotoSansThai-Regular.ttf",
                "NotoSansSC-Regular.otf",
                "NotoSansJP-Regular.otf",
            ]
        );
    }

    #[test]
    fn the_committed_fonts_are_the_files_the_manifest_pins() {
        let committed: [(&str, &[u8]); 2] = [
            (
                "NotoSansHebrew-Regular.ttf",
                include_bytes!("../fonts/NotoSansHebrew-Regular.ttf"),
            ),
            (
                "NotoSansThai-Regular.ttf",
                include_bytes!("../fonts/NotoSansThai-Regular.ttf"),
            ),
        ];
        let fonts = pinned_fonts().unwrap();
        for (file, data) in committed {
            let pin = fonts.iter().find(|font| font.file == file).unwrap();
            assert!(pin.committed, "{file}");
            assert_eq!(sha256(data), pin.sha256, "{file}");
            assert_eq!(u64::try_from(data.len()).unwrap(), pin.bytes, "{file}");
        }
        let marked: Vec<&str> = fonts
            .iter()
            .filter(|font| font.committed)
            .map(|font| font.file.as_str())
            .collect();
        assert_eq!(marked, committed.map(|(file, _)| file));
    }
}
