//! The mock document: a fixed three-page document that `doc.open` opens whatever bytes it is given, until the engine can open real documents (CORE-007).
//!
//! It exists so the shells can prove scrolling, tiles, input and accessibility end to end (DESK-002, WEB-002). Its pages hold a heading, paragraphs, a bulleted list, a small table and, on the first page, an **editable line** that shows the text the shell sends: committed text as dark boxes, one per character, and input method composition as blue boxes with an underline (thicker under the composition's selected part), so the input method path is visible. Text is drawn as grey bars ("greeked") until the text pipeline exists (CORE-003); screen readers get the real words through the accessibility tree.
//!
//! The editable line understands `input.text`, `input.composition`, the keys `Backspace`, `Delete`, `ArrowLeft`, `ArrowRight`, `Home` and `End`, and a primary-button press inside it, which moves the caret. It holds at most 1,024 UTF-16 code units of text and 256 of composition.

mod content;
mod editable;
mod geometry;
mod layout;
mod text;

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::protocol::{
    A11yCaret, A11yNode, A11yRole, Caret, CharBox, PageInfo, PageRect, Rect, TextRange,
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct MockDocument {
    /// The shell's view, once it sent `view.set`.
    pub(crate) view: Option<View>,
    /// The editable line.
    line: EditableLine,
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
            children: Some(children),
        }
    }

    /// The subtree rooted at node `id`, if there is such a node.
    pub(crate) fn a11y_subtree(&self, id: u64) -> Option<A11yNode> {
        fn find(node: A11yNode, id: u64) -> Option<A11yNode> {
            if node.id == id {
                return Some(node);
            }
            node.children?.into_iter().find_map(|child| find(child, id))
        }
        find(self.a11y_tree(), id)
    }

    /// Inserts committed text (`input.text`).
    pub(crate) fn insert_text(&mut self, text: &str) -> Result<bool, TooLong> {
        self.line.insert(text)
    }

    /// Applies an editing key (`input.key`).
    pub(crate) fn key(&mut self, key: &str) -> bool {
        self.line.key(key)
    }

    /// Starts a composition (`input.composition` `start`).
    pub(crate) fn start_composition(&mut self) -> bool {
        self.line.start_composition()
    }

    /// Updates the composition (`input.composition` `update`).
    pub(crate) fn update_composition(
        &mut self,
        text: &str,
        selection: Option<TextRange>,
    ) -> Result<bool, TooLong> {
        self.line.update_composition(text, selection)
    }

    /// Ends the composition (`input.composition` `end`).
    pub(crate) fn end_composition(&mut self) -> bool {
        self.line.end_composition()
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
        view_ok && self.line.is_valid()
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
        let frame = MockDocument::editable_region().rect;
        let area = text_area(frame);
        // Click just right of the second character's middle: the caret lands after it.
        let blu_x = area.left + 2 * BODY.advance - BODY.advance / 3;
        let x = f64::from(i32::try_from(blu_x / BLU_PER_DIP).unwrap());
        let y = f64::from(i32::try_from((frame.y + frame.height / 2) / BLU_PER_DIP).unwrap());
        assert!(document.press(x, y));
        assert_eq!(document.a11y_caret().offset, 2);
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
    fn finds_subtrees_by_identifier() {
        let document = MockDocument::default();
        let tree = document.a11y_tree();
        assert_eq!(tree.role, A11yRole::Document);
        let editable = document.a11y_subtree(fixed().editable_id).unwrap();
        assert!(editable.editable);
        assert!(document.a11y_subtree(u64::MAX).is_none());
    }
}
