//! The WebAssembly interface of the spike, for the runners in `js/`: the same pipeline and results file as the native runner, with the font files handed in by the host, which also keeps the time of the measurements. wasm-bindgen generates the glue between JavaScript and the module; this file has no `unsafe` code of its own, like bayan-wasm.

use wasm_bindgen::prelude::wasm_bindgen;

use crate::bench::{VIEWPORT_HEIGHT, VIEWPORT_WIDTH, Workload, viewport};
use crate::fonts::FontBook;
use crate::hash::hex;
use crate::probes::{PROBES, Probe};
use crate::raster::{Blending, Image, Scene, Vello, render_reference, render_vello};
use crate::run::{ProbeResult, Results, ViewportResult, run_probe, run_viewport};

/// The files of the output that [`Pipeline::step`] made last, a probe's or the viewport's, kept until the host has fetched them or the next step replaces them.
#[derive(Default)]
struct Output {
    layout_json: Vec<u8>,
    /// Each image as a PNG file, encoded as soon as it was drawn, so that no raw image outlives its encoding (`crate::run::Keep`).
    pngs: Vec<(String, Vec<u8>)>,
}

/// A run that [`Pipeline::start_run`] started.
struct Run {
    environment: String,
    /// The probes still to run, in reverse order.
    probes: Vec<&'static Probe>,
    /// Whether the viewport is still to run, after the probes.
    viewport: bool,
    results: Vec<ProbeResult>,
    viewport_result: Option<ViewportResult>,
}

/// The text pipeline, with its fonts.
#[wasm_bindgen]
#[derive(Default)]
pub struct Pipeline {
    book: FontBook,
    run: Option<Run>,
    output: Output,
    bench: Option<(Workload, Scene)>,
}

#[wasm_bindgen]
impl Pipeline {
    /// A pipeline without fonts.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> Pipeline {
        Pipeline::default()
    }

    /// The fonts the manifest pins, as a JSON array of `{"file", "committed"}` objects, in the manifest's order.
    ///
    /// # Errors
    ///
    /// Throws a message if the compiled-in manifest is malformed, which a test rules out.
    #[wasm_bindgen(js_name = pinnedFonts)]
    pub fn pinned_fonts() -> Result<String, String> {
        let fonts: Vec<serde_json::Value> = crate::manifest::pinned_fonts()?
            .into_iter()
            .map(|pin| serde_json::json!({ "file": pin.file, "committed": pin.committed }))
            .collect();
        serde_json::to_string(&fonts).map_err(|error| error.to_string())
    }

    /// Adds the font file `file`, which must be the one the manifest pins.
    ///
    /// # Errors
    ///
    /// Throws a message if the file is not pinned, was added already, or is not the pinned file.
    #[wasm_bindgen(js_name = addFont)]
    pub fn add_font(&mut self, file: &str, data: Vec<u8>) -> Result<(), String> {
        self.book.add(file, data).map(|_| ())
    }

    /// Starts a run of the probes (only those whose fonts are committed, if `committed`) and, with every font, the viewport, for the results file of the environment `environment`. [`Pipeline::step`] then makes their outputs one at a time, as the native runner does, and [`Pipeline::results`] returns the results file.
    #[wasm_bindgen(js_name = startRun)]
    pub fn start_run(&mut self, environment: &str, committed: bool) {
        self.output = Output::default();
        self.run = Some(Run {
            environment: environment.to_owned(),
            probes: PROBES
                .iter()
                .filter(|probe| !committed || probe.committed())
                .rev()
                .collect(),
            viewport: !committed,
            results: Vec::new(),
            viewport_result: None,
        });
    }

    /// Makes the next output of the run, the next probe's or, after the last probe, the viewport's, and keeps its files for [`Pipeline::layout_json`] and [`Pipeline::take_png`] in place of the previous output's. Returns the output's name (a probe's, or `viewport`), or nothing once every output is made.
    ///
    /// # Errors
    ///
    /// Throws a message if no run was started, a font is missing, or a probe fails.
    pub fn step(&mut self) -> Result<Option<String>, String> {
        let run = self.run.as_mut().ok_or("call startRun first")?;
        // The previous output is freed before the next one is drawn.
        self.output = Output::default();
        let mut pngs = Vec::new();
        let mut keep = |name: &str, image: &Image| {
            pngs.push((name.to_owned(), image.png()?));
            Ok(())
        };
        let id = if let Some(probe) = run.probes.pop() {
            let output = run_probe(&self.book, probe, &mut keep)?;
            run.results.push(output.result);
            self.output.layout_json = output.layout_json;
            probe.id
        } else if run.viewport {
            run.viewport = false;
            let probes: Vec<&'static Probe> = PROBES.iter().collect();
            run.viewport_result = Some(run_viewport(
                &self.book,
                &probes,
                VIEWPORT_WIDTH,
                VIEWPORT_HEIGHT,
                &mut keep,
            )?);
            "viewport"
        } else {
            return Ok(None);
        };
        self.output.pngs = pngs;
        Ok(Some(id.to_owned()))
    }

    /// The results file of the run, once [`Pipeline::step`] has made every output; it ends the run.
    ///
    /// # Errors
    ///
    /// Throws a message if no run was started, or outputs are still to be made.
    pub fn results(&mut self) -> Result<String, String> {
        let run = self.run.as_ref().ok_or("call startRun first")?;
        if !run.probes.is_empty() || run.viewport {
            return Err("the run is not complete: call step until it returns nothing".to_owned());
        }
        let run = self.run.take().ok_or("call startRun first")?;
        self.output = Output::default();
        let json = Results::new(
            &run.environment,
            &self.book,
            run.results,
            run.viewport_result,
        )
        .to_json()?;
        String::from_utf8(json).map_err(|error| error.to_string())
    }

    /// The layout JSON of the last output (empty for the viewport).
    #[wasm_bindgen(js_name = layoutJson)]
    #[must_use]
    pub fn layout_json(&self) -> Vec<u8> {
        self.output.layout_json.clone()
    }

    /// The names of the images of the last output that have not been taken yet, separated by commas.
    #[wasm_bindgen(js_name = imageNames)]
    #[must_use]
    pub fn image_names(&self) -> String {
        self.output
            .pngs
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<&str>>()
            .join(",")
    }

    /// The image `name` of the last output as a PNG file, handed over once: the module keeps no copy.
    ///
    /// # Errors
    ///
    /// Throws a message if the last output has no such image, or it was taken already.
    #[wasm_bindgen(js_name = takePng)]
    pub fn take_png(&mut self, name: &str) -> Result<Vec<u8>, String> {
        let index = self
            .output
            .pngs
            .iter()
            .position(|(image, _)| image == name)
            .ok_or_else(|| format!("the last output has no image {name} left"))?;
        Ok(self.output.pngs.remove(index).1)
    }

    /// Prepares the measurements with every probe (every font must have been added) and returns the number of characters one round of [`Pipeline::shape_round`] or [`Pipeline::layout_round`] processes.
    ///
    /// # Errors
    ///
    /// Throws a message if a font is missing.
    #[wasm_bindgen(js_name = prepareBench)]
    pub fn prepare_bench(&mut self) -> Result<usize, String> {
        let probes: Vec<&'static Probe> = PROBES.iter().collect();
        let workload = Workload::new(&self.book, &probes)?;
        let (scene, _) = viewport(&self.book, &probes, VIEWPORT_WIDTH, VIEWPORT_HEIGHT)?;
        let characters = workload.characters();
        self.bench = Some((workload, scene));
        Ok(characters)
    }

    /// Shapes every probe once and returns the number of glyphs.
    ///
    /// # Errors
    ///
    /// Throws a message if [`Pipeline::prepare_bench`] was not called, or shaping fails.
    #[wasm_bindgen(js_name = shapeRound)]
    pub fn shape_round(&self) -> Result<usize, String> {
        let (workload, _) = self.bench.as_ref().ok_or("call prepareBench first")?;
        workload.shape(&self.book)
    }

    /// Lays out every probe once and returns the number of lines.
    ///
    /// # Errors
    ///
    /// Throws a message if [`Pipeline::prepare_bench`] was not called, or a layout fails.
    #[wasm_bindgen(js_name = layoutRound)]
    pub fn layout_round(&self) -> Result<usize, String> {
        let (workload, _) = self.bench.as_ref().ok_or("call prepareBench first")?;
        workload.lay_out(&self.book)
    }

    /// Renders the viewport with `rasterizer` (`reference`, `gamma2`, or a vello_cpu configuration such as `vello_u8_detected`) and returns its raster hash.
    ///
    /// # Errors
    ///
    /// Throws a message if [`Pipeline::prepare_bench`] was not called, or the rasterizer is unknown.
    #[wasm_bindgen(js_name = renderViewport)]
    pub fn render_viewport(&self, rasterizer: &str) -> Result<String, String> {
        let (_, scene) = self.bench.as_ref().ok_or("call prepareBench first")?;
        let image = match rasterizer {
            "reference" => render_reference(scene, Blending::Linear)?,
            "gamma2" => render_reference(scene, Blending::Gamma2)?,
            name => {
                let configuration = Vello::ALL
                    .into_iter()
                    .find(|configuration| configuration.name() == name)
                    .ok_or_else(|| format!("no rasterizer {name}"))?;
                render_vello(scene, configuration)?.0
            }
        };
        Ok(hex(&image.sha256()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::tests::committed_book;
    use crate::run::run_probe;

    fn pipeline() -> Pipeline {
        Pipeline {
            book: committed_book(),
            ..Pipeline::default()
        }
    }

    #[test]
    fn hands_the_outputs_of_a_run_over_one_at_a_time() {
        let mut pipeline = pipeline();
        pipeline.start_run("test", true);
        let mut ids = Vec::new();
        while let Some(id) = pipeline.step().unwrap() {
            assert_eq!(
                pipeline.image_names(),
                "reference,gamma2,vello_u8_detected,vello_u8_fallback,vello_f32_fallback"
            );
            let png = pipeline.take_png("reference").unwrap();
            assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"), "{id}");
            // Handed over once: the module keeps no copy.
            assert!(pipeline.take_png("reference").is_err());
            assert!(pipeline.image_names().starts_with("gamma2,"));
            assert!(!pipeline.layout_json().is_empty());
            ids.push(id);
        }
        let committed: Vec<&'static Probe> =
            PROBES.iter().filter(|probe| probe.committed()).collect();
        let committed_ids: Vec<&str> = committed.iter().map(|probe| probe.id).collect();
        assert_eq!(ids, committed_ids);
        // The same results file as the native runner's.
        let book = committed_book();
        let probes = committed
            .iter()
            .map(|probe| run_probe(&book, probe, &mut |_, _| Ok(())).unwrap().result)
            .collect();
        let expected = Results::new("test", &book, probes, None).to_json().unwrap();
        assert_eq!(pipeline.results().unwrap().as_bytes(), expected);
        // The run is over: the next one starts with startRun.
        assert!(pipeline.step().is_err());
        assert!(pipeline.results().is_err());
    }

    #[test]
    fn gives_the_results_only_when_the_run_is_complete() {
        let mut pipeline = pipeline();
        assert!(pipeline.results().is_err());
        pipeline.start_run("test", true);
        assert!(pipeline.step().unwrap().is_some());
        assert!(pipeline.results().is_err());
        // Still running: the remaining probes follow.
        assert!(pipeline.step().unwrap().is_some());
    }
}
