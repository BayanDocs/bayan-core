//! Fonts (ISO 32000-2 §9.5–§9.10): how wide each glyph is, which characters it stands for, and the font's name.
//!
//! The reader supports what extraction needs and refuses the rest: simple fonts (Type 1, TrueType and Type 3) with their `Widths`, and composite fonts (Type 0) whose encoding is `Identity-H`, with their `W` and `DW`. A simple font without `Widths` (one of the standard 14 fonts, whose metrics the reader does not carry), vertical writing (`Identity-V`) and other CMaps are refused. Characters come from the font's `ToUnicode` map first, then, for simple fonts, from the `Differences` of its encoding and its base encoding; a glyph that none of them maps is kept with U+FFFD and counted. Font programs are never read.

use std::collections::BTreeMap;
use std::rc::Rc;

use super::cmap::{ToUnicode, displayable};
use super::encoding::BaseEncoding;
use super::file::{File, Value};
use super::glyph_names;
use super::object::{Dict, Object, Ref};
use super::{Error, Limit};
use crate::layout;

/// The widths of a composite font's CIDs: one width for a range, or one width per CID from the first.
#[derive(Clone, Debug)]
enum CidWidths {
    /// The same width for every CID of the range.
    Same(f64),
    /// One width for each CID from the start of the range.
    Each(Vec<f64>),
}

/// A simple font's `Differences`: codes to the characters of their glyph names.
type Differences = BTreeMap<u32, Box<str>>;

/// What kind of font, with what the kind needs.
#[derive(Clone, Debug)]
enum Kind {
    /// A simple font: one byte per code.
    Simple {
        first: u32,
        widths: Vec<f64>,
        missing: f64,
        /// Glyph space to text space: 1/1000, or a Type 3 font's `FontMatrix`.
        scale: f64,
        base: Option<BaseEncoding>,
        differences: Differences,
    },
    /// A composite font with `Identity-H`: two bytes per code, each code a CID.
    Composite {
        default: f64,
        /// Ranges of CIDs, by their first CID, with their last CID and widths.
        widths: BTreeMap<u32, (u32, CidWidths)>,
        cid_to_gid: CidToGid,
    },
}

/// How a CIDFont's CIDs become glyph identifiers.
#[derive(Clone, Debug)]
enum CidToGid {
    /// The CID is the glyph identifier.
    Identity,
    /// A table of glyph identifiers (`CIDToGIDMap` stream).
    Map(Vec<u16>),
    /// Only the font program knows (CFF-based CIDFonts).
    Unknown,
}

/// A loaded font.
#[derive(Clone, Debug)]
pub(crate) struct Font {
    /// The index of the font in the layout's `fonts`.
    pub(crate) output: u32,
    kind: Kind,
    to_unicode: Option<ToUnicode>,
}

/// What one character code shows.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GlyphInfo {
    /// The horizontal displacement in text space for a font size of 1 (w0).
    pub(crate) width: f64,
    /// The characters it stands for (U+FFFD when unknown).
    pub(crate) text: Box<str>,
    /// The glyph identifier, where the reader knows it.
    pub(crate) gid: Option<u32>,
    /// Whether it is the single-byte code 32, to which word spacing applies (§9.3.3).
    pub(crate) space: bool,
    /// Whether no map gave its characters.
    pub(crate) unmapped: bool,
}

impl Font {
    /// The character codes of a string shown with this font, with their lengths in bytes, one after the other: nothing is collected, however long the string.
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] for a string of a two-byte font with an odd number of bytes.
    pub(crate) fn codes<'s>(
        &self,
        bytes: &'s [u8],
    ) -> Result<impl Iterator<Item = (u32, u8)> + use<'s>, Error> {
        let length: u8 = match self.kind {
            Kind::Simple { .. } => 1,
            Kind::Composite { .. } => 2,
        };
        if !bytes.len().is_multiple_of(usize::from(length)) {
            return Err(Error::malformed(
                "a string shown with a two-byte font has an odd number of bytes",
            ));
        }
        Ok(bytes.chunks_exact(usize::from(length)).map(move |code| {
            let value = code
                .iter()
                .fold(0_u32, |value, &byte| (value << 8) | u32::from(byte));
            (value, length)
        }))
    }

    /// What the code `code` of `length` bytes shows.
    pub(crate) fn glyph(&self, code: u32, length: u8) -> GlyphInfo {
        let mapped = self
            .to_unicode
            .as_ref()
            .and_then(|map| map.get(length, code));
        let (width, fallback, gid) = match &self.kind {
            Kind::Simple {
                first,
                widths,
                missing,
                scale,
                base,
                differences,
            } => {
                let width = code
                    .checked_sub(*first)
                    .and_then(|index| usize::try_from(index).ok())
                    .and_then(|index| widths.get(index))
                    .copied()
                    .unwrap_or(*missing);
                let fallback = differences.get(&code).cloned().or_else(|| {
                    let byte = u8::try_from(code).ok()?;
                    base.and_then(|base| base.character(byte))
                        .map(|character| character.to_string().into_boxed_str())
                });
                (width * scale, fallback, None)
            }
            Kind::Composite {
                default,
                widths,
                cid_to_gid,
            } => {
                let width = widths
                    .range(..=code)
                    .next_back()
                    .and_then(|(&start, (last, widths))| {
                        if code > *last {
                            return None;
                        }
                        match widths {
                            CidWidths::Same(width) => Some(*width),
                            CidWidths::Each(each) => usize::try_from(code - start)
                                .ok()
                                .and_then(|index| each.get(index))
                                .copied(),
                        }
                    })
                    .unwrap_or(*default);
                let gid = match cid_to_gid {
                    CidToGid::Identity => Some(code),
                    CidToGid::Map(map) => Some(
                        usize::try_from(code)
                            .ok()
                            .and_then(|index| map.get(index))
                            .map_or(0, |&gid| u32::from(gid)),
                    ),
                    CidToGid::Unknown => None,
                };
                (width / 1000.0, None, gid)
            }
        };
        let text = mapped.map(Box::from).or(fallback);
        let unmapped = text
            .as_deref()
            .is_none_or(|text| text.contains(char::REPLACEMENT_CHARACTER));
        GlyphInfo {
            width,
            text: text.unwrap_or_else(|| char::REPLACEMENT_CHARACTER.to_string().into_boxed_str()),
            gid,
            space: length == 1 && code == 32,
            unmapped,
        }
    }
}

/// The fonts of a document: each font dictionary loaded once, and the list of fonts the layout names.
#[derive(Default)]
pub(crate) struct FontCache {
    by_reference: BTreeMap<Ref, Rc<Font>>,
    loaded: usize,
    entries_used: usize,
    output: Vec<layout::Font>,
    output_index: BTreeMap<(String, Option<String>), u32>,
}

impl FontCache {
    /// The font that `object` (a font dictionary or a reference to one) describes.
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] for a broken font, [`Error::Unsupported`] for fonts the reader does not support, and [`Error::Limit`] past [`super::Limits::max_fonts`] or [`super::Limits::max_font_entries`].
    pub(crate) fn font(&mut self, file: &mut File<'_>, object: &Object) -> Result<Rc<Font>, Error> {
        if let Object::Ref(reference) = object
            && let Some(font) = self.by_reference.get(reference)
        {
            return Ok(Rc::clone(font));
        }
        if self.loaded >= file.limits().max_fonts {
            return Err(Error::Limit(Limit::Fonts));
        }
        self.loaded += 1;
        let value = file.resolve(object)?;
        let dict = value
            .as_dict()
            .filter(|_| matches!(&*value, Object::Dict(_)))
            .ok_or_else(|| Error::malformed("a font is not a dictionary"))?;
        let font = Rc::new(self.load(file, dict)?);
        if let Object::Ref(reference) = object {
            self.by_reference.insert(*reference, Rc::clone(&font));
        }
        Ok(font)
    }

    /// The fonts the layout names, in the order they were first used.
    pub(crate) fn into_layout_fonts(self) -> Vec<layout::Font> {
        self.output
    }

    fn entries_left(&self, file: &File<'_>) -> usize {
        file.limits()
            .max_font_entries
            .saturating_sub(self.entries_used)
    }

    fn spend(&mut self, file: &File<'_>, count: usize) -> Result<(), Error> {
        if count > self.entries_left(file) {
            return Err(Error::Limit(Limit::FontEntries));
        }
        self.entries_used += count;
        Ok(())
    }

    fn load(&mut self, file: &mut File<'_>, dict: &Dict) -> Result<Font, Error> {
        if !dict.has_name(b"Type", b"Font") {
            return Err(Error::malformed("a font dictionary's `Type` is not `Font`"));
        }
        let subtype = dict
            .get(b"Subtype")
            .and_then(Object::as_name)
            .unwrap_or_default()
            .to_vec();
        let to_unicode = match file.lookup(dict, b"ToUnicode")? {
            None => None,
            Some(value) => {
                let stream = value
                    .as_stream()
                    .ok_or_else(|| Error::malformed("a font's `ToUnicode` is not a stream"))?;
                let data = file.stream_data(stream)?;
                let mut left = self.entries_left(file);
                let before = left;
                let map = ToUnicode::parse(&data, &mut left, file.limits().max_nesting)?;
                self.entries_used += before - left;
                Some(map)
            }
        };
        let (kind, name, family) = match subtype.as_slice() {
            b"Type1" | b"MMType1" | b"TrueType" => self.simple(file, dict, false)?,
            b"Type3" => self.simple(file, dict, true)?,
            b"Type0" => self.composite(file, dict)?,
            _ => return Err(Error::malformed("a font has an unknown `Subtype`")),
        };
        let output = self.output_font(name, family)?;
        Ok(Font {
            output,
            kind,
            to_unicode,
        })
    }

    /// The index of the layout font with this name and family, added if it is new.
    fn output_font(&mut self, name: String, family: Option<String>) -> Result<u32, Error> {
        let key = (name, family);
        if let Some(&index) = self.output_index.get(&key) {
            return Ok(index);
        }
        let index = u32::try_from(self.output.len()).map_err(|_| Error::Limit(Limit::Fonts))?;
        self.output.push(layout::Font {
            name: key.0.clone(),
            family: key.1.clone(),
        });
        self.output_index.insert(key, index);
        Ok(index)
    }

    /// A simple font: `FirstChar`, `LastChar`, `Widths`, the encoding, and for Type 3 the `FontMatrix`.
    fn simple(
        &mut self,
        file: &mut File<'_>,
        dict: &Dict,
        type3: bool,
    ) -> Result<(Kind, String, Option<String>), Error> {
        let Some(widths_value) = file.lookup(dict, b"Widths")? else {
            return Err(Error::unsupported(
                "a simple font without `Widths` (one of the standard 14 fonts, whose metrics the extractor does not carry)",
            ));
        };
        let widths_items = widths_value
            .as_array()
            .ok_or_else(|| Error::malformed("a font's `Widths` is not an array"))?;
        let first = integer_entry(file, dict, b"FirstChar")?;
        let last = integer_entry(file, dict, b"LastChar")?;
        let (Ok(first), Ok(last)) = (u32::try_from(first), u32::try_from(last)) else {
            return Err(Error::malformed(
                "a font's `FirstChar` or `LastChar` is negative",
            ));
        };
        if first > 255
            || last > 255
            || last < first
            || usize::try_from(last - first + 1).ok() != Some(widths_items.len())
        {
            return Err(Error::malformed(
                "a font's `Widths` do not match its `FirstChar` and `LastChar`",
            ));
        }
        self.spend(file, widths_items.len())?;
        let mut widths = Vec::with_capacity(widths_items.len());
        for item in widths_items {
            widths.push(number(file, item, "a font's `Widths`")?);
        }
        let descriptor = descriptor(file, dict)?;
        let descriptor = descriptor.as_ref().and_then(Value::dict);
        let missing = match descriptor {
            Some(descriptor) => match descriptor.get(b"MissingWidth") {
                Some(width) => number(file, width, "a font's `MissingWidth`")?,
                None => 0.0,
            },
            None => 0.0,
        };
        let scale = if type3 {
            font_matrix_scale(file, dict)?
        } else {
            0.001
        };
        let (base, differences) = self.encoding(file, dict)?;
        let name = match dict.get(b"BaseFont").or_else(|| dict.get(b"Name")) {
            Some(Object::Name(name)) => clean_name(name),
            Some(_) => return Err(Error::malformed("a font's name is not a name")),
            None if type3 => "Type3".to_owned(),
            None => return Err(Error::malformed("a font has no `BaseFont`")),
        };
        let family = family(file, descriptor)?;
        Ok((
            Kind::Simple {
                first,
                widths,
                missing,
                scale,
                base,
                differences,
            },
            name,
            family,
        ))
    }

    /// A simple font's encoding: its base encoding and `Differences`.
    fn encoding(
        &mut self,
        file: &mut File<'_>,
        dict: &Dict,
    ) -> Result<(Option<BaseEncoding>, Differences), Error> {
        let Some(value) = file.lookup(dict, b"Encoding")? else {
            return Ok((None, BTreeMap::new()));
        };
        match &*value {
            Object::Name(name) => Ok((Some(base_encoding(name)?), BTreeMap::new())),
            Object::Dict(encoding) => {
                let base = match encoding.get(b"BaseEncoding") {
                    None => None,
                    Some(Object::Name(name)) => Some(base_encoding(name)?),
                    Some(_) => {
                        return Err(Error::malformed(
                            "an encoding's `BaseEncoding` is not a name",
                        ));
                    }
                };
                let mut differences = BTreeMap::new();
                if let Some(list) = file.lookup(encoding, b"Differences")? {
                    let list = list.as_array().ok_or_else(|| {
                        Error::malformed("an encoding's `Differences` is not an array")
                    })?;
                    self.spend(file, list.len())?;
                    let mut code: Option<u32> = None;
                    for item in list {
                        match item {
                            Object::Integer(value) => {
                                code = Some(u32::try_from(*value).ok().filter(|&value| value <= 255).ok_or_else(|| {
                                    Error::malformed("an encoding's `Differences` has a code out of range")
                                })?);
                            }
                            Object::Name(name) => {
                                let Some(current) = code else {
                                    return Err(Error::malformed(
                                        "an encoding's `Differences` starts with a name",
                                    ));
                                };
                                // Codes are single bytes: a run of names may not count past 255.
                                if current > 255 {
                                    return Err(Error::malformed(
                                        "an encoding's `Differences` runs past code 255",
                                    ));
                                }
                                let text = glyph_names::characters(name)
                                    .filter(|text| text.chars().all(displayable))
                                    .unwrap_or_else(|| char::REPLACEMENT_CHARACTER.to_string());
                                differences.insert(current, text.into_boxed_str());
                                code = current.checked_add(1);
                            }
                            _ => {
                                return Err(Error::malformed(
                                    "an encoding's `Differences` holds something other than codes and names",
                                ));
                            }
                        }
                    }
                }
                Ok((base, differences))
            }
            _ => Err(Error::malformed(
                "a font's `Encoding` is not a name or a dictionary",
            )),
        }
    }

    /// A Type 0 font with `Identity-H` and its CIDFont.
    fn composite(
        &mut self,
        file: &mut File<'_>,
        dict: &Dict,
    ) -> Result<(Kind, String, Option<String>), Error> {
        match file.lookup(dict, b"Encoding")?.as_deref() {
            Some(Object::Name(name)) if name == b"Identity-H" => {}
            Some(Object::Name(name)) if name == b"Identity-V" => {
                return Err(Error::unsupported(
                    "vertical writing (a font encoded with `Identity-V`)",
                ));
            }
            Some(_) => {
                return Err(Error::unsupported(
                    "a composite font with an encoding other than `Identity-H`",
                ));
            }
            None => return Err(Error::malformed("a composite font has no `Encoding`")),
        }
        let descendants = file.lookup(dict, b"DescendantFonts")?;
        let Some([descendant]) = descendants.as_ref().and_then(|value| value.as_array()) else {
            return Err(Error::malformed(
                "a composite font does not have exactly one descendant font",
            ));
        };
        let descendant = file.resolve(descendant)?;
        let Some(cid_font) = descendant.dict() else {
            return Err(Error::malformed(
                "a composite font's descendant is not a dictionary",
            ));
        };
        let type2 = if cid_font.has_name(b"Subtype", b"CIDFontType2") {
            true
        } else if cid_font.has_name(b"Subtype", b"CIDFontType0") {
            false
        } else {
            return Err(Error::malformed(
                "a composite font's descendant is not a CIDFont",
            ));
        };
        let default = match cid_font.get(b"DW") {
            Some(width) => number(file, width, "a CIDFont's `DW`")?,
            None => 1000.0,
        };
        let widths = match file.lookup(cid_font, b"W")? {
            None => BTreeMap::new(),
            Some(value) => {
                let items = value
                    .as_array()
                    .ok_or_else(|| Error::malformed("a CIDFont's `W` is not an array"))?;
                self.cid_widths(file, items)?
            }
        };
        let cid_to_gid = if type2 {
            match file.lookup(cid_font, b"CIDToGIDMap")?.as_deref() {
                None => CidToGid::Identity,
                Some(Object::Name(name)) if name == b"Identity" => CidToGid::Identity,
                Some(Object::Stream(stream)) => {
                    let data = file.stream_data(stream)?;
                    if !data.len().is_multiple_of(2) {
                        return Err(Error::malformed(
                            "a `CIDToGIDMap` has an odd number of bytes",
                        ));
                    }
                    self.spend(file, data.len() / 2)?;
                    CidToGid::Map(
                        data.as_chunks::<2>()
                            .0
                            .iter()
                            .map(|&pair| u16::from_be_bytes(pair))
                            .collect(),
                    )
                }
                Some(_) => {
                    return Err(Error::malformed(
                        "a `CIDToGIDMap` is not `Identity` or a stream",
                    ));
                }
            }
        } else {
            CidToGid::Unknown
        };
        let name = match cid_font.get(b"BaseFont").or_else(|| dict.get(b"BaseFont")) {
            Some(Object::Name(name)) => clean_name(name),
            _ => return Err(Error::malformed("a composite font has no `BaseFont`")),
        };
        let descriptor = descriptor(file, cid_font)?;
        let family = family(file, descriptor.as_ref().and_then(Value::dict))?;
        Ok((
            Kind::Composite {
                default,
                widths,
                cid_to_gid,
            },
            name,
            family,
        ))
    }

    /// A CIDFont's `W` array (§9.7.4.3): `c [w1 w2 …]` and `c_first c_last w`.
    fn cid_widths(
        &mut self,
        file: &mut File<'_>,
        items: &[Object],
    ) -> Result<BTreeMap<u32, (u32, CidWidths)>, Error> {
        let malformed = || Error::malformed("a CIDFont's `W` array is malformed");
        let cid = |object: &Object| {
            object
                .as_integer()
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(malformed)
        };
        let mut ranges: BTreeMap<u32, (u32, CidWidths)> = BTreeMap::new();
        let mut index = 0;
        while index < items.len() {
            let first = cid(&items[index])?;
            let (last, widths) = match items.get(index + 1) {
                Some(Object::Array(list)) => {
                    self.spend(file, list.len().max(1))?;
                    let mut widths = Vec::with_capacity(list.len());
                    for item in list {
                        widths.push(number(file, item, "a CIDFont's `W`")?);
                    }
                    let count = u32::try_from(widths.len()).map_err(|_| malformed())?;
                    let last = first
                        .checked_add(count.checked_sub(1).ok_or_else(malformed)?)
                        .ok_or_else(malformed)?;
                    index += 2;
                    (last, CidWidths::Each(widths))
                }
                Some(Object::Ref(reference)) => {
                    let list = file.get(*reference)?;
                    let Object::Array(list) = &*list else {
                        return Err(malformed());
                    };
                    self.spend(file, list.len().max(1))?;
                    let mut widths = Vec::with_capacity(list.len());
                    for item in list {
                        widths.push(number(file, item, "a CIDFont's `W`")?);
                    }
                    let count = u32::try_from(widths.len()).map_err(|_| malformed())?;
                    let last = first
                        .checked_add(count.checked_sub(1).ok_or_else(malformed)?)
                        .ok_or_else(malformed)?;
                    index += 2;
                    (last, CidWidths::Each(widths))
                }
                Some(last) => {
                    let last = cid(last)?;
                    let width = number(
                        file,
                        items.get(index + 2).ok_or_else(malformed)?,
                        "a CIDFont's `W`",
                    )?;
                    if last < first {
                        return Err(malformed());
                    }
                    self.spend(file, 1)?;
                    index += 3;
                    (last, CidWidths::Same(width))
                }
                None => return Err(malformed()),
            };
            // Ranges must not overlap: the format does not say which width would win.
            let overlaps_before = ranges
                .range(..=first)
                .next_back()
                .is_some_and(|(_, (end, _))| *end >= first);
            let overlaps_after = ranges
                .range(first..)
                .next()
                .is_some_and(|(&start, _)| start <= last);
            if overlaps_before || overlaps_after {
                return Err(Error::malformed("a CIDFont's `W` gives a CID two widths"));
            }
            ranges.insert(first, (last, widths));
        }
        Ok(ranges)
    }
}

/// A predefined encoding by name.
fn base_encoding(name: &[u8]) -> Result<BaseEncoding, Error> {
    if name == b"MacExpertEncoding" {
        return Err(Error::unsupported("MacExpertEncoding"));
    }
    BaseEncoding::from_name(name)
        .ok_or_else(|| Error::malformed("a font names an unknown encoding"))
}

/// An integer entry of a font dictionary.
fn integer_entry(file: &mut File<'_>, dict: &Dict, key: &[u8]) -> Result<i64, Error> {
    file.lookup(dict, key)?
        .and_then(|value| value.as_integer())
        .ok_or_else(|| {
            Error::malformed("a font's `FirstChar` or `LastChar` is missing or not an integer")
        })
}

/// A finite number, resolved.
pub(crate) fn number(file: &mut File<'_>, object: &Object, what: &str) -> Result<f64, Error> {
    file.resolve(object)?
        .as_number()
        .filter(|value| value.is_finite())
        .ok_or_else(|| Error::malformed(format!("{what} holds something other than a number")))
}

/// A Type 3 font's horizontal scale from glyph space to text space: `FontMatrix` must neither rotate nor skew nor mirror.
fn font_matrix_scale(file: &mut File<'_>, dict: &Dict) -> Result<f64, Error> {
    let matrix = file
        .lookup(dict, b"FontMatrix")?
        .and_then(|value| value.as_array().map(<[Object]>::to_vec))
        .filter(|items| items.len() == 6)
        .ok_or_else(|| Error::malformed("a Type 3 font has no `FontMatrix` of six numbers"))?;
    let mut values = [0.0; 6];
    for (slot, item) in values.iter_mut().zip(&matrix) {
        *slot = number(file, item, "a Type 3 font's `FontMatrix`")?;
    }
    let [a, b, c, d, _, _] = values;
    if b != 0.0 || c != 0.0 || a <= 0.0 || d <= 0.0 {
        return Err(Error::unsupported(
            "a Type 3 font whose `FontMatrix` rotates, skews or mirrors its glyphs",
        ));
    }
    Ok(a)
}

/// A font's descriptor, if it has one, borrowed (read it with [`Value::dict`]).
fn descriptor<'d>(file: &mut File<'_>, dict: &'d Dict) -> Result<Option<Value<'d>>, Error> {
    match file.lookup(dict, b"FontDescriptor")? {
        None => Ok(None),
        Some(value) if value.dict().is_some() => Ok(Some(value)),
        Some(_) => Err(Error::malformed(
            "a font's `FontDescriptor` is not a dictionary",
        )),
    }
}

/// The family a font descriptor gives in `FontFamily`.
fn family(file: &mut File<'_>, descriptor: Option<&Dict>) -> Result<Option<String>, Error> {
    let Some(descriptor) = descriptor else {
        return Ok(None);
    };
    match file.lookup(descriptor, b"FontFamily")? {
        None => Ok(None),
        Some(value) => match &*value {
            Object::String(bytes) => Ok(Some(clean_label(&text_string(bytes))))
                .map(|family| family.filter(|family| !family.is_empty())),
            _ => Err(Error::malformed(
                "a font descriptor's `FontFamily` is not a string",
            )),
        },
    }
}

/// A font's name without its subset tag (six capital letters and `+`, §9.6.4), as printable text.
fn clean_name(name: &[u8]) -> String {
    let name = match name.get(..7) {
        Some([tag @ .., b'+']) if tag.iter().all(u8::is_ascii_uppercase) => &name[7..],
        _ => name,
    };
    let name = clean_label(
        &name
            .iter()
            .map(|&byte| char::from(byte))
            .collect::<String>(),
    );
    if name.is_empty() {
        "untitled".to_owned()
    } else {
        name
    }
}

/// A PDF text string (§7.9.2.2): UTF-16BE after its byte order mark, UTF-8 after its own, and otherwise one character per byte.
fn text_string(bytes: &[u8]) -> String {
    if let Some(utf16) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = utf16
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| u16::from_be_bytes(pair))
            .collect();
        return char::decode_utf16(units)
            .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect();
    }
    if let Some(utf8) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(utf8).into_owned();
    }
    bytes.iter().map(|&byte| char::from(byte)).collect()
}

/// A label for the layout: control characters become U+FFFD, and at most [`layout::MAX_LABEL_CHARS`] characters are kept.
fn clean_label(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() {
                char::REPLACEMENT_CHARACTER
            } else {
                character
            }
        })
        .take(layout::MAX_LABEL_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_names() {
        assert_eq!(clean_name(b"ABCDEF+Calibri-Bold"), "Calibri-Bold");
        assert_eq!(clean_name(b"Calibri,Bold"), "Calibri,Bold");
        assert_eq!(clean_name(b"ABCDEf+Calibri"), "ABCDEf+Calibri");
        assert_eq!(clean_name(b"ABCDEF+"), "untitled");
        assert_eq!(clean_name(b"Bad\x01Name"), "Bad\u{FFFD}Name");
        assert_eq!(
            clean_name(&[b'a'; 300]).chars().count(),
            layout::MAX_LABEL_CHARS
        );
        assert_eq!(text_string(b"\xFE\xFF\x00C\x00a"), "Ca");
        assert_eq!(text_string(b"Cambria"), "Cambria");
    }
}
