//! Accessibility trees within the limits ([engine protocol specification][spec] §14.7 item 1).
//!
//! The engine keeps every accessibility tree it sends within two limits of its own and within the message limit (spec §13), without losing text:
//!
//! - **Depth:** a tree is at most [`MAX_DEPTH`] (64) levels deep, the document node being level 1. [`flatten`] gives a node at level 64 no children: the text of the nodes below it is appended to its own text in reading order, each starting on a new line, with their character boxes, and an accessibility caret inside them moves along.
//! - **Size:** one reply to `query.a11y`, or one change of an `a11y.update`, holds at most [`MAX_NODES`] (200,000) nodes, and every message at most 16 MiB. Where a subtree does not fit, [`fill`] gives some nodes only a part of their children: always one unbroken run, with `child_count` and `children_start` on the node, filled breadth-first in reading order, and always with the path from the root to the caret, whose nodes hold the part of their children that starts with the child on the path. A shell loads the rest with `query.a11y { node, children: { start, count } }`, whose answer [`fill`] also builds: the node with as many of the requested children as fit, each with as much of its own subtree as fits, and at least one whenever the node has a child at `start`.
//!
//! The mock document's tree is small, so these functions take any tree; the tests build large synthetic ones (spec §15, `a11y-limits.json`).
//!
//! [spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/engine-protocol.md

use std::collections::VecDeque;

use serde::Serialize;

use crate::limits::MAX_MESSAGE_BYTES;
use crate::protocol::{
    A11yCaret, A11yNode, A11yRole, A11ySelection, CharBox, ChildRange, PageRect, TextDirection,
    TextRun,
};

/// The deepest an accessibility tree may be, the document node being level 1.
pub const MAX_DEPTH: u32 = 64;

/// The most nodes that one reply to `query.a11y`, or one change of an `a11y.update`, may hold.
pub const MAX_NODES: usize = 200_000;

/// Room in a message for everything outside the nodes of its tree: the envelope and the reply's or update's other fields.
const MESSAGE_ROOM: usize = 4096;

/// The bytes that a node adds to its parent's `children` beyond its own fields: the comma before it.
const SEPARATOR: usize = 1;

/// The most bytes that `,"child_count":4294967295` takes, which a node with children may carry.
const CHILD_COUNT_FIELD: usize = 26;

/// The bytes that `,"children":[]` takes: the list of a node's children, without the children.
const CHILDREN_FIELD: usize = 14;

/// The most bytes that `,"children_start":4294967295` and `,"children":[]` take together, which a node carries once it holds a child.
const CHILDREN_FIELDS: usize = 29 + CHILDREN_FIELD;

/// The limits that a tree is kept within.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The most nodes the tree may hold.
    pub nodes: usize,
    /// The most bytes the tree's JSON may take.
    pub bytes: usize,
}

impl Limits {
    /// The limits of the protocol for a tree that one message holds alone: [`MAX_NODES`] nodes, and the message limit less room for the envelope.
    pub const PROTOCOL: Self = Self {
        nodes: MAX_NODES,
        bytes: MAX_MESSAGE_BYTES - MESSAGE_ROOM,
    };
}

/// The length of a string in UTF-16 code units, the unit of every text offset (spec §7).
fn utf16_len(text: &str) -> u32 {
    u32::try_from(text.encode_utf16().count()).unwrap_or(u32::MAX)
}

/// Gives every node at level [`MAX_DEPTH`] of a document tree (whose root is level 1) no children, appending the text of the nodes below it to its own text in reading order, each starting on a new line, with their character boxes (spec §14.7). A caret in one of those nodes moves to the same character in the node they were appended to. The appended text keeps its language and direction, as runs where they differ from the node's, and its lines, each text starting a line.
pub fn flatten(root: &mut A11yNode, caret: &mut Option<A11yCaret>) {
    flatten_with_selection(root, caret, &mut []);
}

/// [`flatten`], which also moves the ends of a selection that lie in the nodes it appends (spec §14.7 item 2), as it moves the caret.
pub fn flatten_with_selection(
    root: &mut A11yNode,
    caret: &mut Option<A11yCaret>,
    selection: &mut [A11ySelection],
) {
    let mut positions = Positions::of(caret, selection);
    flatten_node(root, 1, MAX_DEPTH, &Context::default(), &mut positions);
}

/// [`flatten`] for a node at `level`, with any depth limit; the node's ancestors set no language or direction.
pub fn flatten_from(node: &mut A11yNode, level: u32, depth: u32, caret: &mut Option<A11yCaret>) {
    let mut positions = Positions::of(caret, &mut []);
    flatten_node(node, level, depth, &Context::default(), &mut positions);
}

/// The language and direction of a node: its own, or those it inherits (spec §14.7).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Context {
    lang: Option<String>,
    dir: Option<TextDirection>,
}

impl Context {
    /// The language and direction of `node`, a child of a node with these.
    fn of(&self, node: &A11yNode) -> Self {
        Self {
            lang: node.lang.clone().or_else(|| self.lang.clone()),
            dir: node.dir.or(self.dir),
        }
    }

    /// The run of `start` to `end` in a node with the language and direction `node`, for text with these: `None` where both are the node's.
    fn run(&self, node: &Self, start: u32, end: u32) -> Option<TextRun> {
        let lang = self.lang.clone().filter(|_| self.lang != node.lang);
        let dir = self.dir.filter(|_| self.dir != node.dir);
        (lang.is_some() || dir.is_some()).then_some(TextRun {
            start,
            end,
            lang,
            dir,
        })
    }
}

/// The text positions that move with the text they point into when [`flatten`] appends it to another node: the caret and the ends of a selection.
struct Positions<'a> {
    positions: Vec<(&'a mut u64, &'a mut u32)>,
}

impl<'a> Positions<'a> {
    fn of(caret: &'a mut Option<A11yCaret>, selection: &'a mut [A11ySelection]) -> Self {
        let mut positions = Vec::new();
        if let Some(caret) = caret.as_mut() {
            positions.push((&mut caret.node, &mut caret.offset));
        }
        for range in selection {
            positions.push((&mut range.anchor.node, &mut range.anchor.offset));
            positions.push((&mut range.focus.node, &mut range.focus.offset));
        }
        Self { positions }
    }

    /// Moves every position in node `from` to node `to`: to `offset` further on, or, for a node without text, to `offset` itself.
    fn relocate(&mut self, from: u64, to: u64, offset: u32, has_text: bool) {
        for (node, at) in &mut self.positions {
            if **node == from {
                **node = to;
                **at = if has_text {
                    offset.saturating_add(**at)
                } else {
                    offset
                };
            }
        }
    }
}

/// The parts of a node's own text, `length` UTF-16 code units long, as `(start, end, language and direction)`: its runs, and the rest with the node's own, in order. Runs out of order or outside the text are left out.
fn segments(length: u32, own: &Context, runs: &[TextRun]) -> Vec<(u32, u32, Context)> {
    let mut parts = Vec::new();
    let mut at = 0;
    for run in runs {
        if run.start < at || run.start >= run.end || run.end > length {
            continue;
        }
        if at < run.start {
            parts.push((at, run.start, own.clone()));
        }
        let context = Context {
            lang: run.lang.clone().or_else(|| own.lang.clone()),
            dir: run.dir.or(own.dir),
        };
        parts.push((run.start, run.end, context));
        at = run.end;
    }
    if at < length {
        parts.push((at, length, own.clone()));
    }
    parts
}

/// [`flatten`] for a node at `level` whose parent has the language and direction `inherited`.
fn flatten_node(
    node: &mut A11yNode,
    level: u32,
    depth: u32,
    inherited: &Context,
    positions: &mut Positions<'_>,
) {
    let here = inherited.of(node);
    if level < depth {
        for child in node.children.iter_mut().flatten() {
            flatten_node(child, level + 1, depth, &here, positions);
        }
        return;
    }
    let Some(children) = node.children.take() else {
        return;
    };
    let mut text = node.text.take();
    let mut chars = node.chars.take();
    let mut runs = node.runs.take().unwrap_or_default();
    let own_starts = node.line_starts.take();
    let mut starts = match (&text, &own_starts) {
        (Some(_), Some(starts)) => starts.clone(),
        (Some(_), None) => vec![0],
        (None, _) => Vec::new(),
    };
    let mut appended = false;
    // Depth first, in reading order, with a stack of its own: below the limit the tree may be deeper than the stack allows recursion, and each node is dropped once its children are taken out, so dropping never recurses either. Each node waits with the language and direction of its parent.
    let mut pending: Vec<(A11yNode, Context)> = children
        .into_iter()
        .rev()
        .map(|child| (child, here.clone()))
        .collect();
    while let Some((mut below, parent)) = pending.pop() {
        let context = parent.of(&below);
        if let Some(grandchildren) = below.children.take() {
            pending.extend(
                grandchildren
                    .into_iter()
                    .rev()
                    .map(|child| (child, context.clone())),
            );
        }
        let length = text.as_deref().map_or(0, utf16_len);
        let Some(own) = below.text.take() else {
            // A node without text adds nothing; a position in it moves to the end of the text so far.
            positions.relocate(below.id, node.id, length, false);
            continue;
        };
        appended = true;
        let joined = text.get_or_insert_with(String::new);
        let offset = if joined.is_empty() {
            0
        } else {
            joined.push('\n');
            length + 1
        };
        joined.push_str(&own);
        positions.relocate(below.id, node.id, offset, true);
        // Its text starts a line, and keeps the lines it had.
        if offset > 0 || starts.is_empty() {
            starts.push(offset);
        }
        for &start in below
            .line_starts
            .iter()
            .flatten()
            .filter(|&&start| start > 0)
        {
            starts.push(offset.saturating_add(start));
        }
        // Its language and direction, where they differ from the node's, become runs.
        let own_runs = below.runs.take().unwrap_or_default();
        for (start, end, part) in segments(utf16_len(&own), &context, &own_runs) {
            runs.extend(part.run(
                &here,
                offset.saturating_add(start),
                offset.saturating_add(end),
            ));
        }
        if let Some(boxes) = below.chars.take() {
            chars
                .get_or_insert_with(Vec::new)
                .extend(boxes.into_iter().map(|character| CharBox {
                    start: character.start.saturating_add(offset),
                    end: character.end.saturating_add(offset),
                    rect: character.rect,
                }));
        }
    }
    node.text = text;
    node.chars = chars;
    node.runs = (!runs.is_empty()).then_some(runs);
    node.line_starts = if appended { Some(starts) } else { own_starts };
}

/// A node's own fields, without its children, as they appear in JSON: to measure a node.
#[derive(Serialize)]
struct OwnFields<'a> {
    id: u64,
    role: A11yRole,
    #[serde(skip_serializing_if = "Option::is_none")]
    level: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<&'a str>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    editable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    bounds: Option<&'a PageRect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chars: Option<&'a [CharBox]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lang: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dir: Option<TextDirection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runs: Option<&'a [TextRun]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line_starts: Option<&'a [u32]>,
}

/// Counts the bytes written to it.
struct Counter(usize);

impl std::io::Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// An upper bound of the bytes that a node takes in a tree's JSON, its children and the fields that list them not counted, but its `child_count` if it has children: whether it holds them all is known only at the end. [`CHILDREN_FIELDS`] is added when it gets its first child. A node without children whose list of children is empty rather than absent keeps that empty list in the message, so its cost counts the list.
fn cost(node: &A11yNode) -> usize {
    // Every field is named, so that a field added to the node cannot be left out of its measure: the compiler refuses this pattern until it names the new field too.
    let A11yNode {
        id,
        role,
        level,
        name,
        text,
        editable,
        bounds,
        chars,
        lang,
        dir,
        runs,
        line_starts,
        child_count: _,
        children_start: _,
        children,
    } = node;
    let fields = OwnFields {
        id: *id,
        role: *role,
        level: *level,
        name: name.as_deref(),
        text: text.as_deref(),
        editable: *editable,
        bounds: bounds.as_ref(),
        chars: chars.as_deref(),
        lang: lang.as_deref(),
        dir: *dir,
        runs: runs.as_deref(),
        line_starts: line_starts.as_deref(),
    };
    let mut counter = Counter(0);
    let list_fields = match children.as_deref() {
        None => 0,
        Some([]) => CHILDREN_FIELD,
        Some(_) => CHILD_COUNT_FIELD,
    };
    match serde_json::to_writer(&mut counter, &fields) {
        Ok(()) => counter.0.saturating_add(SEPARATOR + list_fields),
        Err(_) => usize::MAX,
    }
}

/// A node's children.
fn children_of(node: &A11yNode) -> &[A11yNode] {
    node.children.as_deref().unwrap_or_default()
}

/// The indices of the children along the path from `root` to the node `target`, if it is in the subtree; empty if `target` is `root`. The tree is at most [`MAX_DEPTH`] deep once flattened, so the search recurses no deeper.
fn path_to(root: &A11yNode, target: u64) -> Option<Vec<usize>> {
    if root.id == target {
        return Some(Vec::new());
    }
    children_of(root)
        .iter()
        .enumerate()
        .find_map(|(index, child)| {
            path_to(child, target).map(|mut path| {
                path.insert(0, index);
                path
            })
        })
}

/// Whether a whole subtree fits within the limits.
fn fits_whole(root: &A11yNode, limits: &Limits) -> bool {
    let (mut nodes, mut bytes) = (0_usize, 0_usize);
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        nodes += 1;
        bytes = bytes.saturating_add(cost(node));
        if !children_of(node).is_empty() {
            bytes = bytes.saturating_add(CHILDREN_FIELDS);
        }
        if nodes > limits.nodes || bytes > limits.bytes {
            return false;
        }
        pending.extend(children_of(node));
    }
    true
}

/// A node chosen for the message, and the run of its children chosen with it.
struct Chosen<'a> {
    node: &'a A11yNode,
    /// The index of the first child of the run.
    run_start: usize,
    /// The chosen children, in order, as indices into the list of chosen nodes.
    run: Vec<usize>,
}

/// The part of the subtree of `root` that one message holds (spec §14.7): the whole subtree if it fits within `limits`; otherwise a part filled breadth-first in reading order, which always holds the path from `root` to the caret's node, if the caret is in the subtree. With `children`, the answer to a `query.a11y` with a range of `root`'s children: `root` with as many of those children as fit, each with as much of its own subtree as fits, and at least one whenever `root` has a child at `range.start`; the path to the caret does not count then.
///
/// The tree must already be flattened ([`flatten`]). A node's own fields always come whole, so a single node larger than `limits.bytes` makes the result larger too; the message limit then refuses it (spec §14.9).
#[must_use]
pub fn fill(
    root: &A11yNode,
    caret: Option<A11yCaret>,
    children: Option<ChildRange>,
    limits: &Limits,
) -> A11yNode {
    if children.is_none() && fits_whole(root, limits) {
        return root.clone();
    }
    let path = match children {
        None => caret
            .and_then(|caret| path_to(root, caret.node))
            .unwrap_or_default(),
        Some(_) => Vec::new(),
    };
    let mut chosen = vec![Chosen {
        node: root,
        run_start: 0,
        run: Vec::new(),
    }];
    let mut nodes = 1_usize;
    let mut bytes = cost(root);
    // The path to the caret comes first, whatever it costs: each of its nodes holds the part of its children that starts with the child on the path.
    let mut at = 0;
    for &index in &path {
        let Some(child) = children_of(chosen[at].node).get(index) else {
            break;
        };
        chosen.push(Chosen {
            node: child,
            run_start: 0,
            run: Vec::new(),
        });
        let next = chosen.len() - 1;
        chosen[at].run_start = index;
        chosen[at].run.push(next);
        nodes += 1;
        bytes = bytes.saturating_add(cost(child).saturating_add(CHILDREN_FIELDS));
        at = next;
    }
    // A range's first child comes whatever it costs too, so that a node's children are never refused for their number alone.
    let (first, last) = match children {
        Some(range) => {
            let total = children_of(root).len();
            let start = usize::try_from(range.start).unwrap_or(usize::MAX);
            let count = usize::try_from(range.count).unwrap_or(usize::MAX);
            (start.min(total), start.saturating_add(count).min(total))
        }
        None => (0, usize::MAX),
    };
    chosen[0].run_start = if path.is_empty() {
        first
    } else {
        chosen[0].run_start
    };
    if children.is_some() && first < last {
        let child = &children_of(root)[first];
        chosen.push(Chosen {
            node: child,
            run_start: 0,
            run: Vec::new(),
        });
        let next = chosen.len() - 1;
        chosen[0].run.push(next);
        nodes += 1;
        bytes = bytes.saturating_add(cost(child).saturating_add(CHILDREN_FIELDS));
    }
    // Then breadth first, in reading order, until a node does not fit.
    let mut queue: VecDeque<usize> = VecDeque::from([0]);
    'fill: while let Some(parent) = queue.pop_front() {
        let all = children_of(chosen[parent].node);
        let start = chosen[parent].run_start;
        let end = if parent == 0 {
            last.min(all.len())
        } else {
            all.len()
        };
        let already = chosen[parent].run.len();
        // The children chosen already (on the path, or a range's first child) start the run.
        for &child in &chosen[parent].run.clone() {
            queue.push_back(child);
        }
        for child in all.iter().take(end).skip(start + already) {
            let first = chosen[parent].run.is_empty();
            let added = cost(child).saturating_add(if first { CHILDREN_FIELDS } else { 0 });
            if nodes + 1 > limits.nodes || bytes.saturating_add(added) > limits.bytes {
                break 'fill;
            }
            nodes += 1;
            bytes += added;
            chosen.push(Chosen {
                node: child,
                run_start: 0,
                run: Vec::new(),
            });
            let next = chosen.len() - 1;
            chosen[parent].run.push(next);
            queue.push_back(next);
        }
    }
    build(&chosen, 0)
}

/// The node `index` of the chosen nodes, with its chosen children; recurses no deeper than the tree, which is at most [`MAX_DEPTH`] deep.
fn build(chosen: &[Chosen<'_>], index: usize) -> A11yNode {
    let Chosen {
        node,
        run_start,
        run,
    } = &chosen[index];
    let total = children_of(node).len();
    let held: Vec<A11yNode> = run.iter().map(|&child| build(chosen, child)).collect();
    let (child_count, children_start, children) = if total == 0 {
        (None, None, node.children.as_ref().map(|_| Vec::new()))
    } else if held.len() == total {
        (None, None, Some(held))
    } else if held.is_empty() {
        (Some(total), None, None)
    } else {
        (Some(total), Some(*run_start), Some(held))
    };
    // Every field is named, so that a field added to the node cannot be dropped here unnoticed.
    let A11yNode {
        id,
        role,
        level,
        name,
        text,
        editable,
        bounds,
        chars,
        lang,
        dir,
        runs,
        line_starts,
        child_count: _,
        children_start: _,
        children: _,
    } = node;
    A11yNode {
        id: *id,
        role: *role,
        level: *level,
        name: name.clone(),
        text: text.clone(),
        editable: *editable,
        bounds: *bounds,
        chars: chars.clone(),
        lang: lang.clone(),
        dir: *dir,
        runs: runs.clone(),
        line_starts: line_starts.clone(),
        child_count: child_count.map(|count| u32::try_from(count).unwrap_or(u32::MAX)),
        children_start: children_start.map(|start| u32::try_from(start).unwrap_or(u32::MAX)),
        children,
    }
}

/// The node `id` in a tree, if there is one; the tree is at most [`MAX_DEPTH`] deep once flattened.
#[must_use]
pub fn find(root: &A11yNode, id: u64) -> Option<&A11yNode> {
    if root.id == id {
        return Some(root);
    }
    children_of(root).iter().find_map(|child| find(child, id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Rect, TextPosition};

    fn paragraph(id: u64, text: &str) -> A11yNode {
        A11yNode {
            id,
            role: A11yRole::Paragraph,
            level: None,
            name: None,
            text: Some(text.to_owned()),
            editable: false,
            bounds: None,
            chars: None,
            lang: None,
            dir: None,
            runs: None,
            line_starts: None,
            child_count: None,
            children_start: None,
            children: None,
        }
    }

    fn with_children(mut node: A11yNode, children: Vec<A11yNode>) -> A11yNode {
        node.children = Some(children);
        node
    }

    /// A chain of nodes from level 1 to `depth`, node `n` at level `n` with the text `n`.
    fn chain(depth: u64) -> A11yNode {
        let mut node = paragraph(depth, &depth.to_string());
        for id in (1..depth).rev() {
            node = with_children(paragraph(id, &id.to_string()), vec![node]);
        }
        node
    }

    #[test]
    fn a_node_at_the_depth_limit_holds_the_text_below_it() {
        let mut tree = chain(5);
        let mut caret = Some(A11yCaret { node: 5, offset: 1 });
        flatten_from(&mut tree, 1, 3, &mut caret);
        let third = &tree.children.as_ref().unwrap()[0]
            .children
            .as_ref()
            .unwrap()[0];
        assert_eq!(third.id, 3);
        assert_eq!(third.children, None);
        assert_eq!(third.text.as_deref(), Some("3\n4\n5"));
        // "5" starts at offset 4; the caret was after it.
        assert_eq!(caret, Some(A11yCaret { node: 3, offset: 5 }));
    }

    #[test]
    fn character_boxes_move_with_their_text() {
        let rect = Rect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        let mut below = paragraph(3, "ab");
        below.chars = Some(vec![CharBox {
            start: 1,
            end: 2,
            rect,
        }]);
        let mut tree = with_children(
            paragraph(1, "x"),
            vec![with_children(paragraph(2, "😀"), vec![below])],
        );
        let mut caret = None;
        flatten_from(&mut tree, 1, 2, &mut caret);
        let flat = &tree.children.as_ref().unwrap()[0];
        // "😀" is two UTF-16 code units, then a new line: "ab" starts at offset 3.
        assert_eq!(flat.text.as_deref(), Some("😀\nab"));
        assert_eq!(flat.chars.as_ref().unwrap()[0].start, 4);
    }

    #[test]
    fn a_tree_that_fits_comes_whole() {
        let tree = with_children(
            paragraph(1, "root"),
            (2..12).map(|id| paragraph(id, "x")).collect(),
        );
        assert_eq!(fill(&tree, None, None, &Limits::PROTOCOL), tree);
    }

    #[test]
    fn a_tree_that_does_not_fit_is_split_with_the_path_to_the_caret() {
        let tree = with_children(
            paragraph(1, "root"),
            (2..12).map(|id| paragraph(id, "x")).collect(),
        );
        let limits = Limits {
            nodes: 4,
            bytes: usize::MAX,
        };
        let part = fill(&tree, Some(A11yCaret { node: 8, offset: 0 }), None, &limits);
        assert_eq!(part.child_count, Some(10));
        // Child 8 is at index 6: the run starts there.
        assert_eq!(part.children_start, Some(6));
        let ids: Vec<u64> = part
            .children
            .unwrap()
            .iter()
            .map(|child| child.id)
            .collect();
        assert_eq!(ids, [8, 9, 10]);
        // Without a caret, the run starts at the first child.
        let part = fill(&tree, None, None, &limits);
        assert_eq!((part.child_count, part.children_start), (Some(10), Some(0)));
        // A range of children: the answer starts at `start` and holds at least one child.
        let tight = Limits { nodes: 1, bytes: 1 };
        let range = ChildRange { start: 9, count: 5 };
        let part = fill(&tree, None, Some(range), &tight);
        assert_eq!((part.child_count, part.children_start), (Some(10), Some(9)));
        assert_eq!(part.children.unwrap().len(), 1);
        // Past the last child, none.
        let part = fill(
            &tree,
            None,
            Some(ChildRange {
                start: 10,
                count: 5,
            }),
            &Limits::PROTOCOL,
        );
        assert_eq!(
            (part.child_count, part.children_start, part.children),
            (Some(10), None, None)
        );
    }

    #[test]
    fn cost_is_an_upper_bound_of_the_json() {
        let mut node = paragraph(9_007_199_254_740_991, "\"quoted\" \u{1F600}");
        node.children = Some(vec![paragraph(2, "child")]);
        let child_cost = cost(&node.children.as_ref().unwrap()[0]);
        let own_cost = cost(&node) + CHILDREN_FIELDS;
        node.child_count = Some(u32::MAX);
        node.children_start = Some(u32::MAX);
        let json = serde_json::to_string(&node).unwrap();
        // The node with all its fields, a separator before it, and its child.
        assert!(json.len() + SEPARATOR <= own_cost + child_cost, "{json}");
        // A leaf costs exactly its JSON and a separator, also when its list of children is empty rather than absent.
        let leaf = paragraph(3, "leaf");
        assert_eq!(
            cost(&leaf),
            serde_json::to_string(&leaf).unwrap().len() + SEPARATOR
        );
        let mut empty = paragraph(4, "empty");
        empty.children = Some(Vec::new());
        assert_eq!(
            cost(&empty),
            serde_json::to_string(&empty).unwrap().len() + SEPARATOR
        );
    }

    #[test]
    fn cost_counts_every_field_of_a_node() {
        // A leaf with every field set costs exactly its JSON and a separator.
        let mut leaf = paragraph(5, "مرحبا hello");
        leaf.level = Some(2);
        leaf.name = Some("name".to_owned());
        leaf.editable = true;
        leaf.bounds = Some(PageRect {
            page: 1,
            rect: Rect {
                x: -1,
                y: 2,
                width: 3,
                height: 4,
            },
        });
        leaf.chars = Some(vec![CharBox {
            start: 0,
            end: 1,
            rect: Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
        }]);
        leaf.lang = Some("ar".to_owned());
        leaf.dir = Some(TextDirection::Rtl);
        leaf.runs = Some(vec![TextRun {
            start: 6,
            end: 11,
            lang: Some("en".to_owned()),
            dir: Some(TextDirection::Ltr),
        }]);
        leaf.line_starts = Some(vec![0, 6]);
        assert_eq!(
            cost(&leaf),
            serde_json::to_string(&leaf).unwrap().len() + SEPARATOR
        );
        // A split tree keeps every field of the nodes it holds.
        let root = with_children(paragraph(1, "root"), vec![leaf.clone(), paragraph(6, "x")]);
        let limits = Limits {
            nodes: 2,
            bytes: usize::MAX,
        };
        let part = fill(&root, None, None, &limits);
        assert_eq!(part.children.as_ref().unwrap()[0], leaf);
    }

    /// A node with a language and a direction of its own.
    fn speaking(mut node: A11yNode, lang: &str, dir: TextDirection) -> A11yNode {
        node.lang = Some(lang.to_owned());
        node.dir = Some(dir);
        node
    }

    #[test]
    fn flattening_keeps_languages_directions_lines_and_the_selection() {
        // Level 2 holds "ab" (lines at 0 and 1); below it, in reading order: an Arabic node with an English run, a node without text, and an English node with two lines.
        let mut arabic = speaking(paragraph(4, "سلام hi"), "ar", TextDirection::Rtl);
        arabic.runs = Some(vec![TextRun {
            start: 5,
            end: 7,
            lang: Some("en".to_owned()),
            dir: Some(TextDirection::Ltr),
        }]);
        arabic.line_starts = Some(vec![0]);
        let mut english = paragraph(6, "one two");
        english.line_starts = Some(vec![0, 4]);
        let mut wrapper = with_children(paragraph(5, ""), vec![english]);
        wrapper.text = None;
        let mut level_2 = with_children(paragraph(2, "ab"), vec![arabic, wrapper]);
        level_2.line_starts = Some(vec![0, 1]);
        let mut root = speaking(
            with_children(paragraph(1, ""), vec![level_2]),
            "en",
            TextDirection::Ltr,
        );
        root.text = None;
        let mut caret = Some(A11yCaret { node: 6, offset: 4 });
        let mut selection = [A11ySelection {
            anchor: TextPosition { node: 4, offset: 1 },
            focus: TextPosition { node: 5, offset: 0 },
        }];
        let mut positions = Positions::of(&mut caret, &mut selection);
        flatten_node(&mut root, 1, 2, &Context::default(), &mut positions);
        let flat = &root.children.as_ref().unwrap()[0];
        assert_eq!(flat.children, None);
        // "ab", then the Arabic text on a new line at 3, then the English text at 11.
        assert_eq!(flat.text.as_deref(), Some("ab\nسلام hi\none two"));
        assert_eq!(flat.line_starts.as_deref(), Some(&[0, 1, 3, 11, 15][..]));
        // The Arabic part differs from the node's English, left to right; its English run does not.
        let runs: Vec<(u32, u32, Option<&str>, Option<TextDirection>)> = flat
            .runs
            .iter()
            .flatten()
            .map(|run| (run.start, run.end, run.lang.as_deref(), run.dir))
            .collect();
        assert_eq!(runs, [(3, 8, Some("ar"), Some(TextDirection::Rtl))]);
        // The positions move with their text: the node without text gives the end of the text before it.
        assert_eq!(
            caret,
            Some(A11yCaret {
                node: 2,
                offset: 15
            })
        );
        assert_eq!(
            selection,
            [A11ySelection {
                anchor: TextPosition { node: 2, offset: 4 },
                focus: TextPosition {
                    node: 2,
                    offset: 10
                },
            }]
        );
    }

    #[test]
    fn a_flattened_node_keeps_its_own_lines_when_nothing_is_appended() {
        let mut silent = paragraph(3, "");
        silent.text = None;
        let mut node = with_children(paragraph(2, "only"), vec![silent]);
        node.line_starts = None;
        let mut root = with_children(paragraph(1, ""), vec![node]);
        root.text = None;
        flatten_from(&mut root, 1, 2, &mut None);
        let flat = &root.children.as_ref().unwrap()[0];
        assert_eq!(
            (flat.text.as_deref(), flat.line_starts.as_ref()),
            (Some("only"), None)
        );
    }

    #[test]
    fn empty_lists_of_children_count_towards_the_byte_limit() {
        // Children whose lists of children are empty rather than absent keep those lists in the message.
        let leaves = (2..1_002)
            .map(|id| {
                let mut leaf = paragraph(id, "x");
                leaf.children = Some(Vec::new());
                leaf
            })
            .collect();
        let root = with_children(paragraph(1, "root"), leaves);
        let whole = serde_json::to_string(&root).unwrap().len();
        for bytes in [whole, whole - 1, whole / 2, 100] {
            let limits = Limits {
                nodes: MAX_NODES,
                bytes,
            };
            let part = fill(&root, None, None, &limits);
            let json = serde_json::to_string(&part).unwrap();
            assert!(json.len() <= bytes, "{} > {bytes}", json.len());
        }
    }
}
