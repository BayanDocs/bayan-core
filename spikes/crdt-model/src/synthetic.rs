//! The synthetic documents the benchmarks load: the brief's full 500-page document in the BayanDocs model, and the text-with-marks-and-block-markers subset that the Automerge comparison builds identically.

use bayan_crdt::{Doc, PeerId, PropertyMap, Story, Value};
use bayan_model::{AtomKind, EntityId, IdGenerator, encode_binding, marks, registry};
use crdt_workload::{DOCUMENT_SEED, Rng, Shape, comments, paragraphs, sequence_len};

/// Counts of what a synthetic document contains.
#[derive(Debug, Default, Clone, Copy)]
pub struct Contents {
    /// Paragraphs of the main story.
    pub paragraphs: usize,
    /// Tables.
    pub tables: usize,
    /// Table cells (each with its own story).
    pub cells: usize,
    /// Comments.
    pub comments: usize,
    /// Fields.
    pub fields: usize,
    /// Objects.
    pub objects: usize,
    /// Bookmarks.
    pub bookmarks: usize,
    /// Atoms in the main story.
    pub main_len: usize,
}

fn binding(kind: AtomKind, id: EntityId) -> String {
    encode_binding(kind, id)
}

/// A new entity in `registry_name`, with properties.
fn entity(
    doc: &Doc,
    registry_name: &str,
    id: EntityId,
    props: &[(&str, Value)],
) -> Result<PropertyMap, String> {
    let map = doc
        .registry(registry_name)
        .create(&id.to_string())
        .map_err(|error| error.to_string())?;
    for (key, value) in props {
        map.set(key, value).map_err(|error| error.to_string())?;
    }
    Ok(map)
}

/// A new story holding `text` and its final paragraph end.
fn story(doc: &Doc, ids: &mut IdGenerator, text: &str) -> Result<EntityId, String> {
    let id = ids.next_id();
    let handle = doc
        .stories()
        .create(&id.to_string())
        .map_err(|error| error.to_string())?;
    handle.insert(0, text).map_err(|error| error.to_string())?;
    end_paragraph(doc, ids, &handle, "Normal")?;
    Ok(id)
}

/// Appends a paragraph end with a new paragraph of `style`.
fn end_paragraph(
    doc: &Doc,
    ids: &mut IdGenerator,
    handle: &Story,
    style: &str,
) -> Result<(), String> {
    let paragraph = ids.next_id();
    entity(
        doc,
        registry::PARAGRAPHS,
        paragraph,
        &[("style", Value::from(style))],
    )?;
    handle
        .insert_atom(
            handle.len(),
            AtomKind::ParagraphEnd.placeholder(),
            &binding(AtomKind::ParagraphEnd, paragraph),
        )
        .map_err(|error| error.to_string())
}

/// Appends a table of `rows` × `columns` cells at the end of the main story (a block position, right after a paragraph end).
fn table(
    doc: &Doc,
    ids: &mut IdGenerator,
    main: &Story,
    rows: usize,
    columns: usize,
) -> Result<usize, String> {
    let table = ids.next_id();
    let map = entity(
        doc,
        registry::TABLES,
        table,
        &[("style", Value::from("TableGrid"))],
    )?;
    let row_list = map
        .id_list(registry::ROWS_KEY)
        .map_err(|error| error.to_string())?;
    let mut cells = 0;
    for row_index in 0..rows {
        let row = ids.next_id();
        let row_map = entity(doc, registry::ROWS, row, &[])?;
        let cell_list = row_map
            .id_list(registry::CELLS_KEY)
            .map_err(|error| error.to_string())?;
        for column in 0..columns {
            let cell = ids.next_id();
            let cell_story = story(doc, ids, &format!("Cell {row_index},{column}"))?;
            entity(
                doc,
                registry::CELLS,
                cell,
                &[
                    ("w", Value::Int(2_000)),
                    (registry::STORY, Value::Str(cell_story.to_string())),
                ],
            )?;
            cell_list
                .insert(column, &cell.to_string())
                .map_err(|error| error.to_string())?;
            cells += 1;
        }
        row_list
            .insert(row_index, &row.to_string())
            .map_err(|error| error.to_string())?;
    }
    main.insert_atom(
        main.len(),
        AtomKind::TableBlock.placeholder(),
        &binding(AtomKind::TableBlock, table),
    )
    .map_err(|error| error.to_string())?;
    Ok(cells)
}

/// The brief's synthetic document in the BayanDocs model: the paragraphs of the shared workload (about 100 characters each, with bold and italic runs), a field in every 30th paragraph, an object in every 150th, a bookmark in every 75th, a table of 5 × 4 cells after every 75th paragraph, every 20th paragraph a heading, and comments over random ranges, each with its own story and a reference atom. Committed every 100 paragraphs, as an import would.
///
/// # Errors
///
/// A description of what failed.
pub fn full(shape: &Shape) -> Result<(Doc, Contents), String> {
    let doc = Doc::new(PeerId(1), &marks::FAMILIES).map_err(|error| error.to_string())?;
    let mut ids = IdGenerator::new(DOCUMENT_SEED);
    let mut rng = Rng::new(DOCUMENT_SEED);
    let texts = paragraphs(&mut rng, shape);
    let main = doc.main_story();
    let table_every = (shape.paragraphs / shape.tables.max(1)).max(1);
    let mut contents = Contents::default();
    let err = |error: bayan_crdt::CrdtError| error.to_string();
    for (index, paragraph) in texts.iter().enumerate() {
        let start = main.len();
        main.insert(start, &paragraph.text).map_err(err)?;
        if let Some(bold) = &paragraph.bold {
            main.mark(
                start + bold.start..start + bold.end,
                marks::BOLD,
                &Value::Bool(true),
            )
            .map_err(err)?;
        }
        if let Some(italic) = &paragraph.italic {
            main.mark(
                start + italic.start..start + italic.end,
                marks::ITALIC,
                &Value::Bool(true),
            )
            .map_err(err)?;
        }
        let len = paragraph.chars();
        if index % 75 == 13 && len > 3 {
            let bookmark = ids.next_id();
            entity(
                &doc,
                registry::RANGES,
                bookmark,
                &[
                    ("kind", Value::from("bookmark")),
                    ("name", Value::from(format!("b{index}"))),
                ],
            )?;
            main.insert_atom(
                start + 3,
                AtomKind::RangeEnd.placeholder(),
                &binding(AtomKind::RangeEnd, bookmark),
            )
            .map_err(err)?;
            main.insert_atom(
                start,
                AtomKind::RangeStart.placeholder(),
                &binding(AtomKind::RangeStart, bookmark),
            )
            .map_err(err)?;
            contents.bookmarks += 1;
        }
        if index % 150 == 11 {
            let object = ids.next_id();
            entity(
                &doc,
                registry::OBJECTS,
                object,
                &[
                    ("kind", Value::from("picture")),
                    ("cx", Value::Int(914_400)),
                    ("cy", Value::Int(914_400)),
                ],
            )?;
            main.insert_atom(
                start + 1,
                AtomKind::ObjectAnchor.placeholder(),
                &binding(AtomKind::ObjectAnchor, object),
            )
            .map_err(err)?;
            contents.objects += 1;
        }
        if index % 30 == 7 {
            let field = ids.next_id();
            entity(
                &doc,
                registry::FIELDS,
                field,
                &[("instr", Value::from("PAGE"))],
            )?;
            let mut pos = main.len();
            for (kind, text) in [
                (AtomKind::FieldBegin, " PAGE "),
                (AtomKind::FieldSeparator, "7"),
                (AtomKind::FieldEnd, ""),
            ] {
                main.insert_atom(pos, kind.placeholder(), &binding(kind, field))
                    .map_err(err)?;
                pos += 1;
                main.insert(pos, text).map_err(err)?;
                pos += text.chars().count();
            }
            contents.fields += 1;
        }
        let style = if index % 20 == 0 {
            "Heading1"
        } else {
            "Normal"
        };
        end_paragraph(&doc, &mut ids, &main, style)?;
        contents.paragraphs += 1;
        // Halfway through each stretch of paragraphs, so never after the last one: the story still ends with a paragraph end.
        if (index + 1) % table_every == table_every / 2 && contents.tables < shape.tables {
            contents.cells += table(&doc, &mut ids, &main, 5, 4)?;
            contents.tables += 1;
        }
        if index % 100 == 99 {
            doc.commit();
        }
    }
    let body = doc.root_map(registry::BODY);
    for (key, value) in bayan_model::SECTION_DEFAULTS {
        body.set(key, &Value::Int(value)).map_err(err)?;
    }
    doc.commit();
    let table_block = AtomKind::TableBlock.placeholder();
    let mut placed = 0;
    while placed < shape.comments {
        let len = main.len();
        let start = rng.below(len.saturating_sub(202));
        let end = start + rng.between(20, 200);
        // A reference right before a table at a block position would put text before the table in its paragraph.
        if main.char_at(end) == Some(table_block) {
            continue;
        }
        let comment = ids.next_id();
        let comment_story = story(&doc, &mut ids, &format!("Comment {placed}"))?;
        entity(
            &doc,
            registry::COMMENTS,
            comment,
            &[
                ("author", Value::from("Reviewer")),
                (registry::STORY, Value::Str(comment_story.to_string())),
            ],
        )?;
        main.insert_atom(
            end,
            AtomKind::CommentReference.placeholder(),
            &binding(AtomKind::CommentReference, comment),
        )
        .map_err(err)?;
        main.mark(start..end, &marks::comment_key(comment), &Value::Bool(true))
            .map_err(err)?;
        placed += 1;
        if placed % 100 == 0 {
            doc.commit();
        }
    }
    doc.commit();
    contents.comments = placed;
    contents.main_len = main.len();
    Ok((doc, contents))
}

/// The subset of the Automerge comparison, built exactly as the Automerge benchmark builds it (same generator calls in the same order): the workload's paragraphs, each ending with a paragraph-end atom bound to a paragraph entity (`{"type": "paragraph"}`), bold and italic runs, and comments as `cmt:<id>` marks only. Committed every 100 paragraphs and every 100 comments.
///
/// # Errors
///
/// A description of what failed.
pub fn subset(shape: &Shape) -> Result<(Doc, Contents), String> {
    let doc = Doc::new(PeerId(1), &marks::FAMILIES).map_err(|error| error.to_string())?;
    let mut rng = Rng::new(DOCUMENT_SEED);
    let texts = paragraphs(&mut rng, shape);
    let ranges = comments(&mut rng, sequence_len(&texts), shape.comments);
    let mut ids = Rng::new(DOCUMENT_SEED ^ 1);
    let main = doc.main_story();
    let err = |error: bayan_crdt::CrdtError| error.to_string();
    for (index, paragraph) in texts.iter().enumerate() {
        let start = main.len();
        main.insert(start, &paragraph.text).map_err(err)?;
        if let Some(bold) = &paragraph.bold {
            main.mark(
                start + bold.start..start + bold.end,
                marks::BOLD,
                &Value::Bool(true),
            )
            .map_err(err)?;
        }
        if let Some(italic) = &paragraph.italic {
            main.mark(
                start + italic.start..start + italic.end,
                marks::ITALIC,
                &Value::Bool(true),
            )
            .map_err(err)?;
        }
        let paragraph_id = ids.hex_id();
        doc.registry(registry::PARAGRAPHS)
            .create(&paragraph_id)
            .and_then(|map| map.set("type", &Value::from("paragraph")))
            .map_err(err)?;
        main.insert_atom(
            main.len(),
            AtomKind::ParagraphEnd.placeholder(),
            &format!("p:{paragraph_id}"),
        )
        .map_err(err)?;
        if index % 100 == 99 {
            doc.commit();
        }
    }
    doc.commit();
    for (index, comment) in ranges.iter().enumerate() {
        let key = format!("cmt:{}", ids.hex_id());
        main.mark(
            comment.start..comment.start + comment.len,
            &key,
            &Value::Bool(true),
        )
        .map_err(err)?;
        if index % 100 == 99 {
            doc.commit();
        }
    }
    doc.commit();
    Ok((
        doc,
        Contents {
            paragraphs: texts.len(),
            comments: ranges.len(),
            main_len: main.len(),
            ..Contents::default()
        },
    ))
}

/// The canonical text of the subset as stored: paragraph texts joined by line feeds (the text before every paragraph-end atom, without the atom). Equal to the workload's `canonical_text` when the document was built correctly, and to what the Automerge benchmark reads back.
#[must_use]
pub fn subset_text(doc: &Doc) -> String {
    let text = doc.main_story().text();
    let mut paragraphs: Vec<&str> = text.split(AtomKind::ParagraphEnd.placeholder()).collect();
    // The text after the last paragraph end is empty.
    paragraphs.pop();
    paragraphs.join("\n")
}
