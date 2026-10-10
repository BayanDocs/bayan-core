//! The rules that turn the elements of a WordprocessingML package into coverage-matrix features.
//!
//! [`PartTagger`] receives the elements of one part from the XML scanner and records what it finds in [`Collected`], which gathers the findings of all parts of a document. Each rule names the elements (and, where needed, the parent element or an attribute value) that reveal a feature; `lab/corpus/tagging-rules.md` lists them in prose.
//!
//! Markup Compatibility is applied as Word applies it (ECMA-376 Part 3 §10): of an `mc:AlternateContent`, only the first `mc:Choice` whose required namespaces the tagger knows, or else the `mc:Fallback`, counts, so the VML fallback that Word writes next to every modern shape is not mistaken for a legacy shape, and its text is not counted twice. Elements in namespaces the tagger does not know are counted as unknown extensions and skipped with their content.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use super::features::Feature;
use super::scripts::{Script, script_of};
use super::vocabulary::{Ns, namespace};
use super::xml::{Handler, Name, Start, Text, Value};

/// The most font names recorded for one document.
pub const MAX_FONTS: usize = 200;
/// The most language tags recorded for one document.
pub const MAX_LANGUAGES: usize = 100;
/// The longest font name recorded, in characters.
const MAX_FONT_NAME: usize = 64;
/// The most nested fields followed.
const MAX_FIELD_DEPTH: usize = 64;
/// The longest field instruction kept for classification, in bytes; only its start matters.
const MAX_INSTRUCTION: usize = 512;
/// The longest application name or page count kept from the application properties, in bytes.
const MAX_PROPERTY_TEXT: usize = 128;
/// The most prefixes an `mc:Choice` may require to be chosen; real documents require one or two.
const MAX_REQUIRED_PREFIXES: usize = 16;

/// The kind of part being scanned, which decides what its elements mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PartKind {
    /// The main document part.
    Document,
    /// A header.
    Header,
    /// A footer.
    Footer,
    /// Footnotes.
    Footnotes,
    /// Endnotes.
    Endnotes,
    /// Comments.
    Comments,
    /// Word's comment extensions and the people part.
    CommentExtensions,
    /// Styles.
    Styles,
    /// Numbering definitions.
    Numbering,
    /// Document settings.
    Settings,
    /// The font table.
    FontTable,
    /// The theme.
    Theme,
    /// A chart.
    Chart,
    /// The extended (application) properties.
    ApplicationProperties,
}

impl PartKind {
    /// Whether the part holds document text: the main document, headers, footers, notes and comments.
    const fn is_story(self) -> bool {
        matches!(
            self,
            Self::Document
                | Self::Header
                | Self::Footer
                | Self::Footnotes
                | Self::Endnotes
                | Self::Comments
        )
    }

    /// Whether the part is WordprocessingML, where an element of an unknown namespace is an unknown extension.
    const fn is_wordprocessing(self) -> bool {
        !matches!(
            self,
            Self::Theme | Self::Chart | Self::ApplicationProperties
        )
    }
}

/// A theme font slot that `w:rFonts` can refer to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ThemeSlot {
    /// The major (headings) Latin font.
    MajorLatin,
    /// The major East Asian font.
    MajorEastAsian,
    /// The major complex-script font.
    MajorComplex,
    /// The minor (body) Latin font.
    MinorLatin,
    /// The minor East Asian font.
    MinorEastAsian,
    /// The minor complex-script font.
    MinorComplex,
}

impl ThemeSlot {
    /// The slot that a theme font attribute value of `w:rFonts` names, such as `minorHAnsi`.
    fn from_reference(reference: &str) -> Option<Self> {
        Some(match reference {
            "majorAscii" | "majorHAnsi" => Self::MajorLatin,
            "majorEastAsia" => Self::MajorEastAsian,
            "majorBidi" => Self::MajorComplex,
            "minorAscii" | "minorHAnsi" => Self::MinorLatin,
            "minorEastAsia" => Self::MinorEastAsian,
            "minorBidi" => Self::MinorComplex,
            _ => return None,
        })
    }
}

/// What the parts of one document revealed, collected across all its parts.
#[derive(Debug, Default)]
pub struct Collected {
    /// The features found.
    pub features: BTreeSet<Feature>,
    /// Font names from the font table and from `w:rFonts`.
    pub fonts: BTreeSet<String>,
    /// Language tags from `w:lang` and `w:themeFontLang`.
    pub languages: BTreeSet<String>,
    /// Characters per script in the document's text.
    pub scripts: BTreeMap<Script, u64>,
    /// The value of the `compatibilityMode` setting, if present and a number.
    pub compatibility_mode: Option<u32>,
    /// The legacy compatibility options switched on (children of `w:compat` other than `w:compatSetting`).
    pub compatibility_options: BTreeSet<String>,
    /// The application that last saved the document, from the extended properties.
    pub application: Option<String>,
    /// The page count the application recorded, from the extended properties.
    pub pages_hint: Option<u32>,
    /// Non-fatal observations, from a fixed list of messages (never document content).
    pub notes: BTreeSet<&'static str>,
    /// Typefaces of the theme, by slot.
    pub theme_fonts: BTreeMap<ThemeSlot, String>,
    /// Theme slots that `w:rFonts` refers to.
    pub theme_slots_used: BTreeSet<ThemeSlot>,
    /// Sections (`w:sectPr`) of the main document.
    pub sections: u32,
    /// Sections that have at least one header or footer reference.
    pub sections_with_references: u32,
    /// Tables in the stories.
    pub tables: u32,
    /// Tables with a fixed layout.
    pub fixed_tables: u32,
    /// Whether a Strict namespace was seen.
    pub strict: bool,
    /// `w:eastAsia` language tags, recorded only if the text holds East Asian characters (Word declares one in nearly every document).
    pub east_asian_languages: BTreeSet<String>,
    /// `w:bidi` language tags, recorded only if the text holds complex-script characters (Word declares one in nearly every document).
    pub complex_script_languages: BTreeSet<String>,
    /// Table features that only the styles define, counted only if the document has a table (every Word document defines the "Normal Table" style).
    pub table_style_features: BTreeSet<Feature>,
}

impl Collected {
    fn tag(&mut self, feature: Feature) {
        self.features.insert(feature);
    }

    fn add_font(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() || name.chars().any(char::is_control) {
            return;
        }
        if name.chars().count() > MAX_FONT_NAME {
            self.notes
                .insert("a font name longer than 64 characters was not recorded");
            return;
        }
        if self.fonts.len() >= MAX_FONTS && !self.fonts.contains(name) {
            self.notes
                .insert("more fonts than the limit of 200; the list is cut");
            return;
        }
        self.fonts.insert(name.to_owned());
    }

    /// The canonical form of a language tag, or `None` (with a note) if it is not well-formed.
    fn checked_language(&mut self, tag: &str) -> Option<String> {
        let canonical = canonical_language(tag);
        if canonical.is_none() && !tag.trim().is_empty() {
            self.notes
                .insert("a language tag that is not well-formed was not recorded");
        }
        canonical
    }

    fn add_language(&mut self, tag: &str) {
        if let Some(tag) = self.checked_language(tag) {
            self.insert_language(tag);
        }
    }

    fn insert_language(&mut self, tag: String) {
        if self.languages.len() >= MAX_LANGUAGES && !self.languages.contains(&tag) {
            self.notes
                .insert("more language tags than the limit of 100; the list is cut");
            return;
        }
        self.languages.insert(tag);
    }

    /// Derives the features that depend on the whole document rather than on one element.
    pub fn finish(&mut self) {
        if self.sections >= 2 {
            self.tag(Feature::SectionBreaks);
        }
        if self.sections >= 2
            && self.sections_with_references > 0
            && self.sections_with_references < self.sections
        {
            self.tag(Feature::LinkToPrevious);
        }
        if self.tables > self.fixed_tables {
            self.tag(Feature::TableAutofit);
        }
        if self.strict {
            self.tag(Feature::StrictNamespaces);
        }
        match self.compatibility_mode {
            Some(mode) if mode >= 15 => self.tag(Feature::CompatibilityMode15),
            Some(11 | 12 | 14) => self.tag(Feature::OlderCompatibilityModes),
            _ => {}
        }
        if !self.compatibility_options.is_empty() {
            self.tag(Feature::LegacyCompatibilityOptions);
        }
        let scripts: Vec<Script> = self.scripts.keys().copied().collect();
        if scripts.iter().any(|script| script.is_arabic_or_hebrew()) {
            self.tag(Feature::ArabicAndHebrew);
        }
        if scripts
            .iter()
            .any(|script| script.is_indic_or_southeast_asian())
        {
            self.tag(Feature::IndicAndSoutheastAsian);
        }
        if scripts.iter().any(|script| script.is_east_asian()) {
            for tag in std::mem::take(&mut self.east_asian_languages) {
                self.insert_language(tag);
            }
        }
        if scripts.iter().any(|script| script.is_complex()) {
            for tag in std::mem::take(&mut self.complex_script_languages) {
                self.insert_language(tag);
            }
        }
        if self.tables > 0 {
            let defined = std::mem::take(&mut self.table_style_features);
            self.features.extend(defined);
        }
        let used: Vec<String> = self
            .theme_slots_used
            .iter()
            .filter_map(|slot| self.theme_fonts.get(slot).cloned())
            .collect();
        for font in used {
            self.add_font(&font);
        }
    }
}

/// A language tag in canonical case (`en-US`, `zh-Hant-TW`), or `None` if it is not a well-formed BCP 47 tag shape (ASCII letters, digits and hyphens, 2 to 35 characters, subtags of 1 to 8 characters, starting with letters).
fn canonical_language(tag: &str) -> Option<String> {
    let tag = tag.trim();
    if !(2..=35).contains(&tag.len()) {
        return None;
    }
    let mut out = String::with_capacity(tag.len());
    // After a singleton such as `x` (private use) or `u` (extension), subtags are lower case.
    let mut after_singleton = false;
    for (index, subtag) in tag.split('-').enumerate() {
        if subtag.is_empty()
            || subtag.len() > 8
            || !subtag.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            return None;
        }
        if index == 0 && !subtag.bytes().all(|byte| byte.is_ascii_alphabetic()) {
            return None;
        }
        if index > 0 {
            out.push('-');
        }
        let letters = subtag.bytes().all(|byte| byte.is_ascii_alphabetic());
        if after_singleton || index == 0 || !letters {
            out.push_str(&subtag.to_ascii_lowercase());
        } else if subtag.len() == 2 {
            out.push_str(&subtag.to_ascii_uppercase());
        } else if subtag.len() == 4 {
            let mut chars = subtag.chars();
            if let Some(first) = chars.next() {
                out.push(first.to_ascii_uppercase());
            }
            out.push_str(&chars.as_str().to_ascii_lowercase());
        } else {
            out.push_str(&subtag.to_ascii_lowercase());
        }
        after_singleton |= subtag.len() == 1;
    }
    Some(out)
}

/// What an open element means for the rules of its children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Context {
    RunProperties,
    ParagraphProperties,
    SectionProperties,
    SectionPropertiesChange,
    CellProperties,
    TableProperties,
    Style,
    Level,
    Tabs,
    Columns,
    Compat,
    Fonts,
    EffectList,
    Blip,
    MajorFont,
    MinorFont,
    AlternateContent,
    Other,
}

/// What the text being read belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextKind {
    None,
    /// Document text (`w:t`, `m:t`), counted by script.
    Story,
    /// A field instruction (`w:instrText`).
    Instruction,
    /// The application name in the extended properties.
    Application,
    /// The page count in the extended properties.
    Pages,
}

/// A complex field whose instruction is being read.
#[derive(Debug)]
struct Field {
    instruction: String,
    reading_instruction: bool,
    in_table: bool,
}

/// What the tagger knows about the open `w:style`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StyleState {
    /// Its name starts with "heading".
    heading: bool,
    /// It is a numbering style (`w:type="numbering"`).
    numbering: bool,
}

/// The tagger for one part. Create it with [`PartTagger::new`], give it to the XML scanner, then call [`PartTagger::finish`].
pub struct PartTagger<'c> {
    collected: &'c mut Collected,
    kind: PartKind,
    /// The context of each open element that is not skipped.
    stack: Vec<Context>,
    /// The depth of every open element, skipped ones included.
    depth: usize,
    /// The depth of the element whose content is being skipped.
    skip: Option<usize>,
    /// For each open `mc:AlternateContent`: its depth, and whether a branch was chosen.
    alternates: Vec<(usize, bool)>,
    text: TextKind,
    text_buffer: String,
    script_counts: [u64; 35],
    fields: Vec<Field>,
    /// The open `w:style`.
    style: Option<StyleState>,
    table_depth: usize,
    cell_depth: usize,
    columns: u32,
    section_has_references: bool,
}

/// The scripts in the order of [`PartTagger::script_counts`].
const SCRIPTS: [Script; 35] = [
    Script::Latin,
    Script::Greek,
    Script::Cyrillic,
    Script::Armenian,
    Script::Hebrew,
    Script::Arabic,
    Script::Syriac,
    Script::Thaana,
    Script::NKo,
    Script::Devanagari,
    Script::Bengali,
    Script::Gurmukhi,
    Script::Gujarati,
    Script::Oriya,
    Script::Tamil,
    Script::Telugu,
    Script::Kannada,
    Script::Malayalam,
    Script::Sinhala,
    Script::Thai,
    Script::Lao,
    Script::Tibetan,
    Script::Myanmar,
    Script::Georgian,
    Script::Hangul,
    Script::Ethiopic,
    Script::Cherokee,
    Script::CanadianAboriginal,
    Script::Khmer,
    Script::Mongolian,
    Script::Han,
    Script::Hiragana,
    Script::Katakana,
    Script::Bopomofo,
    Script::Yi,
];

/// Whether an on/off value (`w:val` of a toggle or setting) is on. An absent value means on (ECMA-376 Part 1 §17.17.4).
fn is_on(value: Option<Value<'_>>) -> bool {
    value.is_none_or(|value| !matches!(value.decoded().trim(), "0" | "false" | "off"))
}

/// The value of the attribute with this vocabulary and local name, in either spelling of the vocabulary.
fn attribute<'s>(start: &Start<'_, 's>, ns: Ns, local: &str) -> Option<Value<'s>> {
    start
        .attributes()
        .find(|(name, _)| {
            name.local == local && namespace(name.namespace).is_some_and(|(found, _)| found == ns)
        })
        .map(|(_, value)| value)
}

/// The `w:val` attribute.
fn val<'s>(start: &Start<'_, 's>) -> Option<Cow<'s, str>> {
    attribute(start, Ns::W, "val").map(|value| value.decoded())
}

impl<'c> PartTagger<'c> {
    /// A tagger for one part of the given kind.
    pub fn new(collected: &'c mut Collected, kind: PartKind) -> Self {
        Self {
            collected,
            kind,
            stack: Vec::new(),
            depth: 0,
            skip: None,
            alternates: Vec::new(),
            text: TextKind::None,
            text_buffer: String::new(),
            script_counts: [0; 35],
            fields: Vec::new(),
            style: None,
            table_depth: 0,
            cell_depth: 0,
            columns: 0,
            section_has_references: false,
        }
    }

    /// Adds what this part found to the document's findings.
    pub fn finish(self) {
        for (script, count) in SCRIPTS.iter().zip(self.script_counts) {
            if count > 0 {
                let total = self.collected.scripts.entry(*script).or_insert(0);
                *total = total.saturating_add(count);
            }
        }
    }

    fn tag(&mut self, feature: Feature) {
        self.collected.tag(feature);
    }

    fn parent(&self) -> Context {
        self.stack.last().copied().unwrap_or(Context::Other)
    }

    /// Starts skipping the content of the element that just started (at `self.depth`).
    fn skip_content(&mut self) {
        self.skip = Some(self.depth);
    }

    /// Decides what an `mc:Choice` or `mc:Fallback` does; returns whether its content counts.
    fn alternate_branch(&mut self, start: &Start<'_, '_>, local: &str) -> bool {
        let Some((alternate_depth, chosen)) = self.alternates.last_mut() else {
            return true;
        };
        if *alternate_depth + 1 != self.depth || *chosen {
            return false;
        }
        let take = if local == "Choice" {
            // `Requires` is an unqualified attribute (ECMA-376 Part 3 §10.2.1), unlike `mc:Ignorable`.
            start.attribute("", "Requires").is_some_and(|requires| {
                let requires = requires.decoded();
                let prefixes: Vec<&str> = requires
                    .split_ascii_whitespace()
                    .take(MAX_REQUIRED_PREFIXES + 1)
                    .collect();
                (1..=MAX_REQUIRED_PREFIXES).contains(&prefixes.len())
                    && prefixes.iter().all(|prefix| {
                        start
                            .namespace_of_prefix(prefix)
                            .is_some_and(|uri| namespace(uri).is_some())
                    })
            })
        } else {
            true
        };
        if take {
            *chosen = true;
        }
        take
    }

    fn rule(&mut self, start: &Start<'_, '_>, ns: Ns, local: &str) -> Context {
        match self.kind {
            PartKind::Theme => return self.theme(start, ns, local),
            PartKind::Chart => return self.chart(start, ns, local),
            PartKind::ApplicationProperties => return self.application_properties(ns, local),
            _ => {}
        }
        match ns {
            Ns::W => self.wordprocessing(start, local),
            Ns::W14 => {
                match local {
                    "glow" | "shadow" | "textOutline" | "textFill" | "reflection" | "props3d"
                    | "scene3d" => {
                        self.tag(Feature::TextEffects);
                    }
                    "ligatures" | "numForm" | "numSpacing" | "stylisticSets" | "cntxtAlts" => {
                        self.tag(Feature::OpenTypeFeatures);
                    }
                    "contentPart" => self.tag(Feature::Ink),
                    _ => {}
                }
                Context::Other
            }
            Ns::W15 => {
                match local {
                    "repeatingSection" | "repeatingSectionItem" => {
                        self.tag(Feature::RepeatingSections)
                    }
                    "dataBinding" => self.tag(Feature::XmlDataBinding),
                    "commentEx" => self.tag(Feature::Comments),
                    _ => {}
                }
                Context::Other
            }
            // Drawings, objects and equations count only where they are part of the document's stories: the numbering part's picture bullets, for example, define bullets, not shapes.
            _ if !self.kind.is_story() => Context::Other,
            Ns::Wp => {
                match local {
                    "wrapSquare" | "wrapTight" | "wrapThrough" | "wrapTopAndBottom"
                    | "wrapNone" => {
                        self.tag(Feature::TextWrapping);
                    }
                    "positionH" | "positionV" => self.tag(Feature::ObjectPositioning),
                    _ => {}
                }
                Context::Other
            }
            Ns::W10 if local == "wrap" => {
                self.tag(Feature::TextWrapping);
                Context::Other
            }
            Ns::Pic if local == "pic" => {
                self.tag(Feature::Pictures);
                Context::Other
            }
            Ns::Wps => {
                match local {
                    "wsp" => self.tag(Feature::Shapes),
                    "txbx" | "linkedTxbx" => self.tag(Feature::TextBoxes),
                    _ => {}
                }
                Context::Other
            }
            Ns::Wpg | Ns::Wpc if matches!(local, "wgp" | "wpc") => {
                self.tag(Feature::GroupsAndCanvases);
                Context::Other
            }
            Ns::Wpi | Ns::Aink => {
                self.tag(Feature::Ink);
                Context::Other
            }
            Ns::Am3d => {
                self.tag(Feature::Models3d);
                Context::Other
            }
            Ns::A => self.drawing(start, local),
            Ns::AExtension if local == "svgBlip" => {
                self.tag(Feature::ImageFormats);
                Context::Other
            }
            Ns::V => self.vml(start, local),
            Ns::O if local == "OLEObject" => {
                self.tag(Feature::OleObjects);
                let program = attribute(start, Ns::O, "ProgID")
                    .or_else(|| start.attribute("", "ProgID"))
                    .map(|value| value.decoded().into_owned())
                    .unwrap_or_default();
                if program.starts_with("Equation.")
                    || program.contains("MathType")
                    || program.contains("DSMT")
                {
                    self.tag(Feature::LegacyEquations);
                }
                Context::Other
            }
            Ns::M => {
                match local {
                    "oMath" | "oMathPara" => self.tag(Feature::Equations),
                    "t" if self.kind.is_story() => self.text = TextKind::Story,
                    _ => {}
                }
                Context::Other
            }
            Ns::C if local == "chart" => {
                self.tag(Feature::Charts);
                Context::Other
            }
            Ns::Cx if local == "chart" => {
                self.tag(Feature::NewerChartTypes);
                Context::Other
            }
            Ns::Dgm if local == "relIds" => {
                self.tag(Feature::SmartArt);
                Context::Other
            }
            _ => Context::Other,
        }
    }

    /// The theme: only its font scheme matters.
    fn theme(&mut self, start: &Start<'_, '_>, ns: Ns, local: &str) -> Context {
        if ns != Ns::A {
            return Context::Other;
        }
        let slot = match (self.parent(), local) {
            (_, "majorFont") => return Context::MajorFont,
            (_, "minorFont") => return Context::MinorFont,
            (Context::MajorFont, "latin") => ThemeSlot::MajorLatin,
            (Context::MajorFont, "ea") => ThemeSlot::MajorEastAsian,
            (Context::MajorFont, "cs") => ThemeSlot::MajorComplex,
            (Context::MinorFont, "latin") => ThemeSlot::MinorLatin,
            (Context::MinorFont, "ea") => ThemeSlot::MinorEastAsian,
            (Context::MinorFont, "cs") => ThemeSlot::MinorComplex,
            _ => return Context::Other,
        };
        if let Some(typeface) = start.attribute("", "typeface") {
            let typeface = typeface.decoded().trim().to_owned();
            if !typeface.is_empty() {
                self.collected.theme_fonts.insert(slot, typeface);
            }
        }
        Context::Other
    }

    /// A chart part: its 3D chart types and the fonts its text names.
    fn chart(&mut self, start: &Start<'_, '_>, ns: Ns, local: &str) -> Context {
        match (ns, local) {
            (
                Ns::C,
                "bar3DChart" | "line3DChart" | "pie3DChart" | "area3DChart" | "surface3DChart",
            ) => {
                self.tag(Feature::Charts3d);
            }
            (Ns::A, "latin" | "ea" | "cs" | "sym") => self.drawing_font(start),
            _ => {}
        }
        Context::Other
    }

    /// The application properties: the application's name and its page count.
    fn application_properties(&mut self, ns: Ns, local: &str) -> Context {
        if ns == Ns::ExtendedProperties {
            match local {
                "Application" => self.text = TextKind::Application,
                "Pages" => self.text = TextKind::Pages,
                _ => {}
            }
        }
        Context::Other
    }

    /// Records the font that a DrawingML text element names; `+mn-lt` and the like refer to the theme.
    fn drawing_font(&mut self, start: &Start<'_, '_>) {
        if let Some(typeface) = start.attribute("", "typeface") {
            let typeface = typeface.decoded();
            if !typeface.trim_start().starts_with('+') {
                self.collected.add_font(&typeface);
            }
        }
    }

    /// Tags a feature of table formatting; in the styles, only if the document turns out to have a table.
    fn table_tag(&mut self, feature: Feature) {
        if self.kind == PartKind::Styles {
            self.collected.table_style_features.insert(feature);
        } else {
            self.tag(feature);
        }
    }

    fn drawing(&mut self, start: &Start<'_, '_>, local: &str) -> Context {
        match local {
            "blip" => {
                self.tag(Feature::ImageFormats);
                return Context::Blip;
            }
            "srcRect" => {
                if ["l", "t", "r", "b"].iter().any(|edge| {
                    start
                        .attribute("", edge)
                        .is_some_and(|value| !matches!(value.decoded().trim(), "" | "0"))
                }) {
                    self.tag(Feature::PictureAdjustments);
                }
            }
            "effectLst" => return Context::EffectList,
            "scene3d" | "sp3d" => self.tag(Feature::ShapeEffects),
            "prstTxWarp" => self.tag(Feature::WordArt),
            "latin" | "ea" | "cs" | "sym" => self.drawing_font(start),
            _ => {}
        }
        match self.parent() {
            Context::EffectList => self.tag(Feature::ShapeEffects),
            Context::Blip
                if matches!(
                    local,
                    "duotone"
                        | "alphaModFix"
                        | "grayscl"
                        | "biLevel"
                        | "clrChange"
                        | "lum"
                        | "alphaBiLevel"
                        | "alphaCeiling"
                        | "alphaFloor"
                        | "alphaInv"
                        | "alphaMod"
                        | "alphaRepl"
                        | "blur"
                        | "clrRepl"
                        | "fillOverlay"
                        | "hsl"
                        | "tint"
                ) =>
            {
                self.tag(Feature::PictureAdjustments);
            }
            _ => {}
        }
        Context::Other
    }

    fn vml(&mut self, start: &Start<'_, '_>, local: &str) -> Context {
        self.tag(Feature::VmlShapes);
        let header = self.kind == PartKind::Header;
        match local {
            "group" => self.tag(Feature::GroupsAndCanvases),
            "textbox" => self.tag(Feature::TextBoxes),
            "imagedata" => self.tag(Feature::ImageFormats),
            "textpath" => {
                self.tag(Feature::WordArt);
                if header {
                    self.tag(Feature::Watermarks);
                }
            }
            "shape" if header => {
                let id = start
                    .attribute("", "id")
                    .map(|value| value.decoded().into_owned())
                    .unwrap_or_default();
                if id.starts_with("PowerPlusWaterMarkObject")
                    || id.starts_with("WordPictureWatermark")
                {
                    self.tag(Feature::Watermarks);
                }
            }
            _ => {}
        }
        Context::Other
    }

    fn wordprocessing(&mut self, start: &Start<'_, '_>, local: &str) -> Context {
        let parent = self.parent();
        let in_style = self.style.is_some();
        if in_style
            && matches!(
                local,
                "b" | "bCs"
                    | "i"
                    | "iCs"
                    | "caps"
                    | "smallCaps"
                    | "strike"
                    | "dstrike"
                    | "outline"
                    | "shadow"
                    | "emboss"
                    | "imprint"
                    | "vanish"
            )
        {
            self.tag(Feature::ToggleProperties);
        }
        if parent == Context::Compat && local != "compatSetting" {
            if is_on(attribute(start, Ns::W, "val")) {
                self.collected
                    .compatibility_options
                    .insert(local.to_owned());
            }
            return Context::Other;
        }
        match local {
            // Characters and runs.
            "t" => {
                self.tag(Feature::TextTabsBreaksSymbols);
                if self.kind.is_story() {
                    self.text = TextKind::Story;
                }
            }
            "br" | "cr" | "sym" => self.tag(Feature::TextTabsBreaksSymbols),
            "tab" if parent == Context::Tabs => self.tag(Feature::TabStops),
            "tab" => self.tag(Feature::TextTabsBreaksSymbols),
            "rFonts" => self.fonts_of_run(start),
            "sz" | "szCs" | "b" | "bCs" | "i" | "iCs" => self.tag(Feature::SizeBoldItalic),
            "u" => self.tag(Feature::Underline),
            "strike" | "dstrike" | "caps" | "smallCaps" | "vanish" => {
                self.tag(Feature::StrikeCapsHidden)
            }
            "outline" | "shadow" | "emboss" | "imprint" => self.tag(Feature::TextEffects),
            "color" if parent == Context::RunProperties => {
                self.tag(Feature::Color);
                self.theme_color(start);
            }
            "highlight" => self.tag(Feature::HighlightAndShading),
            "shd" => {
                match parent {
                    Context::RunProperties => self.tag(Feature::HighlightAndShading),
                    Context::ParagraphProperties => self.tag(Feature::ParagraphBordersAndShading),
                    Context::CellProperties | Context::TableProperties => {
                        self.table_tag(Feature::TableBordersAndShading)
                    }
                    _ => {}
                }
                self.theme_color(start);
            }
            "vertAlign" | "position" => self.tag(Feature::VerticalPosition),
            "spacing" if parent == Context::RunProperties => self.tag(Feature::CharacterSpacing),
            "spacing" if parent == Context::ParagraphProperties => {
                if [
                    "before",
                    "after",
                    "beforeLines",
                    "afterLines",
                    "beforeAutospacing",
                    "afterAutospacing",
                ]
                .iter()
                .any(|name| attribute(start, Ns::W, name).is_some())
                {
                    self.tag(Feature::ParagraphSpacing);
                }
                if attribute(start, Ns::W, "line").is_some() {
                    self.tag(Feature::LineSpacing);
                }
            }
            "w" | "kern" if parent == Context::RunProperties => self.tag(Feature::CharacterSpacing),
            "bdr" => self.tag(Feature::TextBorders),
            "em" => self.tag(Feature::EmphasisMarks),
            "eastAsianLayout" => {
                self.tag(Feature::EmphasisMarks);
                if ["combine", "vert"].iter().any(|name| {
                    attribute(start, Ns::W, name).is_some_and(|value| is_on(Some(value)))
                }) {
                    self.tag(Feature::CombineCharacters);
                }
            }
            "fitText" => self.tag(Feature::FitText),
            "lang" => {
                self.tag(Feature::LanguageTags);
                self.languages(start);
            }
            "themeFontLang" => self.languages(start),
            "rtl" | "cs" => self.tag(Feature::RightToLeftRuns),
            "hyperlink" => self.tag(Feature::Hyperlinks),
            "ruby" => self.tag(Feature::Ruby),
            // Paragraphs.
            "jc" if parent == Context::ParagraphProperties => {
                self.tag(Feature::Alignment);
                if val(start).is_some_and(|value| value.contains("Kashida")) {
                    self.tag(Feature::Kashida);
                }
            }
            "ind" if parent == Context::ParagraphProperties => self.tag(Feature::Indentation),
            "contextualSpacing" => self.tag(Feature::ParagraphSpacing),
            "tabs" => {
                self.tag(Feature::TabStops);
                return Context::Tabs;
            }
            "keepNext" | "keepLines" | "pageBreakBefore" | "widowControl" => {
                self.tag(Feature::KeepAndWidowControl)
            }
            "pBdr" => self.tag(Feature::ParagraphBordersAndShading),
            "outlineLvl" => self.tag(Feature::OutlineLevel),
            "framePr" => self.tag(Feature::DropCapsAndFrames),
            "bidi" if parent == Context::SectionProperties => {
                self.tag(Feature::SectionTextDirection)
            }
            "bidi" => self.tag(Feature::BidirectionalParagraphs),
            "snapToGrid" | "wordWrap" | "overflowPunct" | "topLinePunct" => {
                self.tag(Feature::EastAsianLineBreaking)
            }
            "kinsoku" => {
                self.tag(Feature::EastAsianLineBreaking);
                self.tag(Feature::Kinsoku);
            }
            "autoSpaceDE" | "autoSpaceDN" => {
                self.tag(Feature::EastAsianLineBreaking);
                self.tag(Feature::AsianAutoSpacing);
            }
            "noLineBreaksBefore" | "noLineBreaksAfter" => self.tag(Feature::Kinsoku),
            "textAlignment" => self.tag(Feature::TextAlignmentAndDirection),
            "textDirection" => {
                match parent {
                    Context::SectionProperties => self.tag(Feature::SectionTextDirection),
                    Context::CellProperties => self.table_tag(Feature::CellTextDirection),
                    _ => self.tag(Feature::TextAlignmentAndDirection),
                }
                if val(start).is_some_and(|value| !matches!(value.trim(), "lrTb" | "lr")) {
                    self.tag(Feature::VerticalText);
                }
            }
            "suppressLineNumbers" | "suppressAutoHyphens" => {
                self.tag(Feature::SuppressLineNumbersAndHyphens)
            }
            "rPr" => {
                if parent == Context::ParagraphProperties {
                    self.tag(Feature::ParagraphMarkFormatting);
                }
                return Context::RunProperties;
            }
            "pPr" => return Context::ParagraphProperties,
            // Styles and themes.
            "docDefaults" => self.tag(Feature::DocumentDefaults),
            "style" => {
                let kind = attribute(start, Ns::W, "type").map(|value| value.decoded());
                let kind = kind.as_deref().map(str::trim);
                if matches!(kind, Some("paragraph" | "character")) {
                    self.tag(Feature::ParagraphAndCharacterStyles);
                }
                self.style = Some(StyleState {
                    heading: false,
                    numbering: kind == Some("numbering"),
                });
                return Context::Style;
            }
            "name" if parent == Context::Style => {
                if val(start)
                    .is_some_and(|name| name.trim().to_ascii_lowercase().starts_with("heading"))
                    && let Some(style) = &mut self.style
                {
                    style.heading = true;
                }
            }
            "tblStylePr" => self.table_tag(Feature::TableStyles),
            "latentStyles" => self.tag(Feature::LatentStyles),
            "autoRedefine" => self.tag(Feature::StyleAutoRedefinition),
            // Numbering.
            "abstractNum" | "num" | "lvlOverride" => self.tag(Feature::AbstractNumbering),
            "lvl" => return Context::Level,
            "numFmt" => self.tag(Feature::NumberFormats),
            "lvlText" | "suff" | "lvlJc" | "isLgl" => self.tag(Feature::LevelTextAndSuffix),
            "lvlRestart" | "startOverride" => self.tag(Feature::RestartRules),
            "numPicBullet" | "lvlPicBulletId" => self.tag(Feature::PictureBullets),
            "legacy" => self.tag(Feature::LegacyNumbering),
            "pStyle" if parent == Context::Level => self.tag(Feature::HeadingNumbering),
            "numPr" => {
                if let Some(style) = self.style {
                    if style.numbering {
                        self.tag(Feature::NumberingStyles);
                    }
                    if style.heading {
                        self.tag(Feature::HeadingNumbering);
                    }
                }
            }
            // Sections and page layout.
            "sectPr" => {
                if parent == Context::SectionPropertiesChange || self.kind != PartKind::Document {
                    return Context::Other;
                }
                self.collected.sections = self.collected.sections.saturating_add(1);
                self.section_has_references = false;
                return Context::SectionProperties;
            }
            "sectPrChange" => {
                self.tag(Feature::TrackedFormatting);
                return Context::SectionPropertiesChange;
            }
            "pgSz" | "pgMar" => self.tag(Feature::PageSizeAndMargins),
            "cols" => {
                let several = attribute(start, Ns::W, "num")
                    .and_then(|value| value.decoded().trim().parse::<u32>().ok())
                    .is_some_and(|count| count > 1);
                let separated =
                    attribute(start, Ns::W, "sep").is_some_and(|value| is_on(Some(value)));
                if several || separated {
                    self.tag(Feature::Columns);
                }
                self.columns = 0;
                return Context::Columns;
            }
            "col" if parent == Context::Columns => {
                self.columns += 1;
                if self.columns >= 2 {
                    self.tag(Feature::Columns);
                }
            }
            "vAlign" if parent == Context::SectionProperties => {
                self.tag(Feature::PageVerticalAlignment)
            }
            "vAlign" if parent == Context::CellProperties => {
                self.table_tag(Feature::CellTextDirection)
            }
            "pgBorders" => self.tag(Feature::PageBorders),
            "lnNumType" => self.tag(Feature::LineNumbering),
            "pgNumType" => {
                if ["fmt", "start", "chapStyle", "chapSep"]
                    .iter()
                    .any(|name| attribute(start, Ns::W, name).is_some())
                {
                    self.tag(Feature::PageNumbering);
                }
            }
            "docGrid" => match attribute(start, Ns::W, "type")
                .map(|value| value.decoded())
                .as_deref()
                .map(str::trim)
            {
                Some("lines") => self.tag(Feature::DocumentGrid),
                Some("linesAndChars" | "snapToChars") => {
                    self.tag(Feature::DocumentGrid);
                    self.tag(Feature::CharacterGrid);
                }
                _ => {}
            },
            "rtlGutter" => self.tag(Feature::SectionTextDirection),
            "mirrorMargins" | "bookFoldPrinting" | "bookFoldRevPrinting" | "gutterAtTop" => {
                if is_on(attribute(start, Ns::W, "val")) {
                    self.tag(Feature::MirrorMarginsAndBookFold);
                }
            }
            // Headers and footers.
            "headerReference" | "footerReference" => {
                self.tag(Feature::HeadersAndFooters);
                self.tag(Feature::HeaderFooterDistances);
                if parent == Context::SectionProperties {
                    self.section_has_references = true;
                }
            }
            "titlePg" | "evenAndOddHeaders" => {
                if is_on(attribute(start, Ns::W, "val")) {
                    self.tag(Feature::HeadersAndFooters);
                }
            }
            // Tables.
            "tbl" => {
                self.tag(Feature::TableGrid);
                if self.table_depth > 0 {
                    self.tag(Feature::NestedTables);
                }
                self.table_depth += 1;
                self.collected.tables = self.collected.tables.saturating_add(1);
            }
            "tblPr" => return Context::TableProperties,
            "tcPr" => return Context::CellProperties,
            "tc" => self.cell_depth += 1,
            "tblLayout" if parent == Context::TableProperties && self.table_depth > 0 => {
                if attribute(start, Ns::W, "type")
                    .is_some_and(|value| value.decoded().trim() == "fixed")
                {
                    self.collected.fixed_tables = self.collected.fixed_tables.saturating_add(1);
                }
            }
            "gridSpan" | "vMerge" | "hMerge" => self.table_tag(Feature::CellMerges),
            "tblBorders" | "tcBorders" => self.table_tag(Feature::TableBordersAndShading),
            "tblCellMar" | "tcMar" | "tblCellSpacing" => {
                self.table_tag(Feature::CellMarginsAndSpacing)
            }
            "trHeight" | "cantSplit" | "tblHeader" => {
                self.table_tag(Feature::RowHeightAndHeaderRows)
            }
            "tblpPr" => self.table_tag(Feature::FloatingTables),
            "bidiVisual" => self.table_tag(Feature::RightToLeftTables),
            // Fields.
            "fldSimple" => {
                self.tag(Feature::Fields);
                let instruction = attribute(start, Ns::W, "instr")
                    .map(|value| value.decoded().into_owned())
                    .unwrap_or_default();
                let in_table = self.cell_depth > 0;
                self.classify_field(&instruction, in_table);
            }
            "fldChar" => {
                self.tag(Feature::Fields);
                self.field_character(start);
            }
            "instrText" => self.text = TextKind::Instruction,
            "ffData" => self.tag(Feature::LegacyFormFields),
            // Notes, comments and review.
            "footnoteReference" | "endnoteReference" => self.tag(Feature::FootnotesAndEndnotes),
            "commentReference" | "commentRangeStart" => self.tag(Feature::Comments),
            "comment" if self.kind == PartKind::Comments => self.tag(Feature::Comments),
            "ins" | "del" => self.tag(Feature::TrackedInsertionsAndDeletions),
            "moveFrom" | "moveTo" | "moveFromRangeStart" | "moveToRangeStart" => {
                self.tag(Feature::TrackedMoves)
            }
            "rPrChange" | "pPrChange" | "tblPrChange" | "tcPrChange" | "trPrChange"
            | "tblGridChange" | "tblPrExChange" | "numberingChange" => {
                self.tag(Feature::TrackedFormatting);
            }
            "documentProtection" | "writeProtection" => {
                if attribute(start, Ns::W, "enforcement").is_some_and(|value| is_on(Some(value)))
                    || local == "writeProtection"
                {
                    self.tag(Feature::RestrictEditing);
                }
            }
            "permStart" | "permEnd" => self.tag(Feature::RestrictEditing),
            // Drawings and objects, only in the stories (the numbering part's picture bullets hold a `w:pict` too).
            "pict" if self.kind.is_story() => self.tag(Feature::VmlShapes),
            "object" if self.kind.is_story() => self.tag(Feature::OleObjects),
            "txbxContent" if self.kind.is_story() => self.tag(Feature::TextBoxes),
            "contentPart" if self.kind.is_story() => self.tag(Feature::Ink),
            // Content controls.
            "sdt" => self.tag(Feature::ContentControls),
            "docPartObj" | "docPartList" => self.tag(Feature::RepeatingSections),
            "dataBinding" => self.tag(Feature::XmlDataBinding),
            // Settings and the font table.
            "compat" => return Context::Compat,
            "compatSetting" if parent == Context::Compat => {
                let name =
                    attribute(start, Ns::W, "name").map(|value| value.decoded().into_owned());
                if name.as_deref().map(str::trim) == Some("compatibilityMode") {
                    self.collected.compatibility_mode =
                        val(start).and_then(|value| value.trim().parse().ok());
                }
            }
            "mailMerge" => self.tag(Feature::MailMergeFields),
            "fonts" if self.kind == PartKind::FontTable => return Context::Fonts,
            "font" if parent == Context::Fonts => {
                if let Some(name) = attribute(start, Ns::W, "name") {
                    self.collected.add_font(&name.decoded());
                }
            }
            "embedRegular" | "embedBold" | "embedItalic" | "embedBoldItalic" => {
                self.tag(Feature::EmbeddedFonts)
            }
            _ => {}
        }
        Context::Other
    }

    fn fonts_of_run(&mut self, start: &Start<'_, '_>) {
        self.tag(Feature::FontsPerScript);
        for slot in ["ascii", "hAnsi", "eastAsia", "cs"] {
            if let Some(name) = attribute(start, Ns::W, slot) {
                self.collected.add_font(&name.decoded());
            }
        }
        for slot in ["asciiTheme", "hAnsiTheme", "eastAsiaTheme", "cstheme"] {
            if let Some(reference) = attribute(start, Ns::W, slot) {
                self.tag(Feature::ThemeFontsAndColors);
                if let Some(slot) = ThemeSlot::from_reference(reference.decoded().trim()) {
                    self.collected.theme_slots_used.insert(slot);
                }
            }
        }
    }

    fn theme_color(&mut self, start: &Start<'_, '_>) {
        if [
            "themeColor",
            "themeFill",
            "themeFillTint",
            "themeFillShade",
            "themeTint",
            "themeShade",
        ]
        .iter()
        .any(|name| attribute(start, Ns::W, name).is_some())
        {
            self.tag(Feature::ThemeFontsAndColors);
        }
    }

    /// Records the language tags of `w:lang` or `w:themeFontLang`. The East Asian and complex-script tags are kept aside until the scripts of the text are known, because Word writes them into nearly every document whatever its text.
    fn languages(&mut self, start: &Start<'_, '_>) {
        if let Some(tag) = attribute(start, Ns::W, "val") {
            self.collected.add_language(&tag.decoded());
        }
        if let Some(tag) = attribute(start, Ns::W, "eastAsia")
            && let Some(tag) = self.collected.checked_language(&tag.decoded())
        {
            self.collected.east_asian_languages.insert(tag);
        }
        if let Some(tag) = attribute(start, Ns::W, "bidi")
            && let Some(tag) = self.collected.checked_language(&tag.decoded())
        {
            self.collected.complex_script_languages.insert(tag);
        }
    }

    fn field_character(&mut self, start: &Start<'_, '_>) {
        let kind =
            attribute(start, Ns::W, "fldCharType").map(|value| value.decoded().trim().to_owned());
        match kind.as_deref() {
            Some("begin") => {
                if self.fields.len() >= MAX_FIELD_DEPTH {
                    self.collected
                        .notes
                        .insert("fields nested deeper than the limit of 64 were not classified");
                    return;
                }
                self.fields.push(Field {
                    instruction: String::new(),
                    reading_instruction: true,
                    in_table: self.cell_depth > 0,
                });
            }
            Some("separate") => {
                if let Some(field) = self.fields.last_mut()
                    && field.reading_instruction
                {
                    field.reading_instruction = false;
                    let instruction = std::mem::take(&mut field.instruction);
                    let in_table = field.in_table;
                    self.classify_field(&instruction, in_table);
                }
            }
            Some("end") => {
                if let Some(field) = self.fields.pop()
                    && field.reading_instruction
                {
                    self.classify_field(&field.instruction, field.in_table);
                }
            }
            _ => {}
        }
    }

    /// Tags the features of a field from its instruction, such as `PAGE \* MERGEFORMAT`.
    fn classify_field(&mut self, instruction: &str, in_table: bool) {
        let instruction = instruction.trim_start();
        let kind: String = if instruction.starts_with('=') {
            "=".to_owned()
        } else {
            instruction
                .split(|character: char| character.is_whitespace() || character == '\\')
                .next()
                .unwrap_or("")
                .to_ascii_uppercase()
        };
        let feature = match kind.as_str() {
            "PAGE" | "NUMPAGES" | "SECTIONPAGES" | "PAGEREF" => Some(Feature::PageFields),
            "DATE" | "TIME" | "CREATEDATE" | "SAVEDATE" | "PRINTDATE" => {
                Some(Feature::DateAndTimeFields)
            }
            "REF" | "NOTEREF" | "SEQ" | "STYLEREF" => Some(Feature::CrossReferences),
            "AUTHOR" | "TITLE" | "FILENAME" | "DOCPROPERTY" | "DOCVARIABLE" | "SUBJECT"
            | "KEYWORDS" | "COMMENTS" | "LASTSAVEDBY" | "NUMWORDS" | "NUMCHARS" | "FILESIZE"
            | "INFO" | "TEMPLATE" | "USERNAME" | "USERINITIALS" | "USERADDRESS" | "EDITTIME"
            | "REVNUM" => Some(Feature::DocumentInformationFields),
            "TOC" | "TC" => Some(Feature::TablesOfContents),
            "=" if in_table => Some(Feature::TableFormulas),
            "=" | "IF" | "COMPARE" => Some(Feature::FormulasAndConditions),
            "MERGEFIELD" | "NEXT" | "NEXTIF" | "SKIPIF" | "ASK" | "FILLIN" | "MERGEREC"
            | "MERGESEQ" | "ADDRESSBLOCK" | "GREETINGLINE" | "DATABASE" => {
                Some(Feature::MailMergeFields)
            }
            "XE" | "INDEX" | "TA" | "TOA" => Some(Feature::IndexesAndAuthorities),
            "CITATION" | "BIBLIOGRAPHY" => Some(Feature::Citations),
            "FORMTEXT" | "FORMCHECKBOX" | "FORMDROPDOWN" => Some(Feature::LegacyFormFields),
            "INCLUDETEXT" | "INCLUDEPICTURE" | "INCLUDE" | "IMPORT" | "LINK" | "DDE"
            | "DDEAUTO" => Some(Feature::ExternalFields),
            "HYPERLINK" => Some(Feature::Hyperlinks),
            "EQ" => Some(Feature::CombineCharacters),
            "SYMBOL" => Some(Feature::TextTabsBreaksSymbols),
            _ => None,
        };
        if let Some(feature) = feature {
            self.tag(feature);
        }
        let upper = instruction.to_ascii_uppercase();
        if upper.contains("\\*")
            || upper.contains("\\#")
            || upper.contains("\\@")
            || upper.contains("MERGEFORMAT")
        {
            self.tag(Feature::FieldFormattingSwitches);
        }
    }

    fn count_scripts(&mut self, text: &str) {
        for character in text.chars() {
            if let Some(script) = script_of(character)
                && let Some(index) = SCRIPTS.iter().position(|&known| known == script)
                && let Some(count) = self.script_counts.get_mut(index)
            {
                *count = count.saturating_add(1);
            }
        }
    }

    /// Ends reading the text of an element of the extended properties.
    fn finish_property_text(&mut self) {
        let text = std::mem::take(&mut self.text_buffer);
        let text = text.trim();
        match self.text {
            TextKind::Application => {
                if text.len() <= MAX_PROPERTY_TEXT && !text.chars().any(char::is_control) {
                    self.collected.application = application_name(text);
                }
            }
            TextKind::Pages => self.collected.pages_hint = text.parse().ok(),
            TextKind::None | TextKind::Story | TextKind::Instruction => {}
        }
    }
}

/// Appends as much of `text` to `buffer` as fits in `limit` bytes, cut at a character boundary.
fn push_limited(buffer: &mut String, text: &str, limit: usize) {
    let room = limit.saturating_sub(buffer.len());
    let end = (0..=room.min(text.len()))
        .rev()
        .find(|&index| text.is_char_boundary(index))
        .unwrap_or(0);
    buffer.push_str(text.get(..end).unwrap_or_default());
}

/// The name of the application that saved a document, as the manifest records it: the text before a `$` (LibreOffice appends its platform and build there), without any GUID (WPS Office appends one per installation), so that the name says which program saved the document but cannot identify a machine.
fn application_name(text: &str) -> Option<String> {
    let text = text.split('$').next().unwrap_or_default();
    let mut kept = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(character) = rest.chars().next() {
        let guid = guid_length(rest.as_bytes());
        if guid > 0 {
            rest = rest.get(guid..).unwrap_or_default();
            continue;
        }
        kept.push(character);
        rest = rest.get(character.len_utf8()..).unwrap_or_default();
    }
    let name = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    let name = name.trim_matches(|character: char| matches!(character, '_' | '-' | ' '));
    (!name.is_empty()).then(|| name.to_owned())
}

/// The length of the GUID that `bytes` starts with (`8-4-4-4-12` hexadecimal digits, optionally in braces), or 0.
fn guid_length(bytes: &[u8]) -> usize {
    let braced = bytes.first() == Some(&b'{');
    let mut index = usize::from(braced);
    for (group, digits) in [8, 4, 4, 4, 12].into_iter().enumerate() {
        if group > 0 {
            if bytes.get(index) != Some(&b'-') {
                return 0;
            }
            index += 1;
        }
        for _ in 0..digits {
            if !bytes.get(index).is_some_and(u8::is_ascii_hexdigit) {
                return 0;
            }
            index += 1;
        }
    }
    if braced {
        if bytes.get(index) != Some(&b'}') {
            return 0;
        }
        index += 1;
    }
    index
}

impl Handler for PartTagger<'_> {
    fn start(&mut self, start: &Start<'_, '_>) {
        self.depth += 1;
        if self.skip.is_some() {
            return;
        }
        let name = start.name();
        if attribute(start, Ns::Mc, "Ignorable").is_some() {
            self.tag(Feature::MarkupCompatibility);
        }
        let Some((ns, strict)) = namespace(name.namespace) else {
            if self.kind.is_wordprocessing() {
                self.tag(Feature::UnknownPartsAndExtensions);
            }
            self.skip_content();
            return;
        };
        if strict {
            self.collected.strict = true;
        }
        let context = if ns == Ns::Mc {
            match name.local {
                "AlternateContent" => {
                    self.tag(Feature::MarkupCompatibility);
                    self.alternates.push((self.depth, false));
                    Context::AlternateContent
                }
                "Choice" | "Fallback" => {
                    if !self.alternate_branch(start, name.local) {
                        self.skip_content();
                        return;
                    }
                    Context::Other
                }
                _ => Context::Other,
            }
        } else {
            self.rule(start, ns, name.local)
        };
        self.stack.push(context);
    }

    fn end(&mut self, name: Name<'_>) {
        if let Some(skip) = self.skip {
            if self.depth == skip {
                self.skip = None;
            }
            self.depth = self.depth.saturating_sub(1);
            return;
        }
        let context = self.stack.pop().unwrap_or(Context::Other);
        match context {
            Context::AlternateContent => {
                self.alternates.pop();
            }
            Context::SectionProperties if self.section_has_references => {
                self.collected.sections_with_references =
                    self.collected.sections_with_references.saturating_add(1);
            }
            _ => {}
        }
        match namespace(name.namespace).map(|(ns, _)| ns) {
            Some(Ns::W) => match name.local {
                "t" | "instrText" => self.text = TextKind::None,
                "tbl" => self.table_depth = self.table_depth.saturating_sub(1),
                "tc" => self.cell_depth = self.cell_depth.saturating_sub(1),
                "style" => self.style = None,
                _ => {}
            },
            Some(Ns::M) if name.local == "t" => self.text = TextKind::None,
            Some(Ns::ExtendedProperties) if matches!(name.local, "Application" | "Pages") => {
                self.finish_property_text();
                self.text = TextKind::None;
            }
            _ => {}
        }
        self.depth = self.depth.saturating_sub(1);
    }

    fn text(&mut self, text: Text<'_>) {
        if self.skip.is_some() {
            return;
        }
        match self.text {
            TextKind::None => {}
            TextKind::Story => self.count_scripts(&text.decoded()),
            TextKind::Instruction => {
                if let Some(field) = self.fields.last_mut()
                    && field.reading_instruction
                {
                    push_limited(&mut field.instruction, &text.decoded(), MAX_INSTRUCTION);
                }
            }
            // One byte more than the limit is kept, so that a longer text is recognized as such and ignored.
            TextKind::Application | TextKind::Pages => {
                push_limited(
                    &mut self.text_buffer,
                    &text.decoded(),
                    MAX_PROPERTY_TEXT + 1,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_tags_are_put_in_canonical_case() {
        assert_eq!(canonical_language("en-us").as_deref(), Some("en-US"));
        assert_eq!(
            canonical_language(" ZH-hant-tw ").as_deref(),
            Some("zh-Hant-TW")
        );
        assert_eq!(canonical_language("x-none").as_deref(), Some("x-none"));
        assert_eq!(
            canonical_language("EN-us-X-Twain").as_deref(),
            Some("en-US-x-twain")
        );
        assert_eq!(canonical_language("ar-SA").as_deref(), Some("ar-SA"));
        assert_eq!(canonical_language("e"), None);
        assert_eq!(canonical_language("en_US"), None);
        assert_eq!(canonical_language("1en"), None);
        assert_eq!(canonical_language("en--US"), None);
        assert_eq!(canonical_language(&"a".repeat(36)), None);
    }

    #[test]
    fn application_names_identify_the_program_but_not_the_machine() {
        for (text, name) in [
            ("Microsoft Office Word", Some("Microsoft Office Word")),
            (
                "LibreOffice/24.2.0.3$Linux_X86_64 LibreOffice_project/420$Build-3",
                Some("LibreOffice/24.2.0.3"),
            ),
            (
                "WPS Office_11.1.0.10700_F1E327BC-269C-435d-A152-05C5408002CA",
                Some("WPS Office_11.1.0.10700"),
            ),
            (
                "Writer {F1E327BC-269C-435D-A152-05C5408002CA} 2",
                Some("Writer 2"),
            ),
            (
                "  docx4j  (https://docx4java.org) ",
                Some("docx4j (https://docx4java.org)"),
            ),
            ("F1E327BC-269C-435d-A152-05C5408002CA", None),
            ("$Linux", None),
            ("F1E327BC-269C-435d-A152", Some("F1E327BC-269C-435d-A152")),
        ] {
            assert_eq!(application_name(text).as_deref(), name, "{text}");
        }
    }

    #[test]
    fn on_off_values_follow_the_standard() {
        assert!(is_on(None));
    }
}
