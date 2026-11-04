//! Content streams (ISO 32000-2 §7.8, §8 and §9): the operators that place glyphs, images and rules on a page.
//!
//! The interpreter keeps the graphics state that positions things (the current transformation matrix, the text state, the line width) and ignores what only colours them. Every glyph's origin is the point (0, rise) of text space taken through the text matrix, the current transformation matrix and the page's own matrix, which turns PDF's user space (y up, from the media box's corner) into the layout's page space (y down, from the top left corner of the page as displayed, after its rotation). After each glyph the text matrix moves by the glyph's displacement, `((w0 × Tfs) + Tc + Tw) × Th` (§9.4.4), and each number of a `TJ` array moves it by `-(n / 1000) × Tfs × Th`.
//!
//! Glyphs that are not upright on the page (rotated, mirrored or vertical text) and invisible glyphs (render modes 3 and 7) are counted and not placed. Images (image XObjects and inline images) and rules (horizontal and vertical stroked segments, and thin filled rectangles) become elements, by their boxes. Every operator of PDF 2.0 is known; any other is refused, unless it stands between `BX` and `EX`.

use std::rc::Rc;

use super::file::File;
use super::font::{Font, FontCache, number};
use super::lexer::{Lexer, Token, is_whitespace};
use super::object::{Dict, Object, parse_from};
use super::pages::PageInfo;
use super::{Counters, Error, Limit};
use crate::layout::{Element, ElementKind, Points};

/// A filled rectangle is a rule if its shorter side is at most this many points (Word's thickest border is 6 pt) and its longer side at least twice as long.
const MAX_RULE_THICKNESS: f64 = 6.0;

/// How far from exact a baseline may lean, relative to its length, and still be upright.
const UPRIGHT_TOLERANCE: f64 = 1e-6;

/// A glyph placed on the page, before it is put on a line.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RawGlyph {
    /// The x of its origin, in points on the displayed page.
    pub(crate) x: f64,
    /// The y of its origin.
    pub(crate) y: f64,
    /// Its displacement along the baseline.
    pub(crate) advance: f64,
    /// Its size: the height of its em square.
    pub(crate) size: f64,
    /// Its characters.
    pub(crate) text: Box<str>,
    /// The index of its font in the layout's fonts.
    pub(crate) font: u32,
    /// Its glyph identifier, if known.
    pub(crate) gid: Option<u32>,
    /// Whether its characters are unknown (its text is U+FFFD).
    pub(crate) unmapped: bool,
}

/// What a page draws.
#[derive(Debug, Default)]
pub(crate) struct Drawn {
    /// The upright, visible glyphs, in drawing order.
    pub(crate) glyphs: Vec<RawGlyph>,
    /// Images and rules, in drawing order.
    pub(crate) elements: Vec<Element>,
    /// Glyphs not placed: not upright, or invisible.
    pub(crate) skipped: u64,
    /// The width of the page as displayed.
    pub(crate) width: f64,
    /// Its height as displayed.
    pub(crate) height: f64,
}

/// An affine transformation `[a b c d e f]`: x' = a·x + c·y + e, y' = b·x + d·y + f (§8.3.3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Matrix {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
}

impl Matrix {
    const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    const fn new([a, b, c, d, e, f]: [f64; 6]) -> Self {
        Self { a, b, c, d, e, f }
    }

    const fn translate(x: f64, y: f64) -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: x,
            f: y,
        }
    }

    /// `self` followed by `other`.
    fn then(self, other: Self) -> Self {
        Self {
            a: self.a * other.a + self.b * other.c,
            b: self.a * other.b + self.b * other.d,
            c: self.c * other.a + self.d * other.c,
            d: self.c * other.b + self.d * other.d,
            e: self.e * other.a + self.f * other.c + other.e,
            f: self.e * other.b + self.f * other.d + other.f,
        }
    }

    fn apply(self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    fn is_finite(self) -> bool {
        [self.a, self.b, self.c, self.d, self.e, self.f]
            .iter()
            .all(|value| value.is_finite())
    }

    /// The matrix from PDF's user space to the displayed page (§8.3.2.3 and the page's `Rotate`).
    fn page(page: &PageInfo) -> (Self, f64, f64) {
        let box_ = page.visible;
        let (width, height) = (box_.x1 - box_.x0, box_.y1 - box_.y0);
        match page.rotate {
            90 => (
                Self::new([0.0, 1.0, 1.0, 0.0, -box_.y0, -box_.x0]),
                height,
                width,
            ),
            180 => (
                Self::new([-1.0, 0.0, 0.0, 1.0, box_.x1, -box_.y0]),
                width,
                height,
            ),
            270 => (
                Self::new([0.0, -1.0, -1.0, 0.0, box_.y1, box_.x1]),
                height,
                width,
            ),
            _ => (
                Self::new([1.0, 0.0, 0.0, -1.0, -box_.x0, box_.y1]),
                width,
                height,
            ),
        }
    }
}

/// The parts of the graphics state that place things.
#[derive(Clone, Debug)]
struct State {
    ctm: Matrix,
    char_spacing: f64,
    word_spacing: f64,
    /// Horizontal scaling as a factor (`Tz` / 100).
    scale: f64,
    leading: f64,
    rise: f64,
    font: Option<Rc<Font>>,
    size: f64,
    render: i64,
    line_width: f64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            ctm: Matrix::IDENTITY,
            char_spacing: 0.0,
            word_spacing: 0.0,
            scale: 1.0,
            leading: 0.0,
            rise: 0.0,
            font: None,
            size: 0.0,
            render: 0,
            line_width: 1.0,
        }
    }
}

/// A segment of the current path, in page space.
#[derive(Clone, Copy, Debug)]
enum Segment {
    Line((f64, f64), (f64, f64)),
    Curve,
}

/// The current path.
#[derive(Debug, Default)]
struct Path {
    subpaths: Vec<Vec<Segment>>,
    start: Option<(f64, f64)>,
    current: Option<(f64, f64)>,
    segments: usize,
}

/// Interprets the content streams of `page`.
///
/// # Errors
///
/// An [`Error`] for anything the interpreter refuses; the caller adds the page number.
pub(crate) fn interpret_page(
    file: &mut File<'_>,
    page: &PageInfo,
    fonts: &mut FontCache,
    counters: &mut Counters,
) -> Result<Drawn, Error> {
    let (matrix, width, height) = Matrix::page(page);
    let resources = match &page.resources {
        Some(object) => Some(dict_of(file, object, "the page's resources")?),
        None => None,
    };
    let data = page_content(file, page)?;
    let mut interpreter = Interpreter {
        file,
        fonts,
        counters,
        page: matrix,
        state: State::default(),
        saved: Vec::new(),
        floor: 0,
        text: None,
        path: Path::default(),
        drawn: Drawn {
            width,
            height,
            ..Drawn::default()
        },
        form_depth: 0,
        marked: 0,
        compatibility: 0,
    };
    interpreter.run(&data, resources.as_ref())?;
    Ok(interpreter.drawn)
}

/// The page's content streams, joined by line feeds (§7.8.2: a page's streams form one stream, split only between tokens).
fn page_content(file: &mut File<'_>, page: &PageInfo) -> Result<Vec<u8>, Error> {
    let dict = page
        .dict
        .as_dict()
        .ok_or_else(|| Error::malformed("the page is not a dictionary"))?;
    let Some(contents) = dict.get(b"Contents") else {
        return Ok(Vec::new());
    };
    let contents = (*file.resolve(contents)?).clone();
    let streams = match contents {
        Object::Stream(stream) => vec![Object::Stream(stream)],
        Object::Array(items) => items,
        Object::Null => Vec::new(),
        _ => {
            return Err(Error::malformed(
                "the page's `Contents` is not a stream or an array of streams",
            ));
        }
    };
    let mut data = Vec::new();
    for item in &streams {
        let item = file.resolve(item)?;
        let stream = item
            .as_stream()
            .ok_or_else(|| {
                Error::malformed("the page's `Contents` holds something other than a stream")
            })?
            .clone();
        drop(item);
        let decoded = file.stream_data(&stream)?;
        if data.len() + decoded.len() + 1 > file.limits().max_stream_bytes {
            return Err(Error::Limit(Limit::StreamSize));
        }
        data.extend_from_slice(&decoded);
        data.push(b'\n');
    }
    Ok(data)
}

/// A dictionary, resolved and copied.
fn dict_of(file: &mut File<'_>, object: &Object, what: &str) -> Result<Dict, Error> {
    match &*file.resolve(object)? {
        Object::Dict(dict) => Ok(dict.clone()),
        _ => Err(Error::malformed(format!("{what} is not a dictionary"))),
    }
}

struct Interpreter<'i, 'a> {
    file: &'i mut File<'a>,
    fonts: &'i mut FontCache,
    counters: &'i mut Counters,
    page: Matrix,
    state: State,
    saved: Vec<State>,
    /// The depth of `saved` below which a `Q` may not go (the state saved around the form XObject being drawn).
    floor: usize,
    /// The text matrix and the text line matrix, inside `BT` … `ET`.
    text: Option<(Matrix, Matrix)>,
    path: Path,
    drawn: Drawn,
    form_depth: usize,
    marked: usize,
    compatibility: usize,
}

impl Interpreter<'_, '_> {
    fn run(&mut self, data: &[u8], resources: Option<&Dict>) -> Result<(), Error> {
        let max_nesting = self.file.limits().max_nesting;
        let mut lexer = Lexer::new(data, 0);
        let mut operands: Vec<Object> = Vec::new();
        while let Some(token) = lexer.next_token()? {
            let operator = match token {
                Token::Keyword(keyword) if !matches!(keyword, b"true" | b"false" | b"null") => {
                    keyword
                }
                other => {
                    operands.push(parse_from(other, &mut lexer, false, max_nesting, 0)?);
                    continue;
                }
            };
            self.counters.operators += 1;
            if self.counters.operators > self.file.limits().max_operators {
                return Err(Error::Limit(Limit::Operators));
            }
            self.operator(operator, &operands, &mut lexer, resources)?;
            operands.clear();
        }
        if !operands.is_empty() {
            return Err(Error::malformed(
                "a content stream ends with operands that no operator takes",
            ));
        }
        Ok(())
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per operator of PDF 2.0 (ISO 32000-2 Annex A) reads best as a single table"
    )]
    fn operator(
        &mut self,
        operator: &[u8],
        operands: &[Object],
        lexer: &mut Lexer<'_>,
        resources: Option<&Dict>,
    ) -> Result<(), Error> {
        match operator {
            // General graphics state.
            b"w" => self.state.line_width = numbers::<1>(operands, "w")?[0],
            b"J" | b"j" => integer(operands, operator_name(operator)).map(|_| ())?,
            b"M" | b"i" => {
                numbers::<1>(operands, operator_name(operator))?;
            }
            b"d" => match operands {
                [Object::Array(_), phase] if phase.as_number().is_some() => {}
                _ => return Err(operands_error("d")),
            },
            b"ri" => name(operands, "ri").map(|_| ())?,
            b"gs" => {
                let key = name(operands, "gs")?;
                self.ext_g_state(&key, resources)?;
            }
            // Special graphics state.
            b"q" => {
                none(operands, "q")?;
                if self.saved.len() >= self.file.limits().max_graphics_depth {
                    return Err(Error::Limit(Limit::GraphicsDepth));
                }
                self.saved.push(self.state.clone());
            }
            b"Q" => {
                none(operands, "Q")?;
                if self.saved.len() <= self.floor {
                    return Err(Error::malformed(
                        "`Q` restores a graphics state that was not saved",
                    ));
                }
                self.state = self
                    .saved
                    .pop()
                    .ok_or_else(|| Error::malformed("`Q` without `q`"))?;
            }
            b"cm" => {
                let matrix = Matrix::new(numbers::<6>(operands, "cm")?);
                self.state.ctm = matrix.then(self.state.ctm);
                if !self.state.ctm.is_finite() {
                    return Err(Error::malformed(
                        "the transformation matrix is out of range",
                    ));
                }
            }
            // Path construction.
            b"m" => {
                let [x, y] = numbers::<2>(operands, "m")?;
                let point = self.to_page(x, y);
                self.path.subpaths.push(Vec::new());
                self.path.start = Some(point);
                self.path.current = Some(point);
            }
            b"l" => {
                let [x, y] = numbers::<2>(operands, "l")?;
                let point = self.to_page(x, y);
                let current = self.current_point("l")?;
                self.add_segment(Segment::Line(current, point))?;
                self.path.current = Some(point);
            }
            b"c" | b"v" | b"y" => {
                let end = match operator {
                    b"c" => {
                        let [_, _, _, _, x, y] = numbers::<6>(operands, "c")?;
                        (x, y)
                    }
                    _ => {
                        let [_, _, x, y] = numbers::<4>(operands, operator_name(operator))?;
                        (x, y)
                    }
                };
                self.current_point(operator_name(operator))?;
                self.add_segment(Segment::Curve)?;
                self.path.current = Some(self.to_page(end.0, end.1));
            }
            b"h" => {
                none(operands, "h")?;
                if let (Some(current), Some(start)) = (self.path.current, self.path.start) {
                    if current != start {
                        self.add_segment(Segment::Line(current, start))?;
                    }
                    self.path.current = Some(start);
                }
            }
            b"re" => {
                let [x, y, width, height] = numbers::<4>(operands, "re")?;
                let corners = [
                    self.to_page(x, y),
                    self.to_page(x + width, y),
                    self.to_page(x + width, y + height),
                    self.to_page(x, y + height),
                ];
                self.path.subpaths.push(Vec::new());
                for index in 0..4 {
                    self.add_segment(Segment::Line(corners[index], corners[(index + 1) % 4]))?;
                }
                self.path.start = Some(corners[0]);
                self.path.current = Some(corners[0]);
            }
            // Path painting.
            b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" => {
                none(operands, operator_name(operator))?;
                if matches!(operator, b"s" | b"b" | b"b*") {
                    self.operator(b"h", &[], lexer, resources)?;
                }
                let fill = matches!(operator, b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*");
                let stroke = matches!(operator, b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*");
                self.paint(fill, stroke)?;
            }
            b"W" | b"W*" => none(operands, operator_name(operator))?,
            // Text objects.
            b"BT" => {
                none(operands, "BT")?;
                if self.text.is_some() {
                    return Err(Error::malformed("`BT` inside a text object"));
                }
                self.text = Some((Matrix::IDENTITY, Matrix::IDENTITY));
            }
            b"ET" => {
                none(operands, "ET")?;
                if self.text.take().is_none() {
                    return Err(Error::malformed("`ET` outside a text object"));
                }
            }
            // Text state.
            b"Tc" => self.state.char_spacing = numbers::<1>(operands, "Tc")?[0],
            b"Tw" => self.state.word_spacing = numbers::<1>(operands, "Tw")?[0],
            b"Tz" => self.state.scale = numbers::<1>(operands, "Tz")?[0] / 100.0,
            b"TL" => self.state.leading = numbers::<1>(operands, "TL")?[0],
            b"Ts" => self.state.rise = numbers::<1>(operands, "Ts")?[0],
            b"Tr" => {
                let mode = integer(operands, "Tr")?;
                if !(0..=7).contains(&mode) {
                    return Err(Error::malformed("`Tr` sets an unknown render mode"));
                }
                self.state.render = mode;
            }
            b"Tf" => match operands {
                [Object::Name(font), size] => {
                    let size = size
                        .as_number()
                        .filter(|size| size.is_finite())
                        .ok_or_else(|| operands_error("Tf"))?;
                    let font = self.font(font, resources)?;
                    self.state.font = Some(font);
                    self.state.size = size;
                }
                _ => return Err(operands_error("Tf")),
            },
            // Text positioning.
            b"Td" | b"TD" => {
                let [x, y] = numbers::<2>(operands, operator_name(operator))?;
                if operator == b"TD" {
                    self.state.leading = -y;
                }
                self.move_line(x, y)?;
            }
            b"Tm" => {
                let matrix = Matrix::new(numbers::<6>(operands, "Tm")?);
                let text = self.text.as_mut().ok_or_else(|| outside_text("Tm"))?;
                *text = (matrix, matrix);
            }
            b"T*" => {
                none(operands, "T*")?;
                self.move_line(0.0, -self.state.leading)?;
            }
            // Text showing.
            b"Tj" => match operands {
                [Object::String(string)] => self.show(string)?,
                _ => return Err(operands_error("Tj")),
            },
            b"'" => match operands {
                [Object::String(string)] => {
                    self.move_line(0.0, -self.state.leading)?;
                    self.show(string)?;
                }
                _ => return Err(operands_error("'")),
            },
            b"\"" => match operands {
                [word, character, Object::String(string)] => {
                    let (Some(word), Some(character)) = (word.as_number(), character.as_number())
                    else {
                        return Err(operands_error("\""));
                    };
                    self.state.word_spacing = word;
                    self.state.char_spacing = character;
                    self.move_line(0.0, -self.state.leading)?;
                    self.show(string)?;
                }
                _ => return Err(operands_error("\"")),
            },
            b"TJ" => match operands {
                [Object::Array(items)] => {
                    for item in items {
                        match item {
                            Object::String(string) => self.show(string)?,
                            Object::Integer(_) | Object::Real(_) => {
                                let adjustment =
                                    item.as_number().ok_or_else(|| operands_error("TJ"))?;
                                let tx =
                                    -(adjustment / 1000.0) * self.state.size * self.state.scale;
                                self.advance(tx)?;
                            }
                            _ => return Err(operands_error("TJ")),
                        }
                    }
                }
                _ => return Err(operands_error("TJ")),
            },
            // Type 3 glyph procedures only.
            b"d0" | b"d1" => {
                return Err(Error::malformed(
                    "a Type 3 glyph operator (`d0` or `d1`) in a page's content",
                ));
            }
            // Colour.
            b"CS" | b"cs" => name(operands, operator_name(operator)).map(|_| ())?,
            b"G" | b"g" => {
                numbers::<1>(operands, operator_name(operator))?;
            }
            b"RG" | b"rg" => {
                numbers::<3>(operands, operator_name(operator))?;
            }
            b"K" | b"k" => {
                numbers::<4>(operands, operator_name(operator))?;
            }
            b"SC" | b"sc" | b"SCN" | b"scn" => {
                let pattern = matches!(operator, b"SCN" | b"scn");
                let (last, rest) = operands
                    .split_last()
                    .ok_or_else(|| operands_error(operator_name(operator)))?;
                let numbers_only = if pattern && matches!(last, Object::Name(_)) {
                    rest
                } else {
                    operands
                };
                if numbers_only.len() > 32
                    || numbers_only
                        .iter()
                        .any(|operand| operand.as_number().is_none())
                {
                    return Err(operands_error(operator_name(operator)));
                }
            }
            // Shading.
            b"sh" => name(operands, "sh").map(|_| ())?,
            // Inline images.
            b"BI" => {
                none(operands, "BI")?;
                self.inline_image(lexer)?;
            }
            b"ID" | b"EI" => return Err(Error::malformed("an inline image operator outside `BI`")),
            // XObjects.
            b"Do" => {
                let key = name(operands, "Do")?;
                self.x_object(&key, resources)?;
            }
            // Marked content.
            b"MP" | b"BMC" => {
                name(operands, operator_name(operator))?;
                if operator == b"BMC" {
                    self.begin_marked()?;
                }
            }
            b"DP" | b"BDC" => {
                match operands {
                    [Object::Name(_), Object::Dict(_) | Object::Name(_)] => {}
                    _ => return Err(operands_error(operator_name(operator))),
                }
                if operator == b"BDC" {
                    self.begin_marked()?;
                }
            }
            b"EMC" => {
                none(operands, "EMC")?;
                self.marked = self
                    .marked
                    .checked_sub(1)
                    .ok_or_else(|| Error::malformed("`EMC` without `BMC` or `BDC`"))?;
            }
            // Compatibility sections.
            b"BX" => {
                none(operands, "BX")?;
                if self.compatibility >= self.file.limits().max_graphics_depth {
                    return Err(Error::Limit(Limit::GraphicsDepth));
                }
                self.compatibility += 1;
            }
            b"EX" => {
                none(operands, "EX")?;
                self.compatibility = self
                    .compatibility
                    .checked_sub(1)
                    .ok_or_else(|| Error::malformed("`EX` without `BX`"))?;
            }
            // Inside `BX` … `EX`, unknown operators are ignored (§7.8.2).
            _ if self.compatibility > 0 => {}
            _ => return Err(Error::malformed("an operator that PDF 2.0 does not define")),
        }
        Ok(())
    }

    fn begin_marked(&mut self) -> Result<(), Error> {
        if self.marked >= self.file.limits().max_graphics_depth {
            return Err(Error::Limit(Limit::GraphicsDepth));
        }
        self.marked += 1;
        Ok(())
    }

    /// A point of user space in page space.
    fn to_page(&self, x: f64, y: f64) -> (f64, f64) {
        self.state.ctm.then(self.page).apply(x, y)
    }

    fn current_point(&self, operator: &str) -> Result<(f64, f64), Error> {
        self.path
            .current
            .ok_or_else(|| Error::malformed(format!("`{operator}` without a current point")))
    }

    fn add_segment(&mut self, segment: Segment) -> Result<(), Error> {
        if self.path.segments >= self.file.limits().max_path_segments {
            return Err(Error::Limit(Limit::PathSegments));
        }
        self.path.segments += 1;
        match self.path.subpaths.last_mut() {
            Some(subpath) => subpath.push(segment),
            None => self.path.subpaths.push(vec![segment]),
        }
        Ok(())
    }

    /// Turns the painted path into rules.
    fn paint(&mut self, fill: bool, stroke: bool) -> Result<(), Error> {
        let path = core::mem::take(&mut self.path);
        let linear = self.state.ctm.then(self.page);
        let thickness =
            self.state.line_width.abs() * (linear.a * linear.d - linear.b * linear.c).abs().sqrt();
        if fill && let Some(rule) = thin_rectangle(&path) {
            let half = if stroke { thickness / 2.0 } else { 0.0 };
            return self.add_element(
                ElementKind::Rule,
                (rule.0 - half, rule.1 - half, rule.2 + half, rule.3 + half),
            );
        }
        if !stroke {
            return Ok(());
        }
        for subpath in &path.subpaths {
            if subpath
                .iter()
                .any(|segment| matches!(segment, Segment::Curve))
            {
                continue;
            }
            for segment in subpath {
                let Segment::Line((x0, y0), (x1, y1)) = *segment else {
                    continue;
                };
                let (dx, dy) = (x1 - x0, y1 - y0);
                let length = (dx * dx + dy * dy).sqrt();
                if length == 0.0 {
                    continue;
                }
                let half = thickness / 2.0;
                if dy.abs() <= UPRIGHT_TOLERANCE * length {
                    let (left, right) = if x0 < x1 { (x0, x1) } else { (x1, x0) };
                    self.add_element(ElementKind::Rule, (left, y0 - half, right, y0 + half))?;
                } else if dx.abs() <= UPRIGHT_TOLERANCE * length {
                    let (top, bottom) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
                    self.add_element(ElementKind::Rule, (x0 - half, top, x0 + half, bottom))?;
                }
            }
        }
        Ok(())
    }

    /// Adds an element with the box `(x0, y0, x1, y1)` in page space.
    fn add_element(
        &mut self,
        kind: ElementKind,
        (x0, y0, x1, y1): (f64, f64, f64, f64),
    ) -> Result<(), Error> {
        if self.counters.elements >= self.file.limits().max_elements {
            return Err(Error::Limit(Limit::Elements));
        }
        self.counters.elements += 1;
        let point = |value: f64| {
            Points::from_points(value)
                .ok_or_else(|| Error::malformed("an image or rule lies out of range"))
        };
        self.drawn.elements.push(Element {
            kind,
            x0: point(x0)?,
            y0: point(y0)?,
            x1: point(x1)?,
            y1: point(y1)?,
            x0_blu: None,
            y0_blu: None,
            x1_blu: None,
            y1_blu: None,
        });
        Ok(())
    }

    /// The box of the unit square under the current transformation matrix: where an image is drawn.
    fn image(&mut self) -> Result<(), Error> {
        let corners = [
            self.to_page(0.0, 0.0),
            self.to_page(1.0, 0.0),
            self.to_page(0.0, 1.0),
            self.to_page(1.0, 1.0),
        ];
        let mut bounds = (corners[0].0, corners[0].1, corners[0].0, corners[0].1);
        for (x, y) in corners {
            if x < bounds.0 {
                bounds.0 = x;
            }
            if y < bounds.1 {
                bounds.1 = y;
            }
            if x > bounds.2 {
                bounds.2 = x;
            }
            if y > bounds.3 {
                bounds.3 = y;
            }
        }
        self.add_element(ElementKind::Image, bounds)
    }

    /// `Td`: a new line, offset from the start of the current one.
    fn move_line(&mut self, x: f64, y: f64) -> Result<(), Error> {
        let (_, line) = self
            .text
            .ok_or_else(|| outside_text("Td, TD, T*, ' or \""))?;
        let line = Matrix::translate(x, y).then(line);
        if !line.is_finite() {
            return Err(Error::malformed("a text position is out of range"));
        }
        self.text = Some((line, line));
        Ok(())
    }

    /// Moves the text matrix by `tx` along the baseline.
    fn advance(&mut self, tx: f64) -> Result<(), Error> {
        let (text, line) = self.text.ok_or_else(|| outside_text("TJ"))?;
        let text = Matrix::translate(tx, 0.0).then(text);
        if !text.is_finite() {
            return Err(Error::malformed("a text position is out of range"));
        }
        self.text = Some((text, line));
        Ok(())
    }

    /// Shows the glyphs of `string` (§9.4.4).
    fn show(&mut self, string: &[u8]) -> Result<(), Error> {
        let font = self
            .state
            .font
            .clone()
            .ok_or_else(|| Error::malformed("text is shown before `Tf` chooses a font"))?;
        if self.text.is_none() {
            return Err(outside_text("Tj, TJ, ' or \""));
        }
        for (code, length) in font.codes(string)? {
            self.counters.glyphs += 1;
            if self.counters.glyphs > self.file.limits().max_glyphs {
                return Err(Error::Limit(Limit::Glyphs));
            }
            let glyph = font.glyph(code, length);
            let (text_matrix, _) = self.text.ok_or_else(|| outside_text("Tj"))?;
            let to_page = text_matrix.then(self.state.ctm).then(self.page);
            let (size, scale) = (self.state.size, self.state.scale);
            // The glyph's axes in page space: its x axis (along the baseline) and its y axis (up), each one em long.
            let (xa, xb) = (size * scale * to_page.a, size * scale * to_page.b);
            let (yc, yd) = (size * to_page.c, size * to_page.d);
            let tx = (glyph.width * size
                + self.state.char_spacing
                + if glyph.space {
                    self.state.word_spacing
                } else {
                    0.0
                })
                * scale;
            let upright = xa > 0.0 && xb.abs() <= UPRIGHT_TOLERANCE * xa && yd < 0.0;
            let visible = !matches!(self.state.render, 3 | 7);
            if upright && visible {
                let (x, y) = to_page.apply(0.0, self.state.rise);
                let height = (xa * yd - xb * yc).abs() / (xa * xa + xb * xb).sqrt();
                let advance = tx * to_page.a;
                if ![x, y, height, advance]
                    .iter()
                    .all(|value| value.is_finite())
                {
                    return Err(Error::malformed("a glyph's position is out of range"));
                }
                self.drawn.glyphs.push(RawGlyph {
                    x,
                    y,
                    advance,
                    size: height,
                    text: glyph.text,
                    font: font.output,
                    gid: glyph.gid,
                    unmapped: glyph.unmapped,
                });
            } else {
                self.drawn.skipped += 1;
            }
            self.advance(tx)?;
        }
        Ok(())
    }

    /// The font that the resources name `key`.
    fn font(&mut self, key: &[u8], resources: Option<&Dict>) -> Result<Rc<Font>, Error> {
        let object = self.resource(resources, b"Font", key, "font")?;
        self.fonts.font(self.file, &object)
    }

    /// An entry of a category of the resources (unresolved, so that fonts are cached by reference).
    fn resource(
        &mut self,
        resources: Option<&Dict>,
        category: &[u8],
        key: &[u8],
        what: &str,
    ) -> Result<Object, Error> {
        let missing = || Error::malformed(format!("a {what} that the resources do not name"));
        let resources = resources.ok_or_else(missing)?;
        let entries = match self.file.lookup(resources, category)? {
            Some(value) => match &*value {
                Object::Dict(entries) => entries.clone(),
                _ => {
                    return Err(Error::malformed(format!(
                        "the resources' {what}s are not a dictionary"
                    )));
                }
            },
            None => return Err(missing()),
        };
        entries.get(key).cloned().ok_or_else(missing)
    }

    /// `gs`: the parts of a graphics state parameter dictionary that place things: the line width and the font.
    fn ext_g_state(&mut self, key: &[u8], resources: Option<&Dict>) -> Result<(), Error> {
        let object = self.resource(resources, b"ExtGState", key, "graphics state")?;
        let parameters = dict_of(self.file, &object, "a graphics state parameter dictionary")?;
        if let Some(width) = parameters.get(b"LW") {
            self.state.line_width = number(self.file, width, "a graphics state's `LW`")?;
        }
        if let Some(font) = parameters.get(b"Font") {
            let font = (*self.file.resolve(font)?).clone();
            let Object::Array(items) = font else {
                return Err(Error::malformed(
                    "a graphics state's `Font` is not an array",
                ));
            };
            let [reference, size] = items.as_slice() else {
                return Err(Error::malformed(
                    "a graphics state's `Font` is not a font and a size",
                ));
            };
            let size = number(self.file, size, "a graphics state's `Font`")?;
            let font = self.fonts.font(self.file, reference)?;
            self.state.font = Some(font);
            self.state.size = size;
        }
        Ok(())
    }

    /// `Do`: an image's box, or a form XObject's content (§8.10).
    fn x_object(&mut self, key: &[u8], resources: Option<&Dict>) -> Result<(), Error> {
        let object = self.resource(resources, b"XObject", key, "XObject")?;
        let value = self.file.resolve(&object)?;
        let stream = value
            .as_stream()
            .ok_or_else(|| Error::malformed("an XObject is not a stream"))?
            .clone();
        drop(value);
        if stream.dict.has_name(b"Subtype", b"Image") {
            return self.image();
        }
        if stream.dict.has_name(b"Subtype", b"PS") {
            return Err(Error::unsupported("PostScript XObjects"));
        }
        if !stream.dict.has_name(b"Subtype", b"Form") {
            return Err(Error::malformed(
                "an XObject is neither an image nor a form",
            ));
        }
        if self.form_depth >= self.file.limits().max_form_depth {
            return Err(Error::Limit(Limit::FormDepth));
        }
        let matrix = match stream.dict.get(b"Matrix") {
            None => Matrix::IDENTITY,
            Some(object) => {
                let items = (*self.file.resolve(object)?).clone();
                let Object::Array(items) = items else {
                    return Err(Error::malformed("a form's `Matrix` is not an array"));
                };
                Matrix::new(numbers::<6>(&items, "a form's `Matrix`")?)
            }
        };
        let form_resources = match stream.dict.get(b"Resources") {
            Some(object) => Some(dict_of(self.file, object, "a form's resources")?),
            None => resources.cloned(),
        };
        let data = self.file.stream_data(&stream)?;
        if self.saved.len() >= self.file.limits().max_graphics_depth {
            return Err(Error::Limit(Limit::GraphicsDepth));
        }
        self.saved.push(self.state.clone());
        let floor = core::mem::replace(&mut self.floor, self.saved.len());
        // A form's content stands on its own (§8.10.1): its path, text objects, marked content and compatibility sections begin and end inside it.
        let path = core::mem::take(&mut self.path);
        let text = self.text.take();
        let marked = core::mem::take(&mut self.marked);
        let compatibility = core::mem::take(&mut self.compatibility);
        self.state.ctm = matrix.then(self.state.ctm);
        if !self.state.ctm.is_finite() {
            return Err(Error::malformed(
                "the transformation matrix is out of range",
            ));
        }
        self.form_depth += 1;
        let result = self.run(&data, form_resources.as_ref());
        self.form_depth -= 1;
        result?;
        // The form's own unbalanced `q`s end with it.
        self.saved.truncate(self.floor);
        self.floor = floor;
        self.state = self
            .saved
            .pop()
            .ok_or_else(|| Error::malformed("a form XObject lost its saved state"))?;
        self.path = path;
        self.text = text;
        self.marked = marked;
        self.compatibility = compatibility;
        Ok(())
    }

    /// `BI` … `ID` … `EI`: an inline image, whose box is placed like an image XObject's (§8.9.7).
    fn inline_image(&mut self, lexer: &mut Lexer<'_>) -> Result<(), Error> {
        let max_nesting = self.file.limits().max_nesting;
        let mut dict = Dict::default();
        loop {
            match lexer.next_token()? {
                Some(Token::Keyword(b"ID")) => break,
                Some(Token::Name(key)) => {
                    let token = lexer.next_token()?.ok_or_else(|| {
                        Error::malformed("an inline image's dictionary does not end")
                    })?;
                    let value = parse_from(token, lexer, false, max_nesting, 0)?;
                    if dict.contains(&key) {
                        return Err(Error::malformed(
                            "an inline image's dictionary has the same key twice",
                        ));
                    }
                    dict.set(key, value);
                }
                _ => {
                    return Err(Error::malformed(
                        "an inline image's dictionary is malformed",
                    ));
                }
            }
        }
        // `ID` is followed by one white-space byte, then the data.
        let start = lexer.position();
        let data = lexer.data();
        if !data.get(start).is_some_and(|&byte| is_whitespace(byte)) {
            return Err(Error::malformed(
                "an inline image's data does not follow `ID` and a white-space byte",
            ));
        }
        let start = start + 1;
        let length = inline_image_length(&dict, data.get(start..).unwrap_or_default())?;
        let end = start + length;
        let mut after = end;
        while data.get(after).is_some_and(|&byte| is_whitespace(byte)) {
            after += 1;
        }
        let ends = data.get(after..after + 2) == Some(b"EI")
            && data
                .get(after + 2)
                .is_none_or(|&byte| !super::lexer::is_regular(byte));
        if !ends {
            return Err(Error::malformed(
                "an inline image's data does not end with `EI`",
            ));
        }
        lexer.seek(after + 2);
        self.image()
    }
}

/// The length in bytes of an inline image's data, from its dictionary: unfiltered data has exactly its size, and ASCIIHexDecode and ASCII85Decode end at their end markers. Other filters give no length, and are refused.
fn inline_image_length(dict: &Dict, data: &[u8]) -> Result<usize, Error> {
    let get = |short: &[u8], long: &[u8]| dict.get(short).or_else(|| dict.get(long));
    if let Some(filter) = get(b"F", b"Filter") {
        let name = match filter {
            Object::Name(name) => name.as_slice(),
            Object::Array(items) if items.len() == 1 => items[0].as_name().unwrap_or_default(),
            _ => return Err(Error::unsupported("an inline image with several filters")),
        };
        let end_marker: &[u8] = match name {
            b"AHx" | b"ASCIIHexDecode" => b">",
            b"A85" | b"ASCII85Decode" => b"~>",
            _ => {
                return Err(Error::unsupported(
                    "an inline image compressed with a filter that does not mark its end",
                ));
            }
        };
        let position = data
            .windows(end_marker.len())
            .position(|window| window == end_marker)
            .ok_or_else(|| Error::malformed("an inline image's data has no end marker"))?;
        return Ok(position + end_marker.len());
    }
    let integer = |short: &[u8], long: &[u8]| -> Result<usize, Error> {
        get(short, long)
            .and_then(Object::as_integer)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| {
                Error::malformed(
                    "an inline image's width, height or bits per component are missing",
                )
            })
    };
    let mask = matches!(get(b"IM", b"ImageMask"), Some(Object::Bool(true)));
    let width = integer(b"W", b"Width")?;
    let height = integer(b"H", b"Height")?;
    let bits = if mask {
        1
    } else {
        integer(b"BPC", b"BitsPerComponent")?
    };
    let components = if mask {
        1
    } else {
        match get(b"CS", b"ColorSpace") {
            Some(Object::Name(name)) => match name.as_slice() {
                b"G" | b"DeviceGray" | b"I" | b"Indexed" => 1,
                b"RGB" | b"DeviceRGB" => 3,
                b"CMYK" | b"DeviceCMYK" => 4,
                _ => {
                    return Err(Error::unsupported(
                        "an inline image with a named colour space",
                    ));
                }
            },
            Some(Object::Array(items))
                if items
                    .first()
                    .and_then(Object::as_name)
                    .is_some_and(|name| name == b"I" || name == b"Indexed") =>
            {
                1
            }
            _ => {
                return Err(Error::unsupported(
                    "an inline image whose colour space the extractor cannot size",
                ));
            }
        }
    };
    if !matches!(bits, 1 | 2 | 4 | 8 | 16) {
        return Err(Error::malformed(
            "an inline image has an invalid number of bits per component",
        ));
    }
    let row_bits = width
        .checked_mul(components)
        .and_then(|value| value.checked_mul(bits))
        .ok_or_else(|| Error::malformed("an inline image is too large"))?;
    let length = row_bits
        .div_ceil(8)
        .checked_mul(height)
        .filter(|&length| length <= data.len())
        .ok_or_else(|| Error::malformed("an inline image's data is shorter than its size"))?;
    Ok(length)
}

/// `count` numbers, the operands of `operator`.
fn numbers<const COUNT: usize>(operands: &[Object], operator: &str) -> Result<[f64; COUNT], Error> {
    if operands.len() != COUNT {
        return Err(operands_error(operator));
    }
    let mut values = [0.0; COUNT];
    for (slot, operand) in values.iter_mut().zip(operands) {
        *slot = operand
            .as_number()
            .filter(|value| value.is_finite())
            .ok_or_else(|| operands_error(operator))?;
    }
    Ok(values)
}

fn integer(operands: &[Object], operator: &str) -> Result<i64, Error> {
    match operands {
        [Object::Integer(value)] => Ok(*value),
        _ => Err(operands_error(operator)),
    }
}

fn name(operands: &[Object], operator: &str) -> Result<Vec<u8>, Error> {
    match operands {
        [Object::Name(name)] => Ok(name.clone()),
        _ => Err(operands_error(operator)),
    }
}

fn none(operands: &[Object], operator: &str) -> Result<(), Error> {
    if operands.is_empty() {
        Ok(())
    } else {
        Err(operands_error(operator))
    }
}

fn operands_error(operator: &str) -> Error {
    Error::malformed(format!("`{operator}` has the wrong operands"))
}

fn outside_text(operators: &str) -> Error {
    Error::malformed(format!("`{operators}` outside a text object"))
}

/// The name of a known operator, for messages (every caller passes one of PDF's own operators).
fn operator_name(operator: &[u8]) -> &str {
    core::str::from_utf8(operator).unwrap_or("?")
}

/// If the path is one filled rectangle, horizontal and vertical, thin enough to be a rule: its box.
fn thin_rectangle(path: &Path) -> Option<(f64, f64, f64, f64)> {
    let [subpath] = path.subpaths.as_slice() else {
        return None;
    };
    let mut points = Vec::with_capacity(4);
    for segment in subpath {
        let Segment::Line(from, to) = *segment else {
            return None;
        };
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        if dx != 0.0 && dy != 0.0 {
            return None;
        }
        points.push(from);
    }
    if points.len() != 4 {
        return None;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (points[0].0, points[0].1, points[0].0, points[0].1);
    for &(x, y) in &points {
        if x < x0 {
            x0 = x;
        }
        if y < y0 {
            y0 = y;
        }
        if x > x1 {
            x1 = x;
        }
        if y > y1 {
            y1 = y;
        }
    }
    let (width, height) = (x1 - x0, y1 - y0);
    let (thin, long) = if width < height {
        (width, height)
    } else {
        (height, width)
    };
    (thin <= MAX_RULE_THICKNESS && long >= 2.0 * thin && long > 0.0).then_some((x0, y0, x1, y1))
}
