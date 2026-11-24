//! The mock document: a fixed three-page document that `doc.open` opens whatever bytes it is given, until the engine can open real documents (CORE-007).
//!
//! It exists so the shells can prove scrolling, tiles, input and accessibility end to end (DESK-002, WEB-002). Its pages hold a heading, paragraphs, a bulleted list, a small table and, on the first page, an **editable line** that shows the text the shell sends: committed text as dark boxes, one per character, and input method composition as blue boxes with an underline (thicker under the composition's selected part), so the input method path is visible. Text is drawn as grey bars ("greeked") until the text pipeline exists (CORE-003); screen readers get the real words through the accessibility tree.
//!
//! The editable line understands `input.text`, `input.composition`, the document keys `Backspace`, `Delete`, `ArrowLeft`, `ArrowRight`, `Home` and `End` (with any modifiers, as spec §14.3 matches document keys), and a primary-button press inside it, which moves the caret. It holds at most 1,024 UTF-16 code units of text and 256 of composition. Its render revision grows with every change to what its tiles show: the editable line's text and composition, but not the caret, which the shell draws over the tiles, and the view's mode and formatting marks; it lays every mode out like `print` (spec §14.6).

mod content;
mod editable;
mod geometry;
mod layout;
mod text;

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::limits::MAX_ID;
use crate::protocol::{
    A11yCaret, A11yNode, A11yRole, Caret, CharBox, PageInfo, PageRect, Rect, TextRange, ViewMode,
};
use crate::raster::{Shape, TileGeometry, rasterize};

pub(crate) use editable::TooLong;
use editable::{EditableLine, Shown};
use geometry::{
    ACCENT, CARET_WIDTH, CHAR_GAP, CHAR_HEIGHT, EDITABLE_BORDER, EDITABLE_FILL, EDITABLE_PADDING,
    INK, PAGE_GAP, PAGE_HEIGHT, PAGE_WIDTH, UNDERLINE,
};
use layout::{DOCUMENT_ID, Layout, Slot};
use text::{BODY, advance};

/// The fixed part of the mock document, laid out the first time it is needed.
fn fixed() -> &'static Layout {
    static LAYOUT: OnceLock<Layout> = OnceLock::new();
    LAYOUT.get_or_init(layout::lay_out)
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
            visible: true,
        }
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
            child_count: None,
            children_start: None,
            children: None,
        }
    }

    /// The accessibility caret: in the editable line, in UTF-16 code units.
    pub(crate) fn a11y_caret(&self) -> A11yCaret {
        let index = usize::try_from(self.line.shown_caret()).unwrap_or(usize::MAX);
        let offset: usize = self
            .line
            .shown()
            .iter()
            .take(index)
            .map(|shown| shown.character.len_utf16())
            .sum();
        A11yCaret {
            node: fixed().editable_id,
            offset: u32::try_from(offset).unwrap_or(u32::MAX),
        }
    }

    /// The whole accessibility tree.
    pub(crate) fn a11y_tree(&self) -> A11yNode {
        let children = fixed()
            .slots
            .iter()
            .map(|slot| match slot {
                Slot::Fixed(node) => node.clone(),
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
            child_count: None,
            children_start: None,
            children: Some(children),
        }
    }

    /// The accessibility tree and the accessibility caret within the depth limit of spec §14.7, flattened below level 64; the mock document's tree is three levels deep, so nothing is.
    pub(crate) fn a11y_tree_and_caret(&self) -> (A11yNode, Option<A11yCaret>) {
        let mut tree = self.a11y_tree();
        let mut caret = Some(self.a11y_caret());
        crate::a11y::flatten(&mut tree, &mut caret);
        (tree, caret)
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

    /// Applies an editing key (`input.key`).
    pub(crate) fn key(&mut self, key: &str) -> bool {
        self.edit(|line| line.key(key))
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
        assert!(document.key("ArrowLeft"));
        assert!(!document.key("Escape"));
        assert_eq!(document.revision(), 2);
        assert!(document.key("Backspace"));
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

    #[test]
    fn finds_subtrees_by_identifier() {
        let document = MockDocument::default();
        let (tree, caret) = document.a11y_tree_and_caret();
        assert_eq!(tree.role, A11yRole::Document);
        assert_eq!(tree, document.a11y_tree());
        let editable = crate::a11y::find(&tree, document.editable_id()).unwrap();
        assert!(editable.editable);
        assert_eq!(caret.unwrap().node, editable.id);
        assert!(crate::a11y::find(&tree, u64::MAX).is_none());
    }
}
