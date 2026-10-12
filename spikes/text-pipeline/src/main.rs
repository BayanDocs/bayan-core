//! `text-pipeline`, the native runner of the CORE-003 spike.
//!
//! ```text
//! text-pipeline run --fonts <folder> --out <folder> --environment <name> [--committed]
//! text-pipeline bench --fonts <folder> --out <file> --environment <name>
//! ```
//!
//! `run` lays out and rasterizes every probe with the fonts in `--fonts` (which `fonts/fetch.sh` fills), and writes into `--out` the results file `results.json`, each probe's layout JSON (`layout/<probe>.json`) and its images (`png/<probe>-<rendering>.png`). With `--committed` it runs only the probes whose fonts are committed. `bench` measures shaping, rasterization and memory and writes them to `--out` as JSON. The WebAssembly runners in `js/` do the same through the module's `Pipeline`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use text_pipeline::bench::{VIEWPORT_HEIGHT, VIEWPORT_WIDTH, Workload, viewport};
use text_pipeline::fonts::FontBook;
use text_pipeline::hash::hex;
use text_pipeline::manifest::pinned_fonts;
use text_pipeline::probes::{PROBES, Probe};
use text_pipeline::raster::{Blending, Vello, render_reference, render_vello};
use text_pipeline::run::{Results, run_probe, run_viewport};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("run") => Options::parse(&args[1..]).and_then(|options| run(&options)),
        Some("bench") => Options::parse(&args[1..]).and_then(|options| bench(&options)),
        _ => Err("usage: text-pipeline run|bench --fonts <folder> --out <path> --environment <name> [--committed]".to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(problem) => {
            // A failure to report the failure leaves only the exit code.
            let _ = writeln!(std::io::stderr().lock(), "text-pipeline: {problem}");
            ExitCode::FAILURE
        }
    }
}

/// The command-line options.
struct Options {
    fonts: PathBuf,
    out: PathBuf,
    environment: String,
    committed: bool,
}

impl Options {
    fn parse(args: &[String]) -> Result<Options, String> {
        let mut fonts = None;
        let mut out = None;
        let mut environment = None;
        let mut committed = false;
        let mut rest = args.iter();
        while let Some(arg) = rest.next() {
            let mut value = || {
                rest.next()
                    .cloned()
                    .ok_or_else(|| format!("{arg} needs a value"))
            };
            match arg.as_str() {
                "--fonts" => fonts = Some(PathBuf::from(value()?)),
                "--out" => out = Some(PathBuf::from(value()?)),
                "--environment" => environment = Some(value()?),
                "--committed" => committed = true,
                other => return Err(format!("unknown option {other}")),
            }
        }
        Ok(Options {
            fonts: fonts.ok_or("--fonts is missing")?,
            out: out.ok_or("--out is missing")?,
            environment: environment.ok_or("--environment is missing")?,
            committed,
        })
    }
}

/// Loads the pinned fonts from `folder` (only the committed ones if `committed`); [`FontBook::add`] checks each against its SHA-256.
fn load_fonts(folder: &Path, committed: bool) -> Result<FontBook, String> {
    let mut book = FontBook::new();
    for pin in pinned_fonts()? {
        if committed && !pin.committed {
            continue;
        }
        let path = folder.join(&pin.file);
        let data = std::fs::read(&path).map_err(|error| {
            format!(
                "cannot read {}: {error} (run spikes/text-pipeline/fonts/fetch.sh)",
                path.display()
            )
        })?;
        book.add(&pin.file, data)?;
    }
    Ok(book)
}

fn write(path: &Path, data: &[u8]) -> Result<(), String> {
    std::fs::write(path, data).map_err(|error| format!("cannot write {}: {error}", path.display()))
}

fn run(options: &Options) -> Result<(), String> {
    let book = load_fonts(&options.fonts, options.committed)?;
    let layouts = options.out.join("layout");
    let pngs = options.out.join("png");
    for folder in [&layouts, &pngs] {
        std::fs::create_dir_all(folder)
            .map_err(|error| format!("cannot create {}: {error}", folder.display()))?;
    }
    let mut stdout = std::io::stdout().lock();
    let mut results = Vec::new();
    for probe in PROBES
        .iter()
        .filter(|probe| !options.committed || probe.committed())
    {
        let output = run_probe(&book, probe, &mut |name, image| {
            write(
                &pngs.join(format!("{}-{name}.png", probe.id)),
                &image.png()?,
            )
        })?;
        write(
            &layouts.join(format!("{}.json", probe.id)),
            &output.layout_json,
        )?;
        let result = output.result;
        writeln!(
            stdout,
            "{:<28} layout {}  reference {}  {} lines, {} glyphs{}",
            result.id,
            &result.layout_sha256[..16],
            &result.reference_sha256[..16],
            result.lines,
            result.glyphs,
            if result.missing_glyphs > 0 {
                format!(", {} MISSING", result.missing_glyphs)
            } else {
                String::new()
            }
        )
        .map_err(|error| error.to_string())?;
        results.push(result);
    }
    // The viewport needs every font, so a run of the committed probes leaves it out.
    let viewport = if options.committed {
        None
    } else {
        let probes: Vec<&'static Probe> = PROBES.iter().collect();
        let viewport = run_viewport(
            &book,
            &probes,
            VIEWPORT_WIDTH,
            VIEWPORT_HEIGHT,
            &mut |name, image| write(&pngs.join(format!("viewport-{name}.png")), &image.png()?),
        )?;
        writeln!(
            stdout,
            "{:<28} layouts {}  reference {}  {} paragraphs, {} glyphs",
            "viewport",
            &viewport.layouts_sha256[..16],
            &viewport.reference_sha256[..16],
            viewport.paragraphs,
            viewport.glyphs
        )
        .map_err(|error| error.to_string())?;
        Some(viewport)
    };
    let results = Results::new(&options.environment, &book, results, viewport);
    write(&options.out.join("results.json"), &results.to_json()?)?;
    writeln!(
        stdout,
        "{}: layouts {}  references {}",
        options.environment, results.layouts_sha256, results.references_sha256
    )
    .map_err(|error| error.to_string())
}

/// The rasterizers the viewport measurement times, as the WebAssembly runners name them (js/harness.mjs).
const RASTERIZERS: [&str; 5] = [
    "reference",
    "gamma2",
    "vello_u8_detected",
    "vello_u8_fallback",
    "vello_f32_fallback",
];

/// Runs `round` until at least two seconds have passed (after one round to warm up), as js/harness.mjs does, and returns the rounds and the milliseconds.
fn rounds(mut round: impl FnMut() -> Result<usize, String>) -> Result<(u32, f64), String> {
    round()?;
    let mut count = 0_u32;
    let start = Instant::now();
    while count < 3 || start.elapsed() < Duration::from_secs(2) {
        round()?;
        count += 1;
    }
    Ok((count, start.elapsed().as_secs_f64() * 1000.0))
}

/// The process's peak resident memory in bytes, which Linux reports as VmHWM in /proc/self/status; `None` elsewhere.
fn peak_memory() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmHWM:"))?;
    let kilobytes: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    kilobytes.checked_mul(1024)
}

fn bench(options: &Options) -> Result<(), String> {
    use serde_json::json;

    let book = load_fonts(&options.fonts, false)?;
    let mut memory = serde_json::Map::new();
    memory.insert("fonts".to_owned(), json!(peak_memory()));
    let probes: Vec<&'static Probe> = PROBES.iter().collect();
    let workload = Workload::new(&book, &probes)?;
    let (scene, _) = viewport(&book, &probes, VIEWPORT_WIDTH, VIEWPORT_HEIGHT)?;
    let characters = workload.characters();
    let (shaping_rounds, shaping_ms) = rounds(|| workload.shape(&book))?;
    let (layout_rounds, layout_ms) = rounds(|| workload.lay_out(&book))?;
    memory.insert("layout".to_owned(), json!(peak_memory()));
    let mut rasterizers = serde_json::Map::new();
    for name in RASTERIZERS {
        let mut runs = Vec::with_capacity(5);
        let mut sha256 = String::new();
        for _ in 0..5 {
            let start = Instant::now();
            let image = match name {
                "reference" => render_reference(&scene, Blending::Linear)?,
                "gamma2" => render_reference(&scene, Blending::Gamma2)?,
                _ => {
                    let configuration = Vello::ALL
                        .into_iter()
                        .find(|configuration| configuration.name() == name)
                        .ok_or_else(|| format!("no rasterizer {name}"))?;
                    render_vello(&scene, configuration)?.0
                }
            };
            runs.push(start.elapsed().as_secs_f64() * 1000.0);
            sha256 = hex(&image.sha256());
        }
        let mut sorted = runs.clone();
        sorted.sort_by(f64::total_cmp);
        rasterizers.insert(
            name.to_owned(),
            json!({ "median_ms": sorted[sorted.len() / 2], "runs_ms": runs, "sha256": sha256 }),
        );
        memory.insert(name.to_owned(), json!(peak_memory()));
    }
    let per_second = |rounds: u32, ms: f64| {
        let processed =
            f64::from(u32::try_from(characters).unwrap_or(u32::MAX)) * f64::from(rounds);
        processed * 1000.0 / ms
    };
    let bench = json!({
        "format": "bayan-core CORE-003 text-pipeline measurements v1",
        "environment": options.environment,
        "characters": characters,
        "shaping": { "rounds": shaping_rounds, "ms": shaping_ms, "characters_per_second": per_second(shaping_rounds, shaping_ms) },
        "layout": { "rounds": layout_rounds, "ms": layout_ms, "characters_per_second": per_second(layout_rounds, layout_ms) },
        "viewport": { "rasterizers": rasterizers },
        "memory_bytes": memory,
    });
    let mut text = serde_json::to_vec_pretty(&bench).map_err(|error| error.to_string())?;
    text.push(b'\n');
    if let Some(folder) = options.out.parent() {
        std::fs::create_dir_all(folder)
            .map_err(|error| format!("cannot create {}: {error}", folder.display()))?;
    }
    write(&options.out, &text)?;
    writeln!(
        std::io::stdout().lock(),
        "{}: shaping {:.0} characters per second, layout {:.0}, viewport reference {:.1} ms",
        options.environment,
        per_second(shaping_rounds, shaping_ms),
        per_second(layout_rounds, layout_ms),
        bench["viewport"]["rasterizers"]["reference"]["median_ms"]
            .as_f64()
            .unwrap_or(0.0)
    )
    .map_err(|error| error.to_string())
}
