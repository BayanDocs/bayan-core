//! The determinism check of work package CORE-003 that the verification gate runs: the probes whose fonts are committed (Noto Sans Hebrew and Noto Sans Thai, in `fonts/`) give exactly the layout hashes and reference raster hashes of `results/expected.json`.
//!
//! `cargo xtask verify` runs this test on the host and, under Node.js, in WebAssembly, and its `determinism` step shows the hashes of both runs; CI runs the gate on Linux x86-64, Windows x86-64 and macOS arm64, so every pull request checks these probes on four platforms. The whole probe set needs fonts that are too large to commit, so `.github/workflows/text-pipeline.yml` runs it, in eleven environments, when the spike or what it depends on changes, and compares it with the same expectation. A deliberate change of the output updates `results/expected.json` (`node js/compare.mjs --expect <results.json>`), and its pull request says why (ADR-0004 §4, layout epochs).

use text_pipeline::fonts::FontBook;
use text_pipeline::hash::{hex, sha256};
use text_pipeline::probes::{PROBES, Probe};
use text_pipeline::raster::{Blending, render_reference, scene};
use text_pipeline::run::lay_out_probe;

/// The committed fonts, compiled into the test, which reads no files: tests in WebAssembly cannot.
const FONTS: [(&str, &[u8]); 2] = [
    (
        "NotoSansHebrew-Regular.ttf",
        include_bytes!("../fonts/NotoSansHebrew-Regular.ttf"),
    ),
    (
        "NotoSansThai-Regular.ttf",
        include_bytes!("../fonts/NotoSansThai-Regular.ttf"),
    ),
];

/// The expectation that the workflow compares every environment with.
const EXPECTED: &str = include_str!("../results/expected.json");

#[test]
fn the_probes_with_committed_fonts_give_the_expected_hashes() {
    let expectation: serde_json::Value = serde_json::from_str(EXPECTED).unwrap();
    // The hash `name` (`layout_sha256` or `reference_sha256`) that the expectation gives the probe `id`.
    let expected = |id: &str, name: &str| {
        expectation["probes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|probe| probe["id"] == id)
            .unwrap_or_else(|| panic!("results/expected.json has no probe {id}"))[name]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let mut book = FontBook::new();
    for (file, data) in FONTS {
        book.add(file, data.to_vec()).unwrap();
    }
    let probes: Vec<&Probe> = PROBES.iter().filter(|probe| probe.committed()).collect();
    assert_eq!(
        probes.iter().map(|probe| probe.id).collect::<Vec<&str>>(),
        ["hebrew-points", "thai-dictionary", "thai-marks"]
    );
    let mut differences = Vec::new();
    for probe in probes {
        let layout = lay_out_probe(&book, probe).unwrap();
        let layout_sha256 = hex(&sha256(&layout.to_json().unwrap()));
        let image = render_reference(&scene(&book, &layout).unwrap(), Blending::Linear).unwrap();
        let reference_sha256 = hex(&image.sha256());
        // `cargo xtask verify` shows these lines from the host and from WebAssembly.
        println!(
            "{}: layout {layout_sha256}, reference raster {reference_sha256}",
            probe.id
        );
        let layout_expected = expected(probe.id, "layout_sha256");
        let reference_expected = expected(probe.id, "reference_sha256");
        if layout_sha256 != layout_expected {
            differences.push(format!(
                "{}: layout {layout_sha256}, expected {layout_expected}",
                probe.id
            ));
        }
        if reference_sha256 != reference_expected {
            differences.push(format!(
                "{}: reference raster {reference_sha256}, expected {reference_expected}",
                probe.id
            ));
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}
