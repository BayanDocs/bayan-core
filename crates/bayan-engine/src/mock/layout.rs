//! The fixed part of the mock document laid out once: the shapes of every page, the accessibility nodes, and where the editable line sits.

use crate::protocol::{A11yNode, A11yRole, PageRect, Rect};
use crate::raster::Shape;

use super::content::{BLOCKS, Block};
use super::geometry::{
    BULLET_INDENT, BULLET_SIZE, CELL_PADDING, CONTENT_WIDTH, EDITABLE_HEIGHT, HEADER_FILL,
    HEADING_SPACING_AFTER, HEADING_SPACING_BEFORE, LIST_ITEM_SPACING, LIST_TEXT_INDENT, MARGIN,
    MUTED, PAGE_HEIGHT, PAGE_WIDTH, PAPER, PARAGRAPH_SPACING, TABLE_ROW_HEIGHT, TABLE_RULE,
};
use super::text::{BODY, HEADING_1, HEADING_2, TextBlock, TextStyle};

/// One top-level child of the document node, in reading order.
#[derive(Debug, Clone)]
pub(crate) enum Slot {
    /// A fixed node, with its children.
    Fixed(A11yNode),
    /// The editable line, whose node changes with its text.
    Editable,
}

/// The fixed part of the mock document.
#[derive(Debug, Clone)]
pub(crate) struct Layout {
    /// The fixed shapes of each page, paper first.
    pub(crate) pages: Vec<Vec<Shape>>,
    /// The document node's children.
    pub(crate) slots: Vec<Slot>,
    /// The editable line's page and frame.
    pub(crate) editable: PageRect,
    /// The editable line's accessibility node identifier.
    pub(crate) editable_id: u64,
}

/// The accessibility node identifier of the document.
pub(crate) const DOCUMENT_ID: u64 = 1;

/// Lays the blocks out from the top of the first page, moving a block to the next page when it does not fit.
struct Builder {
    pages: Vec<Vec<Shape>>,
    slots: Vec<Slot>,
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

    /// Lays out text, moving it to the next page if it does not fit, and returns its node.
    fn text(
        &mut self,
        text: &str,
        style: TextStyle,
        left: i64,
        width: i64,
        role: A11yRole,
    ) -> (A11yNode, i64) {
        let mut block = TextBlock::new(text, style, left, self.y, width);
        if self.y > MARGIN && block.bottom() > PAGE_HEIGHT - MARGIN {
            self.new_page();
            block = TextBlock::new(text, style, left, self.y, width);
        }
        self.shapes(block.shapes());
        let node = A11yNode {
            id: self.id(),
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
            children: None,
        };
        (node, block.bottom())
    }

    fn heading(&mut self, level: u8, text: &str) {
        if self.y > MARGIN {
            self.y += HEADING_SPACING_BEFORE;
        }
        let style = if level == 1 { HEADING_1 } else { HEADING_2 };
        let (mut node, bottom) = self.text(text, style, MARGIN, CONTENT_WIDTH, A11yRole::Heading);
        node.level = Some(level);
        self.slots.push(Slot::Fixed(node));
        self.y = bottom + HEADING_SPACING_AFTER;
    }

    fn paragraph(&mut self, text: &str) {
        let (node, bottom) = self.text(text, BODY, MARGIN, CONTENT_WIDTH, A11yRole::Paragraph);
        self.slots.push(Slot::Fixed(node));
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
        self.slots.push(Slot::Fixed(A11yNode {
            id: list_id,
            role: A11yRole::List,
            level: None,
            name: None,
            text: None,
            editable: false,
            bounds,
            chars: None,
            children: Some(children),
        }));
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
        self.slots.push(Slot::Fixed(A11yNode {
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
            children: Some(row_nodes),
        }));
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
        editable: None,
        next_id: DOCUMENT_ID + 1,
        y: MARGIN,
    };
    builder.new_page();
    for block in BLOCKS {
        match block {
            Block::Heading(level, text) => builder.heading(*level, text),
            Block::Paragraph(text) => builder.paragraph(text),
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
        editable,
        editable_id,
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
