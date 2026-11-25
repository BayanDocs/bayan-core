//! The mock document: a fixed three-page document that `doc.open` opens whatever bytes it is given, until the engine can open real documents (CORE-007).
//!
//! It exists so the shells can prove scrolling, tiles, input and accessibility end to end (DESK-002, WEB-002). Its pages hold a heading, paragraphs, a bulleted list, a small table and, on the first page, an **editable line** that shows the text the shell sends: committed text as dark boxes, one per character, and input method composition as blue boxes with an underline (thicker under the composition's selected part), so the input method path is visible. Text is drawn as grey bars ("greeked") until the text pipeline exists (CORE-003); screen readers get the real words through the accessibility tree.
//!
//! The editable line understands `input.text` (also with `replace`, a range of its text), `input.select`, `input.paste` and `clipboard.copy` (spec §14.4, §14.5), `input.composition`, the document keys `Backspace`, `Delete`, `ArrowLeft`, `ArrowRight`, `Home` and `End` (with any modifiers, as spec §14.3 matches document keys; with Shift, the arrows, Home and End extend the selection), and a primary-button press inside it, which moves the caret. It holds at most 1,024 UTF-16 code units of text and 256 of composition, also when text is pasted. It is the only text that holds a caret or a selection, which the shell draws from `overlay.update`. Its render revision grows with every change to what its tiles show: the editable line's text and composition, but not the caret or the selection, which the shell draws over the tiles, and the view's mode and formatting marks; it lays every mode out like `print` (spec §14.6).
//!
//! The document is in English, left to right, which every node inherits, except for one paragraph on the last page in Arabic, right to left, with an English phrase in it, so that the shells can test language, direction and runs (spec §14.7); every node with text reports where its lines start. `query.hit` finds the text position at a point from the same lines and character boxes that the accessibility tree reports (spec §14.4).

mod content;
mod editable;
mod geometry;
mod hit;
mod layout;
mod text;

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::limits::MAX_ID;
use crate::protocol::{
    A11yCaret, A11yNode, A11yRole, A11ySelection, Caret, CharBox, Hit, PageInfo, PageRect, Rect,
    SelectionRects, TextPosition, TextRange, ViewMode,
};
use crate::raster::{Shape, TileGeometry, rasterize};

use editable::{EditError, EditableLine, Shown};
pub(crate) use editable::{Selected, TooLong, clipboard_text};
use geometry::{
    ACCENT, CARET_WIDTH, CHAR_GAP, CHAR_HEIGHT, EDITABLE_BORDER, EDITABLE_FILL, EDITABLE_PADDING,
    INK, PAGE_GAP, PAGE_HEIGHT, PAGE_WIDTH, UNDERLINE,
};
use layout::{DOCUMENT_ID, Layout, Lines, Slot};
use text::{BODY, HitItem, HitLine, advance};

/// The fixed part of the mock document, laid out the first time it is needed.
fn fixed() -> &'static Layout {
    static LAYOUT: OnceLock<Layout> = OnceLock::new();
    LAYOUT.get_or_init(layout::lay_out)
}

/// Why the mock document refuses a text position or range that the shell sent (spec §14.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextError {
    /// The node does not exist (`not_found`).
    NotFound,
    /// The node is not editable, or the position or range lies beyond its text, backwards or inside a surrogate pair (`invalid_request`).
    Invalid,
    /// The text would be longer than its limit (`limit_exceeded`).
    TooLong,
}

impl From<EditError> for TextError {
    fn from(error: EditError) -> Self {
        match error {
            EditError::Invalid => Self::Invalid,
            EditError::TooLong => Self::TooLong,
        }
    }
}

/// Checks that the nodes `ids` exist and are editable: in the mock document, only the editable line is, until editing arrives (Phase 2). A node that does not exist counts before one that is not editable.
fn editable_nodes(ids: &[u64]) -> Result<(), TextError> {
    let layout = fixed();
    if ids
        .iter()
        .any(|id| !(DOCUMENT_ID..=layout.last_id).contains(id))
    {
        return Err(TextError::NotFound);
    }
    if ids.iter().any(|&id| id != layout.editable_id) {
        return Err(TextError::Invalid);
    }
    Ok(())
}

/// What the shell last told the engine about its view of a document (`view.set`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct View {
    /// The zoom factor, which converts pointer positions to BLU.
    pub(crate) zoom: f64,
}

/// One open copy of the mock document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct MockDocument {
    /// The shell's view, once it sent `view.set`.
    pub(crate) view: Option<View>,
    /// The view mode that the tiles show (spec §14.6): `print` until a `view.set` chooses another.
    mode: ViewMode,
    /// Whether the tiles show formatting marks: not until a `view.set` asks for them.
    show_formatting_marks: bool,
    /// The render revision (spec §14.6): 1 when the document opens, one more with every change to what any of its tiles shows.
    revision: u64,
    /// The editable line.
    line: EditableLine,
}

impl Default for MockDocument {
    fn default() -> Self {
        Self {
            view: None,
            mode: ViewMode::Print,
            show_formatting_marks: false,
            revision: 1,
            line: EditableLine::default(),
        }
    }
}

/// Where the editable line's text sits.
struct TextArea {
    left: i64,
    top: i64,
    right: i64,
}

fn text_area(frame: Rect) -> TextArea {
    TextArea {
        left: frame.x + EDITABLE_PADDING,
        top: frame.y + (frame.height - BODY.line_pitch) / 2,
        right: frame.x + frame.width - EDITABLE_PADDING,
    }
}

/// The left edge of every shown character, relative to the text's left edge, and the position after the last one.
fn positions(shown: &[Shown]) -> Vec<i64> {
    let mut x = 0;
    let mut edges = Vec::with_capacity(shown.len() + 1);
    for character in shown {
        edges.push(x);
        x += advance(&BODY, character.character);
    }
    edges.push(x);
    edges
}

/// The view-space BLU coordinate of a coordinate in device-independent pixels at a zoom factor (spec §7), or `None` if it is not finite or out of range. Uses only exact IEEE-754 operations and rounding (ADR-0005 §4).
#[expect(
    clippy::cast_possible_truncation,
    reason = "the value is finite, rounded to an integer and checked to lie far inside the range of i64 before the conversion"
)]
fn dip_to_blu(dip: f64, zoom: f64) -> Option<i64> {
    const BLU_PER_DIP_F64: f64 = 19_050.0;
    if !dip.is_finite() || dip.abs() > crate::limits::MAX_VIEW_COORDINATE {
        return None;
    }
    let blu = (dip * BLU_PER_DIP_F64 / zoom).round();
    if !blu.is_finite() || blu.abs() > 4.0e18 {
        return None;
    }
    Some(blu as i64)
}

impl MockDocument {
    /// The number of pages.
    pub(crate) fn page_count() -> u32 {
        u32::try_from(fixed().pages.len()).unwrap_or(u32::MAX)
    }

    /// The pages and their positions in view space: stacked from the view origin downwards, with a gap between them (spec §7).
    pub(crate) fn pages() -> Vec<PageInfo> {
        (0..Self::page_count())
            .map(|index| PageInfo {
                index,
                width: PAGE_WIDTH,
                height: PAGE_HEIGHT,
                section: 0,
                x: 0,
                y: i64::from(index) * (PAGE_HEIGHT + PAGE_GAP),
            })
            .collect()
    }

    /// Every page, whole, in page coordinates.
    pub(crate) fn page_regions() -> Vec<PageRect> {
        Self::pages()
            .iter()
            .map(|page| PageRect {
                page: page.index,
                rect: Rect {
                    x: 0,
                    y: 0,
                    width: page.width,
                    height: page.height,
                },
            })
            .collect()
    }

    /// The editable line's frame.
    pub(crate) fn editable_region() -> PageRect {
        fixed().editable
    }

    /// The shapes of the editable line in its current state.
    fn editable_shapes(&self) -> Vec<Shape> {
        let frame = fixed().editable.rect;
        let area = text_area(frame);
        let mut shapes = vec![Shape {
            rect: frame,
            color: EDITABLE_FILL,
        }];
        let border = [
            Rect {
                height: EDITABLE_BORDER,
                ..frame
            },
            Rect {
                y: frame.y + frame.height - EDITABLE_BORDER,
                height: EDITABLE_BORDER,
                ..frame
            },
            Rect {
                width: EDITABLE_BORDER,
                ..frame
            },
            Rect {
                x: frame.x + frame.width - EDITABLE_BORDER,
                width: EDITABLE_BORDER,
                ..frame
            },
        ];
        shapes.extend(border.into_iter().map(|rect| Shape {
            rect,
            color: ACCENT,
        }));
        let shown = self.line.shown();
        let edges = positions(&shown);
        let box_top = area.top + (BODY.line_pitch - CHAR_HEIGHT) / 2;
        let underline_top = box_top + CHAR_HEIGHT + UNDERLINE;
        for (index, character) in shown.iter().enumerate() {
            let (left, right) = (area.left + edges[index], area.left + edges[index + 1]);
            // Characters beyond the frame are not drawn (the line does not scroll).
            if right > area.right {
                break;
            }
            let color = if character.composed { ACCENT } else { INK };
            if !character.character.is_whitespace() {
                shapes.push(Shape {
                    rect: Rect {
                        x: left + CHAR_GAP,
                        y: box_top,
                        width: right - left - 2 * CHAR_GAP,
                        height: CHAR_HEIGHT,
                    },
                    color,
                });
            }
            if character.composed {
                let thickness = if character.selected {
                    2 * UNDERLINE
                } else {
                    UNDERLINE
                };
                shapes.push(Shape {
                    rect: Rect {
                        x: left,
                        y: underline_top,
                        width: right - left,
                        height: thickness,
                    },
                    color: ACCENT,
                });
            }
        }
        shapes
    }

    /// Renders a tile of a page, or returns `None` if the page does not exist or the tile does not fit in memory.
    pub(crate) fn render(&self, page: u32, geometry: &TileGeometry) -> Option<Vec<u8>> {
        let layout = fixed();
        let fixed_shapes = layout.pages.get(usize::try_from(page).ok()?)?;
        let editable = if page == layout.editable.page {
            self.editable_shapes()
        } else {
            Vec::new()
        };
        rasterize(fixed_shapes.iter().chain(&editable), geometry)
    }

    /// The caret, in the editable line.
    pub(crate) fn caret(&self) -> Caret {
        let region = fixed().editable;
        let area = text_area(region.rect);
        let shown = self.line.shown();
        let edges = positions(&shown);
        let index = usize::try_from(self.line.shown_caret()).unwrap_or(usize::MAX);
        let offset = edges.get(index).copied().unwrap_or(0);
        let x = (area.left + offset).min(area.right);
        Caret {
            page: region.page,
            rect: Rect {
                x: x - CARET_WIDTH / 2,
                y: area.top,
                width: CARET_WIDTH,
                height: BODY.line_pitch,
            },
            // While text is selected the shell shows the selection, not the caret, as Word does; the caret's place still serves input methods and magnifiers.
            visible: !self.has_selection(),
        }
    }

    /// Whether text is selected.
    fn has_selection(&self) -> bool {
        self.line.shown_anchor() != self.line.shown_caret()
    }

    /// The selection's rectangles (spec §6.3): over the selected characters of the editable line, which neither wraps nor scrolls, clipped to its frame as the caret is; none when nothing is selected or nothing selected lies inside the frame.
    pub(crate) fn selection_rects(&self) -> Vec<SelectionRects> {
        if !self.has_selection() {
            return Vec::new();
        }
        let region = fixed().editable;
        let area = text_area(region.rect);
        let edges = positions(&self.line.shown());
        let edge = |index: u32| {
            let offset = edges
                .get(usize::try_from(index).unwrap_or(usize::MAX))
                .copied()
                .unwrap_or(0);
            (area.left + offset).min(area.right)
        };
        let (anchor, focus) = (self.line.shown_anchor(), self.line.shown_caret());
        let (left, right) = (edge(anchor.min(focus)), edge(anchor.max(focus)));
        if left >= right {
            return Vec::new();
        }
        vec![SelectionRects {
            page: region.page,
            rects: vec![Rect {
                x: left,
                y: area.top,
                width: right - left,
                height: BODY.line_pitch,
            }],
        }]
    }

    /// The editable line's accessibility node. The line neither wraps nor scrolls, so characters past its frame (after about the 99th) are not drawn; their boxes continue beyond the frame's right edge, possibly beyond the page (spec §6.6).
    pub(crate) fn editable_node(&self) -> A11yNode {
        let layout = fixed();
        let area = text_area(layout.editable.rect);
        let shown = self.line.shown();
        let edges = positions(&shown);
        let mut chars = Vec::with_capacity(shown.len());
        let mut offset = 0_u32;
        for (index, character) in shown.iter().enumerate() {
            let units = u32::try_from(character.character.len_utf16()).unwrap_or(1);
            chars.push(CharBox {
                start: offset,
                end: offset + units,
                rect: Rect {
                    x: area.left + edges[index],
                    y: area.top,
                    width: edges[index + 1] - edges[index],
                    height: BODY.line_pitch,
                },
            });
            offset += units;
        }
        A11yNode {
            id: layout.editable_id,
            role: A11yRole::Paragraph,
            level: None,
            name: Some(content::EDITABLE_NAME.to_owned()),
            text: Some(self.line.shown_text()),
            editable: true,
            bounds: Some(layout.editable),
            chars: Some(chars),
            lang: None,
            dir: None,
            runs: None,
            // The line neither wraps nor scrolls: one line.
            line_starts: Some(vec![0]),
            child_count: None,
            children_start: None,
            children: None,
        }
    }

    /// The UTF-16 offset of the editable line's shown character `index`.
    fn units(&self, index: u32) -> u32 {
        let offset: usize = self
            .line
            .shown()
            .iter()
            .take(usize::try_from(index).unwrap_or(usize::MAX))
            .map(|shown| shown.character.len_utf16())
            .sum();
        u32::try_from(offset).unwrap_or(u32::MAX)
    }

    /// The accessibility caret: in the editable line, in UTF-16 code units.
    pub(crate) fn a11y_caret(&self) -> A11yCaret {
        A11yCaret {
            node: fixed().editable_id,
            offset: self.units(self.line.shown_caret()),
        }
    }

    /// The accessibility tree's selection (spec §14.7): the editable line's, in UTF-16 code units, or none when nothing is selected.
    pub(crate) fn a11y_selection(&self) -> Vec<A11ySelection> {
        if !self.has_selection() {
            return Vec::new();
        }
        let node = fixed().editable_id;
        vec![A11ySelection {
            anchor: TextPosition {
                node,
                offset: self.units(self.line.shown_anchor()),
            },
            focus: TextPosition {
                node,
                offset: self.units(self.line.shown_caret()),
            },
        }]
    }

    /// The whole accessibility tree.
    pub(crate) fn a11y_tree(&self) -> A11yNode {
        let children = fixed()
            .slots
            .iter()
            .map(|slot| match slot {
                Slot::Fixed(node) => node.as_ref().clone(),
                Slot::Editable => self.editable_node(),
            })
            .collect();
        A11yNode {
            id: DOCUMENT_ID,
            role: A11yRole::Document,
            level: None,
            name: Some(content::DOCUMENT_NAME.to_owned()),
            text: None,
            editable: false,
            bounds: None,
            chars: None,
            lang: Some(content::DOCUMENT_LANG.to_owned()),
            dir: Some(content::DOCUMENT_DIR),
            runs: None,
            line_starts: None,
            child_count: None,
            children_start: None,
            children: Some(children),
        }
    }

    /// The accessibility tree, its caret and its selection within the depth limit of spec §14.7, flattened below level 64; the mock document's tree is three levels deep, so nothing is.
    pub(crate) fn a11y_state(&self) -> (A11yNode, Option<A11yCaret>, Vec<A11ySelection>) {
        let mut tree = self.a11y_tree();
        let mut caret = Some(self.a11y_caret());
        let mut selection = self.a11y_selection();
        crate::a11y::flatten_with_selection(&mut tree, &mut caret, &mut selection);
        (tree, caret, selection)
    }

    /// The identifier of the editable line's accessibility node.
    pub(crate) fn editable_id(&self) -> u64 {
        fixed().editable_id
    }

    /// The render revision: what the document's tiles show now (spec §14.6).
    pub(crate) const fn revision(&self) -> u64 {
        self.revision
    }

    /// Raises the render revision after a change to what a tile shows. It stays below 2⁵³ (spec §14.6): reaching that would take more changes than a session can make, and the revision then stays at its largest value.
    fn raise_revision(&mut self) {
        self.revision = self.revision.saturating_add(1).min(MAX_ID);
    }

    /// Applies an edit to the editable line, and raises the render revision if the edit changed what the tiles show: the text and the composition, but not the caret, which the shell draws over the tiles (spec §6.3).
    fn edit<R>(&mut self, edit: impl FnOnce(&mut EditableLine) -> R) -> R {
        let before = self.line.shown();
        let result = edit(&mut self.line);
        if self.line.shown() != before {
            self.raise_revision();
        }
        result
    }

    /// Takes a `view.set` (spec §14.6): the zoom, which converts pointer positions, the mode and whether formatting marks show. Returns whether every page must be invalidated: after a new mode or a change of `show_formatting_marks`, which raise the render revision. A new viewport, zoom or device scale changes no tile: the mock document lays every mode out like `print`, so even in `web`, whose layout follows the window, the new layout changes nothing.
    pub(crate) fn set_view(
        &mut self,
        zoom: f64,
        mode: ViewMode,
        show_formatting_marks: bool,
    ) -> bool {
        self.view = Some(View { zoom });
        let changed = mode != self.mode || show_formatting_marks != self.show_formatting_marks;
        self.mode = mode;
        self.show_formatting_marks = show_formatting_marks;
        if changed {
            self.raise_revision();
        }
        changed
    }

    /// Inserts committed text (`input.text`).
    pub(crate) fn insert_text(&mut self, text: &str) -> Result<bool, TooLong> {
        self.edit(|line| line.insert(text))
    }

    /// Applies an editing key (`input.key`); `shift` extends the selection with the arrows, Home and End.
    pub(crate) fn key(&mut self, key: &str, shift: bool) -> bool {
        self.edit(|line| line.key(key, shift))
    }

    /// Replaces the UTF-16 range `start` to `end` of node `node`'s text (`input.text` with `replace`, spec §14.4). Returns whether anything changed.
    pub(crate) fn replace_text(
        &mut self,
        node: u64,
        start: u32,
        end: u32,
        text: &str,
    ) -> Result<bool, TextError> {
        editable_nodes(&[node])?;
        Ok(self.edit(|line| line.replace(start, end, text))?)
    }

    /// Sets the selection (`input.select`, spec §14.4), unless a composition keeps it.
    pub(crate) fn select(
        &mut self,
        anchor: TextPosition,
        focus: TextPosition,
    ) -> Result<Selected, TextError> {
        editable_nodes(&[anchor.node, focus.node])?;
        Ok(self.line.select(anchor.offset, focus.offset)?)
    }

    /// The selected text, for `clipboard.copy` (spec §14.5); empty when nothing is selected.
    pub(crate) fn selected_text(&self) -> String {
        self.line.selected_text()
    }

    /// Deletes the selected text, for `clipboard.copy` with `cut`. Returns whether anything changed.
    pub(crate) fn cut(&mut self) -> bool {
        self.edit(EditableLine::cut)
    }

    /// The text position at a point in device-independent pixels from the view origin (`query.hit`, spec §14.4): `None` when the point is on no page, or before a `view.set` has given the zoom.
    pub(crate) fn hit(&self, x: f64, y: f64) -> Option<Hit> {
        let view = self.view?;
        let (x, y) = (dip_to_blu(x, view.zoom)?, dip_to_blu(y, view.zoom)?);
        let pitch = PAGE_HEIGHT + PAGE_GAP;
        if !(0..PAGE_WIDTH).contains(&x) || y < 0 {
            return None;
        }
        let (page, page_y) = (y / pitch, y % pitch);
        if page_y >= PAGE_HEIGHT || page >= i64::from(Self::page_count()) {
            return None;
        }
        let page = u32::try_from(page).ok()?;
        let layout = fixed();
        let editable = self.editable_line();
        let lines = layout
            .lines
            .iter()
            .flat_map(|lines| -> Vec<(u64, &HitLine)> {
                match lines {
                    Lines::Fixed {
                        node,
                        page: on,
                        lines,
                    } if *on == page => lines.iter().map(|line| (*node, line)).collect(),
                    Lines::Editable if layout.editable.page == page => {
                        vec![(layout.editable_id, &editable)]
                    }
                    Lines::Fixed { .. } | Lines::Editable => Vec::new(),
                }
            });
        hit::hit(lines, x, page_y)
    }

    /// The editable line as hit-testing sees it: one line of its shown characters, left to right, with the boxes that its accessibility node reports.
    fn editable_line(&self) -> HitLine {
        let area = text_area(fixed().editable.rect);
        let shown = self.line.shown();
        let edges = positions(&shown);
        let mut offset = 0_u32;
        let items = shown
            .iter()
            .enumerate()
            .map(|(index, character)| {
                let units = u32::try_from(character.character.len_utf16()).unwrap_or(1);
                let item = HitItem {
                    left: area.left + edges[index],
                    right: area.left + edges[index + 1],
                    start: offset,
                    end: offset + units,
                    rtl: false,
                };
                offset += units;
                item
            })
            .collect();
        HitLine {
            top: area.top,
            bottom: area.top + BODY.line_pitch,
            start: 0,
            empty_x: area.left,
            items,
        }
    }

    /// Starts a composition (`input.composition` `start`).
    pub(crate) fn start_composition(&mut self) -> bool {
        self.edit(EditableLine::start_composition)
    }

    /// Updates the composition (`input.composition` `update`).
    pub(crate) fn update_composition(
        &mut self,
        text: &str,
        selection: Option<TextRange>,
    ) -> Result<bool, TooLong> {
        self.edit(|line| line.update_composition(text, selection))
    }

    /// Ends the composition (`input.composition` `end`).
    pub(crate) fn end_composition(&mut self) -> bool {
        self.edit(EditableLine::end_composition)
    }

    /// A primary-button press at a position in device-independent pixels from the view origin: inside the editable line, it moves the caret to the nearest character boundary. Returns whether the caret moved.
    pub(crate) fn press(&mut self, x: f64, y: f64) -> bool {
        let Some(view) = self.view else {
            return false;
        };
        let (Some(view_x), Some(view_y)) = (dip_to_blu(x, view.zoom), dip_to_blu(y, view.zoom))
        else {
            return false;
        };
        let region = fixed().editable;
        let page_top = i64::from(region.page) * (PAGE_HEIGHT + PAGE_GAP);
        let (page_x, page_y) = (view_x, view_y - page_top);
        let frame = region.rect;
        let inside = page_x >= frame.x
            && page_x < frame.x + frame.width
            && page_y >= frame.y
            && page_y < frame.y + frame.height;
        if !inside || self.line.composing() {
            return false;
        }
        let area = text_area(frame);
        let edges = positions(&self.line.shown());
        let target = page_x - area.left;
        let nearest = edges
            .iter()
            .enumerate()
            .min_by_key(|&(_, &edge)| (edge - target).abs())
            .map_or(0, |(index, _)| index);
        self.line
            .set_caret(u32::try_from(nearest).unwrap_or(u32::MAX))
    }

    /// Whether a state read from a recording is consistent (spec §10: recordings are untrusted).
    pub(crate) fn is_valid(&self) -> bool {
        let view_ok = self.view.is_none_or(|view| {
            view.zoom.is_finite()
                && view.zoom >= crate::limits::MIN_ZOOM
                && view.zoom <= crate::limits::MAX_ZOOM
        });
        view_ok && (1..=MAX_ID).contains(&self.revision) && self.line.is_valid()
    }
}

#[cfg(test)]
mod tests {
    use super::geometry::BLU_PER_DIP;
    use super::*;
    use crate::protocol::TextDirection;

    #[test]
    fn stacks_three_pages_with_gaps() {
        let pages = MockDocument::pages();
        assert_eq!(pages.len(), 3);
        assert_eq!(pages[1].y, PAGE_HEIGHT + PAGE_GAP);
        assert_eq!(MockDocument::page_count(), 3);
    }

    #[test]
    fn converts_pointer_positions_exactly() {
        assert_eq!(dip_to_blu(1.0, 1.0), Some(BLU_PER_DIP));
        assert_eq!(dip_to_blu(1.0, 2.0), Some(BLU_PER_DIP / 2));
        assert_eq!(dip_to_blu(-3.0, 1.0), Some(-3 * BLU_PER_DIP));
        assert_eq!(dip_to_blu(f64::NAN, 1.0), None);
        assert_eq!(dip_to_blu(1.0e300, 1.0), None);
    }

    #[test]
    fn a_click_in_the_editable_line_moves_the_caret() {
        let mut document = MockDocument::default();
        document.insert_text("abcdef").unwrap();
        // Without a view, the engine cannot tell where a click lands.
        assert!(!document.press(0.0, 0.0));
        document.view = Some(View { zoom: 1.0 });
        let revision = document.revision();
        let frame = MockDocument::editable_region().rect;
        let area = text_area(frame);
        // Click just right of the second character's middle: the caret lands after it.
        let blu_x = area.left + 2 * BODY.advance - BODY.advance / 3;
        let x = f64::from(i32::try_from(blu_x / BLU_PER_DIP).unwrap());
        let y = f64::from(i32::try_from((frame.y + frame.height / 2) / BLU_PER_DIP).unwrap());
        assert!(document.press(x, y));
        assert_eq!(document.a11y_caret().offset, 2);
        // The caret is drawn over the tiles, so moving it changes no tile.
        assert_eq!(document.revision(), revision);
        // A click far outside does nothing.
        assert!(!document.press(1.0, 1.0));
    }

    #[test]
    fn the_editable_node_reports_text_boxes_and_caret_in_utf16() {
        let mut document = MockDocument::default();
        document.insert_text("a😀").unwrap();
        let node = document.editable_node();
        assert_eq!(node.text.as_deref(), Some("a😀"));
        let chars = node.chars.unwrap();
        assert_eq!((chars[1].start, chars[1].end), (1, 3));
        assert_eq!(document.a11y_caret().offset, 3);
        assert!(node.editable);
    }

    #[test]
    fn typing_changes_only_the_editable_lines_page() {
        let geometry = |page_rect: PageRect| TileGeometry {
            rect: page_rect.rect,
            width: 102,
            height: 132,
        };
        let pages = MockDocument::page_regions();
        let blank = MockDocument::default();
        let mut typed = MockDocument::default();
        typed.insert_text("Hello").unwrap();
        let first = blank.render(0, &geometry(pages[0])).unwrap();
        assert_ne!(first, typed.render(0, &geometry(pages[0])).unwrap());
        assert_eq!(
            blank.render(1, &geometry(pages[1])).unwrap(),
            typed.render(1, &geometry(pages[1])).unwrap()
        );
        assert!(blank.render(3, &geometry(pages[0])).is_none());
    }

    #[test]
    fn the_revision_grows_with_what_the_tiles_show() {
        let mut document = MockDocument::default();
        assert_eq!(document.revision(), 1);
        document.insert_text("ab").unwrap();
        assert_eq!(document.revision(), 2);
        // Moving the caret, or a key that changes nothing, keeps the revision.
        assert!(document.key("ArrowLeft", false));
        assert!(!document.key("Escape", false));
        assert_eq!(document.revision(), 2);
        assert!(document.key("Backspace", false));
        assert_eq!(document.revision(), 3);
        // A composition shows in the tiles.
        assert!(document.start_composition());
        assert!(document.update_composition("x", None).unwrap());
        assert_eq!(document.revision(), 4);
        // The view: only a new mode or formatting marks change what the tiles show.
        assert!(!document.set_view(2.0, ViewMode::Print, false));
        assert!(!document.set_view(0.5, ViewMode::Print, false));
        assert_eq!(document.revision(), 4);
        assert!(document.set_view(0.5, ViewMode::Web, false));
        assert!(!document.set_view(1.5, ViewMode::Web, false));
        assert!(document.set_view(1.5, ViewMode::Web, true));
        assert_eq!(document.revision(), 6);
        document.revision = MAX_ID;
        document.raise_revision();
        assert_eq!(document.revision(), MAX_ID);
    }

    /// The mock document, viewed at zoom 1.
    fn viewed() -> MockDocument {
        MockDocument {
            view: Some(View { zoom: 1.0 }),
            ..MockDocument::default()
        }
    }

    /// The point at page coordinates `x`, `y` of page `page`, in device-independent pixels from the view origin at zoom 1.
    fn point(page: u32, x: i64, y: i64) -> (f64, f64) {
        let dip = |blu: i64| f64::from(i32::try_from(blu).unwrap()) / 19_050.0;
        (dip(x), dip(i64::from(page) * (PAGE_HEIGHT + PAGE_GAP) + y))
    }

    /// The caret and the character at a point of a page.
    fn hit_at(
        document: &MockDocument,
        page: u32,
        x: i64,
        y: i64,
    ) -> Option<(u64, u32, Option<u32>)> {
        let (x, y) = point(page, x, y);
        document
            .hit(x, y)
            .map(|hit| (hit.node, hit.caret, hit.character))
    }

    /// A node of the accessibility tree, its page and the box of its character `index`.
    fn character(document: &MockDocument, node: u64, index: usize) -> (u32, Rect) {
        let tree = document.a11y_tree();
        let node = crate::a11y::find(&tree, node).unwrap();
        (
            node.bounds.unwrap().page,
            node.chars.as_ref().unwrap()[index].rect,
        )
    }

    #[test]
    fn hits_both_halves_of_a_character_in_left_to_right_text() {
        let mut document = MockDocument::default();
        // Before a view.set the engine cannot place a point.
        assert_eq!(document.hit(100.0, 100.0), None);
        document.view = Some(View { zoom: 1.0 });
        // The second character of the first heading (node 2).
        let (page, rect) = character(&document, 2, 1);
        let middle = rect.y + rect.height / 2;
        assert_eq!(
            hit_at(&document, page, rect.x, middle),
            Some((2, 1, Some(1)))
        );
        assert_eq!(
            hit_at(&document, page, rect.x + rect.width / 2 - 1, middle),
            Some((2, 1, Some(1)))
        );
        // The exact middle counts as the right half.
        assert_eq!(
            hit_at(&document, page, rect.x + rect.width / 2, middle),
            Some((2, 2, Some(1)))
        );
        assert_eq!(
            hit_at(&document, page, rect.x + rect.width - 1, middle),
            Some((2, 2, Some(1)))
        );
        // In the left margin beside the heading: the caret before its first character, and no character.
        assert_eq!(hit_at(&document, page, 0, middle), Some((2, 0, None)));
    }

    #[test]
    fn hits_both_halves_of_a_character_in_right_to_left_text() {
        let document = viewed();
        // The Arabic paragraph (node 35): its first character is the rightmost of its first line; its right half gives the offset before it.
        let (page, first) = character(&document, 35, 0);
        let middle = first.y + first.height / 2;
        assert_eq!(
            hit_at(&document, page, first.x + first.width - 1, middle),
            Some((35, 0, Some(0)))
        );
        assert_eq!(
            hit_at(&document, page, first.x, middle),
            Some((35, 1, Some(0)))
        );
        // Right of the line, in the margin: the caret before the first character.
        assert_eq!(
            hit_at(&document, page, PAGE_WIDTH - 1, middle),
            Some((35, 0, None))
        );
        // "H" (offset 52) of the English run, which runs left to right inside it.
        let (_, h) = character(&document, 35, 52);
        assert_eq!(
            hit_at(&document, page, h.x, middle),
            Some((35, 52, Some(52)))
        );
        assert_eq!(
            hit_at(&document, page, h.x + h.width - 1, middle),
            Some((35, 53, Some(52)))
        );
        // The second line starts at offset 84 at the right edge.
        let (_, second) = character(&document, 35, 84);
        assert_eq!(second.y, first.y + first.height);
        assert_eq!(
            hit_at(&document, page, second.x + second.width - 1, second.y),
            Some((35, 84, Some(84)))
        );
    }

    #[test]
    fn between_lines_a_point_gets_the_nearer_line_and_between_pages_none() {
        let document = viewed();
        // Below the first heading, in the space before the first paragraph (node 3), nearer the heading.
        let (page, heading) = character(&document, 2, 1);
        let (_, paragraph) = character(&document, 3, 1);
        let below = heading.y + heading.height;
        assert!(below < paragraph.y);
        assert_eq!(
            hit_at(&document, page, heading.x, below),
            Some((2, 1, None))
        );
        assert_eq!(
            hit_at(&document, page, paragraph.x, paragraph.y - 1),
            Some((3, 1, None))
        );
        // In the gap between the first two pages, and outside the pages: nothing.
        assert_eq!(hit_at(&document, 0, 1000, PAGE_HEIGHT), None);
        assert_eq!(hit_at(&document, 0, 1000, PAGE_HEIGHT + PAGE_GAP - 1), None);
        assert_eq!(hit_at(&document, 0, -1, 1000), None);
        assert_eq!(hit_at(&document, 0, PAGE_WIDTH, 1000), None);
        assert_eq!(hit_at(&document, 0, 1000, -1), None);
        assert_eq!(hit_at(&document, 3, 1000, 1000), None);
        // The top of the next page is a page again.
        assert!(hit_at(&document, 1, 1000, 0).is_some());
    }

    #[test]
    fn hits_the_editable_line_empty_and_with_text() {
        let mut document = viewed();
        let frame = MockDocument::editable_region();
        let area = text_area(frame.rect);
        let middle = area.top + BODY.line_pitch / 2;
        let editable = document.editable_id();
        assert_eq!(
            hit_at(&document, frame.page, area.left + 1000, middle),
            Some((editable, 0, None))
        );
        document.insert_text("a😀").unwrap();
        // The emoji is wide (two advances) and two UTF-16 code units: its right half gives offset 3.
        let right = area.left + BODY.advance + 2 * BODY.advance - 1;
        assert_eq!(
            hit_at(&document, frame.page, right, middle),
            Some((editable, 3, Some(1)))
        );
        // The character boxes of the accessibility tree say the same.
        let (_, rect) = character(&document, editable, 1);
        assert!((rect.x..rect.x + rect.width).contains(&right));
    }

    #[test]
    fn every_node_has_a_language_and_a_direction_and_its_lines() {
        fn walk(
            node: &A11yNode,
            lang: Option<&str>,
            dir: Option<TextDirection>,
            count: &mut usize,
        ) {
            let lang = node.lang.as_deref().or(lang);
            let dir = node.dir.or(dir);
            assert!(lang.is_some() && dir.is_some(), "node {}", node.id);
            *count += 1;
            if let (Some(chars), Some(starts)) = (&node.chars, &node.line_starts) {
                // A line starts wherever a character's box is lower than the one before it.
                let mut expected = vec![0];
                for pair in chars.windows(2) {
                    if pair[1].rect.y > pair[0].rect.y {
                        expected.push(pair[1].start);
                    }
                }
                assert_eq!(starts, &expected, "node {}", node.id);
            }
            for child in node.children.iter().flatten() {
                walk(child, lang, dir, count);
            }
        }
        let document = MockDocument::default();
        let (tree, _, _) = document.a11y_state();
        assert_eq!(
            (tree.lang.as_deref(), tree.dir),
            (Some("en"), Some(TextDirection::Ltr))
        );
        let mut count = 0;
        walk(&tree, None, None, &mut count);
        assert_eq!(u64::try_from(count).unwrap(), fixed().last_id);
    }

    #[test]
    fn the_selection_shows_in_the_overlay_and_the_accessibility_tree() {
        let mut document = MockDocument::default();
        document.insert_text("hello").unwrap();
        assert!(document.selection_rects().is_empty());
        assert!(document.caret().visible);
        let editable = document.editable_id();
        let position = |offset| TextPosition {
            node: editable,
            offset,
        };
        assert_eq!(
            document.select(position(4), position(1)),
            Ok(Selected::Changed)
        );
        let area = text_area(MockDocument::editable_region().rect);
        let rects = document.selection_rects();
        assert_eq!(
            rects,
            [SelectionRects {
                page: 0,
                rects: vec![Rect {
                    x: area.left + BODY.advance,
                    y: area.top,
                    width: 3 * BODY.advance,
                    height: BODY.line_pitch,
                }]
            }]
        );
        // The caret stays at the focus but is hidden while text is selected.
        assert!(!document.caret().visible);
        assert_eq!(document.a11y_caret().offset, 1);
        assert_eq!(
            document.a11y_selection(),
            [A11ySelection {
                anchor: position(4),
                focus: position(1)
            }]
        );
        // Selecting changes no tile.
        assert_eq!(document.revision(), 2);
        // Positions in other nodes are refused.
        assert_eq!(
            document.select(position(0), TextPosition { node: 2, offset: 0 }),
            Err(TextError::Invalid)
        );
        assert_eq!(
            document.select(
                TextPosition {
                    node: 999,
                    offset: 0
                },
                position(0)
            ),
            Err(TextError::NotFound)
        );
        assert_eq!(
            document.replace_text(0, 0, 0, "x"),
            Err(TextError::NotFound)
        );
        assert_eq!(document.selected_text(), "ell");
        assert!(document.cut());
        assert_eq!(document.revision(), 3);
        assert!(document.a11y_selection().is_empty());
    }

    #[test]
    fn finds_subtrees_by_identifier() {
        let document = MockDocument::default();
        let (tree, caret, selection) = document.a11y_state();
        assert!(selection.is_empty());
        assert_eq!(tree.role, A11yRole::Document);
        assert_eq!(tree, document.a11y_tree());
        let editable = crate::a11y::find(&tree, document.editable_id()).unwrap();
        assert!(editable.editable);
        assert_eq!(caret.unwrap().node, editable.id);
        assert!(crate::a11y::find(&tree, u64::MAX).is_none());
    }
}
