//! The fixed part of the mock document laid out once: the shapes of every page, the accessibility nodes, the lines of their text for hit-testing, and where the editable line sits.

use crate::protocol::{A11yNode, A11yRole, PageRect, Rect, TextDirection, TextRun};
use crate::raster::Shape;

use super::content::{BLOCKS, Block, Foreign};
use super::geometry::{
    BULLET_INDENT, BULLET_SIZE, CELL_PADDING, CONTENT_WIDTH, EDITABLE_HEIGHT, HEADER_FILL,
    HEADING_SPACING_AFTER, HEADING_SPACING_BEFORE, LIST_ITEM_SPACING, LIST_TEXT_INDENT, MARGIN,
    MUTED, PAGE_HEIGHT, PAGE_WIDTH, PAPER, PARAGRAPH_SPACING, TABLE_ROW_HEIGHT, TABLE_RULE,
};
use super::text::{BODY, Bidi, HEADING_1, HEADING_2, HitLine, LTR, TextBlock, TextStyle};

/// One top-level child of the document node, in reading order.
#[derive(Debug, Clone)]
pub(crate) enum Slot {
    /// A fixed node, with its children, kept apart because a node is large and the editable line's slot holds nothing.
    Fixed(Box<A11yNode>),
    /// The editable line, whose node changes with its text.
    Editable,
}

/// The text of one node, in reading order, as hit-testing sees it (spec §14.4).
#[derive(Debug, Clone)]
pub(crate) enum Lines {
    /// The lines of a fixed node, on its page.
    Fixed {
        /// The node.
        node: u64,
        /// The page.
        page: u32,
        /// Its lines.
        lines: Vec<HitLine>,
    },
    /// The editable line, whose characters change.
    Editable,
}

/// The fixed part of the mock document.
#[derive(Debug, Clone)]
pub(crate) struct Layout {
    /// The fixed shapes of each page, paper first.
    pub(crate) pages: Vec<Vec<Shape>>,
    /// The document node's children.
    pub(crate) slots: Vec<Slot>,
    /// The text of every node that holds text, in reading order.
    pub(crate) lines: Vec<Lines>,
    /// The editable line's page and frame.
    pub(crate) editable: PageRect,
    /// The editable line's accessibility node identifier.
    pub(crate) editable_id: u64,
    /// The largest accessibility node identifier: the nodes are numbered from [`DOCUMENT_ID`] to it without gaps.
    pub(crate) last_id: u64,
}

/// The accessibility node identifier of the document.
pub(crate) const DOCUMENT_ID: u64 = 1;

/// The length of a string in UTF-16 code units.
fn utf16_len(text: &str) -> u32 {
    u32::try_from(text.encode_utf16().count()).unwrap_or(u32::MAX)
}

/// Lays the blocks out from the top of the first page, moving a block to the next page when it does not fit.
struct Builder {
    pages: Vec<Vec<Shape>>,
    slots: Vec<Slot>,
    lines: Vec<Lines>,
    editable: Option<(PageRect, u64)>,
    next_id: u64,
    y: i64,
}

impl Builder {
    fn page(&self) -> u32 {
        u32::try_from(self.pages.len().saturating_sub(1)).unwrap_or(u32::MAX)
    }

    fn new_page(&mut self) {
        self.pages.push(vec![Shape {
            rect: Rect {
                x: 0,
                y: 0,
                width: PAGE_WIDTH,
                height: PAGE_HEIGHT,
            },
            color: PAPER,
        }]);
        self.y = MARGIN;
    }

    /// Starts a new page unless `height` fits below the current position.
    fn reserve(&mut self, height: i64) {
        if self.y > MARGIN && self.y + height > PAGE_HEIGHT - MARGIN {
            self.new_page();
        }
    }

    fn id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn shapes(&mut self, shapes: impl IntoIterator<Item = Shape>) {
        if let Some(page) = self.pages.last_mut() {
            page.extend(shapes);
        }
    }

    /// Lays out left-to-right text, moving it to the next page if it does not fit, and returns its node.
    fn text(
        &mut self,
        text: &str,
        style: TextStyle,
        left: i64,
        width: i64,
        role: A11yRole,
    ) -> (A11yNode, i64) {
        self.directed(text, style, left, width, role, LTR)
    }

    /// Lays out text in the direction of `bidi`, as [`TextBlock::directed`] does, and returns its node, which inherits its language and direction; records its lines for hit-testing.
    fn directed(
        &mut self,
        text: &str,
        style: TextStyle,
        left: i64,
        width: i64,
        role: A11yRole,
        bidi: Bidi<'_>,
    ) -> (A11yNode, i64) {
        let mut block = TextBlock::directed(text, style, left, self.y, width, bidi);
        if self.y > MARGIN && block.bottom() > PAGE_HEIGHT - MARGIN {
            self.new_page();
            block = TextBlock::directed(text, style, left, self.y, width, bidi);
        }
        self.shapes(block.shapes());
        let id = self.id();
        self.lines.push(Lines::Fixed {
            node: id,
            page: self.page(),
            lines: block.hit_lines(),
        });
        let node = A11yNode {
            id,
            role,
            level: None,
            name: None,
            text: Some(text.to_owned()),
            editable: false,
            bounds: Some(PageRect {
                page: self.page(),
                rect: block.bounds(),
            }),
            chars: Some(block.char_boxes()),
            lang: None,
            dir: None,
            runs: None,
            line_starts: Some(block.line_starts()),
            child_count: None,
            children_start: None,
            children: None,
        };
        (node, block.bottom())
    }

    /// A paragraph whose language and direction differ from the document's, with its parts in other languages as runs (spec §14.7).
    fn foreign(&mut self, foreign: &Foreign) {
        let mut text = String::new();
        let mut runs = Vec::new();
        let mut ltr = Vec::new();
        for part in foreign.parts {
            let start = utf16_len(&text);
            text.push_str(part.text);
            let end = utf16_len(&text);
            if let Some((lang, dir)) = part.run {
                runs.push(TextRun {
                    start,
                    end,
                    lang: (lang != foreign.lang).then(|| lang.to_owned()),
                    dir: (dir != foreign.dir).then_some(dir),
                });
                if dir == TextDirection::Ltr {
                    ltr.push((start, end));
                }
            }
        }
        let bidi = Bidi {
            dir: foreign.dir,
            ltr: &ltr,
        };
        let (mut node, bottom) = self.directed(
            &text,
            BODY,
            MARGIN,
            CONTENT_WIDTH,
            A11yRole::Paragraph,
            bidi,
        );
        node.lang = Some(foreign.lang.to_owned());
        node.dir = Some(foreign.dir);
        node.runs = (!runs.is_empty()).then_some(runs);
        self.slots.push(Slot::Fixed(Box::new(node)));
        self.y = bottom + PARAGRAPH_SPACING;
    }

    fn heading(&mut self, level: u8, text: &str) {
        if self.y > MARGIN {
            self.y += HEADING_SPACING_BEFORE;
        }
        let style = if level == 1 { HEADING_1 } else { HEADING_2 };
        let (mut node, bottom) = self.text(text, style, MARGIN, CONTENT_WIDTH, A11yRole::Heading);
        node.level = Some(level);
        self.slots.push(Slot::Fixed(Box::new(node)));
        self.y = bottom + HEADING_SPACING_AFTER;
    }

    fn paragraph(&mut self, text: &str) {
        let (node, bottom) = self.text(text, BODY, MARGIN, CONTENT_WIDTH, A11yRole::Paragraph);
        self.slots.push(Slot::Fixed(Box::new(node)));
        self.y = bottom + PARAGRAPH_SPACING;
    }

    fn editable(&mut self) {
        self.reserve(EDITABLE_HEIGHT);
        let frame = PageRect {
            page: self.page(),
            rect: Rect {
                x: MARGIN,
                y: self.y,
                width: CONTENT_WIDTH,
                height: EDITABLE_HEIGHT,
            },
        };
        let id = self.id();
        self.editable = Some((frame, id));
        self.slots.push(Slot::Editable);
        self.lines.push(Lines::Editable);
        self.y += EDITABLE_HEIGHT + PARAGRAPH_SPACING;
    }

    fn list(&mut self, items: &[&str]) {
        let list_id = self.id();
        let top = self.y;
        let first_page = self.page();
        let mut children = Vec::new();
        for item in items {
            let left = MARGIN + LIST_TEXT_INDENT;
            let (node, bottom) = self.text(
                item,
                BODY,
                left,
                CONTENT_WIDTH - LIST_TEXT_INDENT,
                A11yRole::ListItem,
            );
            let line_top = node.bounds.map_or(self.y, |bounds| bounds.rect.y);
            self.shapes([Shape {
                rect: Rect {
                    x: MARGIN + BULLET_INDENT,
                    y: line_top + (BODY.line_pitch - BULLET_SIZE) / 2,
                    width: BULLET_SIZE,
                    height: BULLET_SIZE,
                },
                color: MUTED,
            }]);
            children.push(node);
            self.y = bottom + LIST_ITEM_SPACING;
        }
        let bounds = (self.page() == first_page).then_some(PageRect {
            page: first_page,
            rect: Rect {
                x: MARGIN,
                y: top,
                width: CONTENT_WIDTH,
                height: self.y - LIST_ITEM_SPACING - top,
            },
        });
        self.slots.push(Slot::Fixed(Box::new(A11yNode {
            id: list_id,
            role: A11yRole::List,
            level: None,
            name: None,
            text: None,
            editable: false,
            bounds,
            chars: None,
            lang: None,
            dir: None,
            runs: None,
            line_starts: None,
            child_count: None,
            children_start: None,
            children: Some(children),
        })));
        self.y += PARAGRAPH_SPACING - LIST_ITEM_SPACING;
    }

    fn table(&mut self, rows: &[&[&str]]) {
        let row_count = i64::try_from(rows.len()).unwrap_or(i64::MAX);
        let column_count = rows.first().map_or(0, |row| row.len());
        let columns = i64::try_from(column_count).unwrap_or(i64::MAX).max(1);
        let height = row_count * TABLE_ROW_HEIGHT;
        self.reserve(height);
        let top = self.y;
        let column_width = CONTENT_WIDTH / columns;
        let table_id = self.id();
        let mut row_nodes = Vec::new();
        for (row_index, cells) in rows.iter().enumerate() {
            let row_top = top + i64::try_from(row_index).unwrap_or(i64::MAX) * TABLE_ROW_HEIGHT;
            let header = row_index == 0;
            if header {
                self.shapes([Shape {
                    rect: Rect {
                        x: MARGIN,
                        y: row_top,
                        width: column_width * columns,
                        height: TABLE_ROW_HEIGHT,
                    },
                    color: HEADER_FILL,
                }]);
            }
            let row_id = self.id();
            let mut cell_nodes = Vec::new();
            for (column_index, cell) in cells.iter().enumerate() {
                let cell_left =
                    MARGIN + i64::try_from(column_index).unwrap_or(i64::MAX) * column_width;
                self.y = row_top + (TABLE_ROW_HEIGHT - BODY.line_pitch) / 2;
                let role = if header {
                    A11yRole::ColumnHeader
                } else {
                    A11yRole::Cell
                };
                let (node, _) = self.text(
                    cell,
                    BODY,
                    cell_left + CELL_PADDING,
                    column_width - 2 * CELL_PADDING,
                    role,
                );
                cell_nodes.push(node);
            }
            row_nodes.push(A11yNode {
                id: row_id,
                role: A11yRole::Row,
                level: None,
                name: None,
                text: None,
                editable: false,
                bounds: Some(PageRect {
                    page: self.page(),
                    rect: Rect {
                        x: MARGIN,
                        y: row_top,
                        width: column_width * columns,
                        height: TABLE_ROW_HEIGHT,
                    },
                }),
                chars: None,
                lang: None,
                dir: None,
                runs: None,
                line_starts: None,
                child_count: None,
                children_start: None,
                children: Some(cell_nodes),
            });
        }
        // The rules: one above every row and below the last, and one left of every column and right of the last.
        let width = column_width * columns;
        let mut rules = Vec::new();
        for row in 0..=row_count {
            rules.push(Rect {
                x: MARGIN,
                y: top + row * TABLE_ROW_HEIGHT - TABLE_RULE / 2,
                width,
                height: TABLE_RULE,
            });
        }
        for column in 0..=columns {
            rules.push(Rect {
                x: MARGIN + column * column_width - TABLE_RULE / 2,
                y: top,
                width: TABLE_RULE,
                height,
            });
        }
        self.shapes(rules.into_iter().map(|rect| Shape { rect, color: MUTED }));
        self.slots.push(Slot::Fixed(Box::new(A11yNode {
            id: table_id,
            role: A11yRole::Table,
            level: None,
            name: None,
            text: None,
            editable: false,
            bounds: Some(PageRect {
                page: self.page(),
                rect: Rect {
                    x: MARGIN,
                    y: top,
                    width,
                    height,
                },
            }),
            chars: None,
            lang: None,
            dir: None,
            runs: None,
            line_starts: None,
            child_count: None,
            children_start: None,
            children: Some(row_nodes),
        })));
        self.y = top + height + PARAGRAPH_SPACING;
    }

    /// A short bar centred in the bottom margin of every page, standing for its page number.
    fn page_numbers(&mut self) {
        let width = 6 * BODY.advance;
        for page in &mut self.pages {
            page.push(Shape {
                rect: Rect {
                    x: (PAGE_WIDTH - width) / 2,
                    y: PAGE_HEIGHT - MARGIN / 2 - BODY.bar_height / 2,
                    width,
                    height: BODY.bar_height,
                },
                color: MUTED,
            });
        }
    }
}

/// Lays out the fixed part of the mock document.
pub(crate) fn lay_out() -> Layout {
    let mut builder = Builder {
        pages: Vec::new(),
        slots: Vec::new(),
        lines: Vec::new(),
        editable: None,
        next_id: DOCUMENT_ID + 1,
        y: MARGIN,
    };
    builder.new_page();
    for block in BLOCKS {
        match block {
            Block::Heading(level, text) => builder.heading(*level, text),
            Block::Paragraph(text) => builder.paragraph(text),
            Block::Foreign(foreign) => builder.foreign(foreign),
            Block::Editable => builder.editable(),
            Block::List(items) => builder.list(items),
            Block::Table(rows) => builder.table(rows),
            Block::PageBreak => builder.new_page(),
        }
    }
    builder.page_numbers();
    let (editable, editable_id) = builder.editable.unwrap_or((
        PageRect {
            page: 0,
            rect: Rect {
                x: MARGIN,
                y: MARGIN,
                width: CONTENT_WIDTH,
                height: EDITABLE_HEIGHT,
            },
        },
        0,
    ));
    Layout {
        pages: builder.pages,
        slots: builder.slots,
        lines: builder.lines,
        editable,
        editable_id,
        last_id: builder.next_id - 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_three_pages_and_the_editable_line_on_the_first() {
        let layout = lay_out();
        assert_eq!(layout.pages.len(), 3);
        assert_eq!(layout.editable.page, 0);
        assert!(layout.editable_id > DOCUMENT_ID);
        // Every shape stays on its page.
        for page in &layout.pages {
            for shape in page {
                assert!(shape.rect.x >= 0 && shape.rect.x + shape.rect.width <= PAGE_WIDTH);
                assert!(shape.rect.y >= 0 && shape.rect.y + shape.rect.height <= PAGE_HEIGHT);
            }
        }
    }

    #[test]
    fn contains_a_heading_paragraphs_a_list_and_a_table() {
        let layout = lay_out();
        let roles: Vec<A11yRole> = layout
            .slots
            .iter()
            .filter_map(|slot| match slot {
                Slot::Fixed(node) => Some(node.role),
                Slot::Editable => None,
            })
            .collect();
        for role in [
            A11yRole::Heading,
            A11yRole::Paragraph,
            A11yRole::List,
            A11yRole::Table,
        ] {
            assert!(roles.contains(&role), "{role:?}");
        }
    }

    /// The fixed node with the identifier `id`, among the layout's top-level nodes.
    fn fixed_node(layout: &Layout, id: u64) -> &A11yNode {
        layout
            .slots
            .iter()
            .find_map(|slot| match slot {
                Slot::Fixed(node) if node.id == id => Some(node),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn the_arabic_paragraph_runs_right_to_left_on_the_last_page_with_an_english_run() {
        let layout = lay_out();
        // It comes before the last paragraph, which keeps the last place; the nodes before it keep their identifiers.
        let arabic = fixed_node(&layout, 35);
        assert_eq!(arabic.lang.as_deref(), Some("ar"));
        assert_eq!(arabic.dir, Some(TextDirection::Rtl));
        let text = arabic.text.as_deref().unwrap();
        let run = &arabic.runs.as_ref().unwrap()[0];
        assert_eq!(
            (run.start, run.end, run.lang.as_deref(), run.dir),
            (52, 64, Some("en"), Some(TextDirection::Ltr))
        );
        let english: String = text
            .encode_utf16()
            .skip(52)
            .take(12)
            .map(|unit| char::from_u32(u32::from(unit)).unwrap())
            .collect();
        assert_eq!(english, "Hello, world");
        // Two lines; the second starts with the word that did not fit.
        assert_eq!(arabic.line_starts.as_deref(), Some(&[0, 84][..]));
        assert_eq!(arabic.bounds.unwrap().page, 2);
        let last = fixed_node(&layout, 36);
        assert_eq!(
            last.text.as_deref(),
            Some("This is the end of the mock document.")
        );
        assert_eq!((last.lang.as_ref(), last.dir), (None, None));
        // The English run reads left to right: its first character is left of its last.
        let boxes = arabic.chars.as_ref().unwrap();
        assert!(boxes[52].rect.x < boxes[63].rect.x);
        // The Arabic reads right to left: its first character is right of its second.
        assert!(boxes[0].rect.x > boxes[1].rect.x);
        assert_eq!(
            boxes[0].rect.x + boxes[0].rect.width,
            MARGIN + CONTENT_WIDTH
        );
    }

    #[test]
    fn the_content_holds_no_bidirectional_formatting_characters() {
        // Right-to-left text in a source file is shown reordered by editors; formatting characters could make code or text read differently from what it is ("Trojan Source"). The content uses none: only letters, digits, spaces and punctuation, whose order the Unicode Bidirectional Algorithm alone decides.
        let formatting = |character: char| {
            matches!(
                character,
                '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
            )
        };
        for part in super::super::content::ARABIC.parts {
            assert!(!part.text.chars().any(formatting), "{:?}", part.text);
        }
        let layout = lay_out();
        for slot in &layout.slots {
            if let Slot::Fixed(node) = slot {
                assert!(
                    !node
                        .text
                        .iter()
                        .flat_map(|text| text.chars())
                        .any(formatting)
                );
            }
        }
    }

    #[test]
    fn every_text_node_has_line_starts_and_hit_lines() {
        let layout = lay_out();
        let mut with_text = 0;
        let mut check = |node: &A11yNode| {
            if let Some(text) = &node.text {
                with_text += 1;
                let starts = node.line_starts.as_deref().unwrap();
                assert_eq!(starts.first(), Some(&0));
                assert!(starts.windows(2).all(|pair| pair[0] < pair[1]));
                assert!(starts.iter().all(|&start| start <= utf16_len(text)));
            } else {
                assert_eq!(node.line_starts, None);
            }
        };
        fn walk(node: &A11yNode, check: &mut dyn FnMut(&A11yNode)) {
            check(node);
            for child in node.children.iter().flatten() {
                walk(child, check);
            }
        }
        for slot in &layout.slots {
            if let Slot::Fixed(node) = slot {
                walk(node, &mut check);
            }
        }
        // Every node with text has its lines for hit-testing, and the editable line its place among them.
        let fixed = layout
            .lines
            .iter()
            .filter(|lines| matches!(lines, Lines::Fixed { .. }))
            .count();
        assert_eq!(fixed, with_text);
        assert_eq!(
            layout
                .lines
                .iter()
                .filter(|lines| matches!(lines, Lines::Editable))
                .count(),
            1
        );
    }

    #[test]
    fn gives_every_node_its_own_identifier() {
        fn collect(node: &A11yNode, ids: &mut Vec<u64>) {
            ids.push(node.id);
            for child in node.children.iter().flatten() {
                collect(child, ids);
            }
        }
        let layout = lay_out();
        let mut ids = vec![DOCUMENT_ID, layout.editable_id];
        for slot in &layout.slots {
            if let Slot::Fixed(node) = slot {
                collect(node, &mut ids);
            }
        }
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count);
    }
}
