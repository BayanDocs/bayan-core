//! The WebAssembly interface of the spike, for the runners in `js/`: the same pipeline and results file as the native runner, with the font files handed in by the host, which also keeps the time of the measurements. wasm-bindgen generates the glue between JavaScript and the module; this file has no `unsafe` code of its own, like bayan-wasm.

use wasm_bindgen::prelude::wasm_bindgen;

use crate::bench::{VIEWPORT_HEIGHT, VIEWPORT_WIDTH, Workload, viewport};
use crate::fonts::FontBook;
use crate::hash::hex;
use crate::probes::{PROBES, Probe};
use crate::raster::{Blending, Image, Scene, Vello, render_reference, render_vello};
use crate::run::{Results, run_probe, run_viewport};

/// One probe's (or the viewport's) files, kept for the host to write out.
struct Output {
    id: String,
    layout_json: Vec<u8>,
    images: Vec<(String, Image)>,
}

/// The text pipeline, with its fonts.
#[wasm_bindgen]
#[derive(Default)]
pub struct Pipeline {
    book: FontBook,
    outputs: Vec<Output>,
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

    /// Runs the probes (only those whose fonts are committed, if `committed`) and, with every font, the viewport; keeps their files for [`Pipeline::layout_json`] and [`Pipeline::png`]; and returns the results file of the environment `environment`.
    ///
    /// # Errors
    ///
    /// Throws a message if a font is missing or a probe fails.
    pub fn run(&mut self, environment: &str, committed: bool) -> Result<String, String> {
        self.outputs.clear();
        let mut results = Vec::new();
        for probe in PROBES
            .iter()
            .filter(|probe| !committed || probe.committed())
        {
            let output = run_probe(&self.book, probe)?;
            self.outputs.push(Output {
                id: probe.id.to_owned(),
                layout_json: output.layout_json,
                images: output.images,
            });
            results.push(output.result);
        }
        let viewport = if committed {
            None
        } else {
            let probes: Vec<&'static Probe> = PROBES.iter().collect();
            let (result, images) =
                run_viewport(&self.book, &probes, VIEWPORT_WIDTH, VIEWPORT_HEIGHT)?;
            self.outputs.push(Output {
                id: "viewport".to_owned(),
                layout_json: Vec::new(),
                images,
            });
            Some(result)
        };
        let json = Results::new(environment, &self.book, results, viewport).to_json()?;
        String::from_utf8(json).map_err(|error| error.to_string())
    }

    /// The number of outputs the last [`Pipeline::run`] kept: one per probe, and the viewport last.
    #[wasm_bindgen(js_name = outputCount)]
    #[must_use]
    pub fn output_count(&self) -> usize {
        self.outputs.len()
    }

    /// The name of output `index`: a probe's, or `viewport`.
    #[wasm_bindgen(js_name = outputId)]
    #[must_use]
    pub fn output_id(&self, index: usize) -> Option<String> {
        self.outputs.get(index).map(|output| output.id.clone())
    }

    /// The layout JSON of output `index` (empty for the viewport).
    #[wasm_bindgen(js_name = layoutJson)]
    #[must_use]
    pub fn layout_json(&self, index: usize) -> Option<Vec<u8>> {
        self.outputs
            .get(index)
            .map(|output| output.layout_json.clone())
    }

    /// The names of the images of output `index`, separated by commas.
    #[wasm_bindgen(js_name = imageNames)]
    #[must_use]
    pub fn image_names(&self, index: usize) -> Option<String> {
        self.outputs.get(index).map(|output| {
            output
                .images
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<&str>>()
                .join(",")
        })
    }

    /// The image `name` of output `index` as a PNG file.
    ///
    /// # Errors
    ///
    /// Throws a message if there is no such image.
    pub fn png(&self, index: usize, name: &str) -> Result<Vec<u8>, String> {
        let output = self.outputs.get(index).ok_or("no such output")?;
        let (_, image) = output
            .images
            .iter()
            .find(|(image, _)| image == name)
            .ok_or_else(|| format!("{} has no image {name}", output.id))?;
        image.png()
    }

    /// Frees the outputs of the last run.
    #[wasm_bindgen(js_name = clearOutputs)]
    pub fn clear_outputs(&mut self) {
        self.outputs.clear();
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
