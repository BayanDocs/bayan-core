//! Property tests of the normalization (CORE-004 AC-2): on random raw documents, including every kind of inconsistency that concurrent edits or a misbehaving replica can leave, the view always satisfies I1–I7, normalization is deterministic (also when the same content is split into runs differently), and it is idempotent (normalizing a view's own raw form changes nothing and repairs nothing).
//!
//! Every case is generated from a seed, so a failure is reproducible; a failing document is shrunk to a small one before it is reported.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use bayan_crdt::{ATOM_KEY, Run, Value};
use bayan_model::simulation::Rng;
use bayan_model::{
    AtomKind, EntityId, Item, MAX_TABLE_DEPTH, RawDocument, RawRow, RawTable, Report, View,
    check_invariants, encode_binding, normalize, registry,
};

/// Cases per run of the test (each a small document full of inconsistencies), unless `BAYAN_NORMALIZATION_CASES` asks for more, as the CORE-004 report's long run does.
fn cases() -> u64 {
    std::env::var("BAYAN_NORMALIZATION_CASES")
        .ok()
        .and_then(|cases| cases.parse().ok())
        .unwrap_or(1_500)
}

fn id(rng: &mut Rng) -> EntityId {
    EntityId((u128::from(rng.next_u64()) << 64) | u128::from(rng.next_u64()))
}

/// The pools of identifiers a random document draws from: most exist in their registry, some do not.
struct Pools {
    stories: Vec<EntityId>,
    paragraphs: Vec<EntityId>,
    tables: Vec<EntityId>,
    rows: Vec<EntityId>,
    cells: Vec<EntityId>,
    objects: Vec<EntityId>,
    fields: Vec<EntityId>,
    comments: Vec<EntityId>,
    ranges: Vec<EntityId>,
    missing: Vec<EntityId>,
}

fn pool(rng: &mut Rng, size: usize) -> Vec<EntityId> {
    (0..=rng.below(size)).map(|_| id(rng)).collect()
}

fn pick(rng: &mut Rng, pool: &[EntityId], missing: &[EntityId]) -> EntityId {
    if rng.chance(1, 10) || pool.is_empty() {
        *rng.pick(missing).unwrap_or(&EntityId(1))
    } else {
        *rng.pick(pool).unwrap_or(&EntityId(1))
    }
}

fn props(rng: &mut Rng) -> BTreeMap<String, Value> {
    let mut props = BTreeMap::new();
    for _ in 0..rng.below(3) {
        let key = ["style", "jc", "w", "author"][rng.below(4)].to_owned();
        let value = match rng.below(4) {
            0 => Value::Int(i64::try_from(rng.below(1000)).unwrap_or(0)),
            1 => Value::from("x"),
            2 => Value::Bool(true),
            _ => Value::List(vec![Value::Int(1)]),
        };
        props.insert(key, value);
    }
    props
}

fn marks(rng: &mut Rng, pools: &Pools) -> BTreeMap<String, Value> {
    let mut marks = BTreeMap::new();
    let choices = rng.below(4);
    if choices >= 1 {
        marks.insert("r:b".to_owned(), Value::Bool(true));
    }
    if choices >= 2 {
        let comment = pick(rng, &pools.comments, &pools.missing);
        marks.insert(format!("cmt:{comment}"), Value::Bool(true));
    }
    if rng.chance(1, 20) {
        marks.insert("link".to_owned(), Value::Null);
    }
    if rng.chance(1, 30) {
        marks.insert("r:sz".to_owned(), Value::Unsupported);
    }
    marks
}

/// A placeholder bound to `id` as an atom of `kind`, with random marks besides.
fn bound(rng: &mut Rng, pools: &Pools, kind: AtomKind, id: EntityId) -> Run {
    let mut marks = marks(rng, pools);
    marks.insert(ATOM_KEY.to_owned(), Value::Str(encode_binding(kind, id)));
    Run {
        text: kind.placeholder().to_string(),
        marks,
    }
}

/// A well-formed field: begin, a code, usually a separator and a result, and the end, with further fields nested in the code or the result (up to `depth` levels). The random tokens around it, and the occasional missing delimiter, then break some of them, so that the property test sees both nested fields that normalization keeps and ones it must repair (I2).
fn field(rng: &mut Rng, pools: &Pools, depth: usize, runs: &mut Vec<Run>) {
    let id = pick(rng, &pools.fields, &pools.missing);
    let part = |rng: &mut Rng, runs: &mut Vec<Run>| {
        runs.push(Run {
            text: ["PAGE", "IF ", " = 1", "x"][rng.below(4)].to_owned(),
            marks: BTreeMap::new(),
        });
        if depth > 0 && rng.chance(1, 2) {
            field(rng, pools, depth - 1, runs);
        }
    };
    let delimiters = [
        AtomKind::FieldBegin,
        AtomKind::FieldSeparator,
        AtomKind::FieldEnd,
    ];
    // Now and then one delimiter is missing, as after a concurrent deletion.
    let missing = if rng.chance(1, 8) {
        Some(delimiters[rng.below(3)])
    } else {
        None
    };
    let delimiter = |rng: &mut Rng, runs: &mut Vec<Run>, kind: AtomKind| {
        if missing != Some(kind) {
            runs.push(bound(rng, pools, kind, id));
        }
    };
    delimiter(rng, runs, AtomKind::FieldBegin);
    part(rng, runs);
    if rng.chance(4, 5) {
        delimiter(rng, runs, AtomKind::FieldSeparator);
        part(rng, runs);
    }
    delimiter(rng, runs, AtomKind::FieldEnd);
}

/// A random story: text in several scripts, placeholders with good, wrong, dangling or missing bindings, stray control characters, and well-formed nested fields that the rest may break.
fn story(rng: &mut Rng, pools: &Pools, max_tokens: usize) -> Vec<Run> {
    let mut runs = Vec::new();
    for _ in 0..rng.below(max_tokens) {
        if rng.chance(1, 8) {
            field(rng, pools, 2, &mut runs);
            continue;
        }
        let mut marks = marks(rng, pools);
        let text = match rng.below(10) {
            0..=3 => ["abc", "\u{0627}\u{0644}", "\u{1F600}", "z", "x y"][rng.below(5)].to_owned(),
            4 => ["\u{01}", "\u{0A}", "\u{1F}", "\u{0C}"][rng.below(4)].to_owned(),
            _ => {
                let kind = AtomKind::ALL[rng.below(AtomKind::ALL.len())];
                let pool = match kind {
                    AtomKind::ParagraphEnd => &pools.paragraphs,
                    AtomKind::FieldBegin | AtomKind::FieldSeparator | AtomKind::FieldEnd => {
                        &pools.fields
                    }
                    AtomKind::ObjectAnchor => &pools.objects,
                    AtomKind::RangeStart | AtomKind::RangeEnd => &pools.ranges,
                    AtomKind::TableBlock => &pools.tables,
                    AtomKind::CommentReference => &pools.comments,
                    AtomKind::Tab => &pools.missing,
                };
                let entity = pick(rng, pool, &pools.missing);
                match rng.below(12) {
                    // Bound to an entity of another kind.
                    0 => {
                        let other = AtomKind::ALL[rng.below(AtomKind::ALL.len())];
                        marks.insert(
                            ATOM_KEY.to_owned(),
                            Value::Str(encode_binding(other, entity)),
                        );
                    }
                    // Not bound at all, or bound to garbage.
                    1 => {}
                    2 => {
                        marks.insert(ATOM_KEY.to_owned(), Value::from("garbage"));
                    }
                    _ => {
                        marks.insert(
                            ATOM_KEY.to_owned(),
                            Value::Str(encode_binding(kind, entity)),
                        );
                    }
                }
                // Now and then two placeholders share one binding (as after concurrent moves).
                let copies = if rng.chance(1, 15) { 2 } else { 1 };
                kind.placeholder().to_string().repeat(copies)
            }
        };
        // Text with an atom binding is meaningless and must be ignored.
        if rng.chance(1, 40) {
            marks.insert(
                ATOM_KEY.to_owned(),
                Value::from("p:00000000000000000000000000000001"),
            );
        }
        runs.push(Run { text, marks });
    }
    if rng.chance(3, 4) {
        let paragraph = pick(rng, &pools.paragraphs, &pools.missing);
        let mut marks = BTreeMap::new();
        marks.insert(
            ATOM_KEY.to_owned(),
            Value::Str(encode_binding(AtomKind::ParagraphEnd, paragraph)),
        );
        runs.push(Run {
            text: "\u{0D}".to_owned(),
            marks,
        });
    }
    runs
}

fn list(rng: &mut Rng, pool: &[EntityId], missing: &[EntityId]) -> Vec<Value> {
    (0..rng.below(4))
        .map(|_| match rng.below(15) {
            0 => Value::Int(3),
            1 => Value::from("not an id"),
            _ => Value::Str(pick(rng, pool, missing).to_string()),
        })
        .collect()
}

fn owned_story(rng: &mut Rng, pools: &Pools) -> BTreeMap<String, Value> {
    let mut props = props(rng);
    match rng.below(10) {
        0 => {}
        1 => {
            props.insert(
                registry::STORY.to_owned(),
                Value::Str(EntityId::MAIN_STORY.to_string()),
            );
        }
        _ => {
            let story = pick(rng, &pools.stories, &pools.missing);
            props.insert(registry::STORY.to_owned(), Value::Str(story.to_string()));
        }
    }
    props
}

fn random_document(seed: u64) -> RawDocument {
    let mut rng = Rng::new(seed);
    let size = 2 + rng.below(8);
    let pools = Pools {
        stories: pool(&mut rng, size),
        paragraphs: pool(&mut rng, size * 2),
        tables: pool(&mut rng, size / 2 + 1),
        rows: pool(&mut rng, size),
        cells: pool(&mut rng, size * 2),
        objects: pool(&mut rng, size),
        fields: pool(&mut rng, size),
        comments: pool(&mut rng, size),
        ranges: pool(&mut rng, size),
        missing: pool(&mut rng, 3),
    };
    let mut raw = RawDocument {
        main: story(&mut rng, &pools, 30),
        ..RawDocument::default()
    };
    for story_id in &pools.stories {
        raw.stories.insert(*story_id, story(&mut rng, &pools, 12));
    }
    for paragraph in &pools.paragraphs {
        raw.paragraphs.insert(*paragraph, props(&mut rng));
    }
    for table in &pools.tables {
        let rows = list(&mut rng, &pools.rows, &pools.missing);
        raw.tables.insert(
            *table,
            RawTable {
                props: props(&mut rng),
                rows,
            },
        );
    }
    for row in &pools.rows {
        let cells = list(&mut rng, &pools.cells, &pools.missing);
        raw.rows.insert(
            *row,
            RawRow {
                props: props(&mut rng),
                cells,
            },
        );
    }
    for cell in &pools.cells {
        let props = owned_story(&mut rng, &pools);
        raw.cells.insert(*cell, props);
    }
    for comment in &pools.comments {
        let props = owned_story(&mut rng, &pools);
        raw.comments.insert(*comment, props);
    }
    for (registry, pool) in [
        (&mut raw.objects, &pools.objects),
        (&mut raw.fields, &pools.fields),
        (&mut raw.ranges, &pools.ranges),
    ] {
        for entity in pool {
            registry.insert(*entity, props(&mut rng));
        }
    }
    if rng.chance(1, 2) {
        raw.body.insert("pgSz.w".to_owned(), Value::Int(12_240));
        if rng.chance(1, 2) {
            raw.body.insert("pgSz.h".to_owned(), Value::from("tall"));
        }
    }
    raw
}

/// The same content with every run of two or more characters split in two, at a random character.
fn split_runs(raw: &RawDocument, seed: u64) -> RawDocument {
    let mut rng = Rng::new(seed);
    let mut split = |runs: &[Run]| -> Vec<Run> {
        let mut result = Vec::new();
        for run in runs {
            let chars: Vec<char> = run.text.chars().collect();
            if chars.len() >= 2 {
                let at = 1 + rng.below(chars.len() - 1);
                result.push(Run {
                    text: chars[..at].iter().collect(),
                    marks: run.marks.clone(),
                });
                result.push(Run {
                    text: chars[at..].iter().collect(),
                    marks: run.marks.clone(),
                });
            } else {
                result.push(run.clone());
            }
        }
        result
    };
    let mut copy = raw.clone();
    copy.main = split(&raw.main);
    for (story, runs) in &raw.stories {
        copy.stories.insert(*story, split(runs));
    }
    copy
}

fn repairs(report: &Report) -> usize {
    report.dropped_characters
        + report.n1
        + report.n2
        + report.n3
        + report.n4
        + report.n5
        + report.n6
        + report.n7
        + report.n8
        + report.too_deep
        + report.virtual_stories.len()
}

/// Everything the properties require of one document, or a description of what failed.
fn failure(raw: &RawDocument, seed: u64) -> Option<String> {
    let (view, _) = normalize(raw);
    let violations = check_invariants(&view);
    if !violations.is_empty() {
        return Some(format!("invariants: {violations:?}"));
    }
    if normalize(&raw.clone()).0 != view {
        return Some("normalizing twice gave different views".to_owned());
    }
    if normalize(&split_runs(raw, seed)).0 != view {
        return Some("splitting runs differently changed the view".to_owned());
    }
    let (again, report) = normalize(&view.to_raw());
    if again != view {
        return Some("normalizing the view's raw form changed it".to_owned());
    }
    if repairs(&report) != 0 {
        return Some(format!(
            "normalizing the view's raw form repaired something: {report:?}"
        ));
    }
    for story in view.stories.keys() {
        let text = view.plain_text(*story);
        if text
            .chars()
            .any(|character| character < ' ' && character != '\n' && character != '\t')
        {
            return Some(format!(
                "plain text of {story} contains a control character"
            ));
        }
    }
    None
}

/// Smaller variants of a document: one story, run, entity or list item fewer, or a run's text halved.
fn smaller(raw: &RawDocument) -> Vec<RawDocument> {
    let mut variants = Vec::new();
    let runs_variants = |runs: &Vec<Run>, rebuild: &mut dyn FnMut(Vec<Run>)| {
        for index in 0..runs.len() {
            let mut fewer = runs.clone();
            fewer.remove(index);
            rebuild(fewer);
            let chars: Vec<char> = runs[index].text.chars().collect();
            if chars.len() > 1 {
                let mut halved = runs.clone();
                halved[index].text = chars[..chars.len() / 2].iter().collect();
                rebuild(halved);
            }
        }
    };
    runs_variants(&raw.main, &mut |runs| {
        let mut copy = raw.clone();
        copy.main = runs;
        variants.push(copy);
    });
    for (story, runs) in &raw.stories {
        let mut copy = raw.clone();
        copy.stories.remove(story);
        variants.push(copy);
        runs_variants(runs, &mut |runs| {
            let mut copy = raw.clone();
            copy.stories.insert(*story, runs);
            variants.push(copy);
        });
    }
    macro_rules! remove_each {
        ($field:ident) => {
            for key in raw.$field.keys() {
                let mut copy = raw.clone();
                copy.$field.remove(key);
                variants.push(copy);
            }
        };
    }
    remove_each!(paragraphs);
    remove_each!(tables);
    remove_each!(rows);
    remove_each!(cells);
    remove_each!(objects);
    remove_each!(fields);
    remove_each!(comments);
    remove_each!(ranges);
    for (table, entry) in &raw.tables {
        for index in 0..entry.rows.len() {
            let mut copy = raw.clone();
            if let Some(table) = copy.tables.get_mut(table) {
                table.rows.remove(index);
            }
            variants.push(copy);
        }
    }
    for (row, entry) in &raw.rows {
        for index in 0..entry.cells.len() {
            let mut copy = raw.clone();
            if let Some(row) = copy.rows.get_mut(row) {
                row.cells.remove(index);
            }
            variants.push(copy);
        }
    }
    if !raw.body.is_empty() {
        let mut copy = raw.clone();
        copy.body.clear();
        variants.push(copy);
    }
    variants
}

/// Shrinks a failing document: keeps taking the first smaller variant that still fails, until none does.
fn shrink(mut raw: RawDocument, seed: u64) -> RawDocument {
    'outer: loop {
        for variant in smaller(&raw) {
            if failure(&variant, seed).is_some() {
                raw = variant;
                continue 'outer;
            }
        }
        return raw;
    }
}

/// Whether a story of the view holds a field inside another field's code or result.
fn has_nested_field(view: &View) -> bool {
    view.stories.values().any(|items| {
        let mut open = 0_usize;
        items.iter().any(|item| match item.kind() {
            Some(AtomKind::FieldBegin) => {
                open += 1;
                open > 1
            }
            Some(AtomKind::FieldEnd) => {
                open = open.saturating_sub(1);
                false
            }
            _ => false,
        })
    })
}

#[test]
fn normalization_yields_invariant_views_and_is_deterministic_and_idempotent() {
    let mut repaired = Report::default();
    let cases = cases();
    // Cases whose view keeps a field, a field with a separator, and a field nested in another.
    let (mut with_fields, mut with_separators, mut with_nested) = (0_u64, 0_u64, 0_u64);
    for seed in 0..cases {
        let raw = random_document(seed);
        if let Some(problem) = failure(&raw, seed) {
            let small = shrink(raw, seed);
            panic!(
                "seed {seed}: {problem}\nshrunk to: {small:#?}\nwhich fails with: {:?}",
                failure(&small, seed)
            );
        }
        let (view, report) = normalize(&raw);
        with_fields += u64::from(!view.fields.is_empty());
        with_separators += u64::from(
            view.stories
                .values()
                .flatten()
                .any(|item| item.is(AtomKind::FieldSeparator)),
        );
        with_nested += u64::from(has_nested_field(&view));
        repaired.n1 += report.n1;
        repaired.n2 += report.n2;
        repaired.n3 += report.n3;
        repaired.n4 += report.n4;
        repaired.n5 += report.n5;
        repaired.n6 += report.n6;
        repaired.n7 += report.n7;
        repaired.n8 += report.n8;
        repaired.dropped_characters += report.dropped_characters;
    }
    // The generator really exercises every rule (otherwise the properties would hold vacuously).
    for (rule, count) in [
        ("N1", repaired.n1),
        ("N2", repaired.n2),
        ("N3", repaired.n3),
        ("N4", repaired.n4),
        ("N5", repaired.n5),
        ("N6", repaired.n6),
        ("N7", repaired.n7),
        ("N8", repaired.n8),
        ("dropped characters", repaired.dropped_characters),
    ] {
        assert!(count > 0, "rule {rule} never applied in {cases} cases");
    }
    // I2's nesting is really exercised: a good share of views keep fields, separators and fields nested in a code or a result.
    println!(
        "{cases} cases: {with_fields} keep a field, {with_separators} a separator, {with_nested} a nested field"
    );
    for (what, count) in [
        ("a field", with_fields),
        ("a separator", with_separators),
        ("a nested field", with_nested),
    ] {
        assert!(
            count * 10 >= cases,
            "only {count} of {cases} views keep {what}"
        );
    }
}

/// A table whose cell story contains the table's own atom (as two concurrent moves can produce): the cycle is cut, and the view stays finite and valid.
#[test]
fn a_table_inside_its_own_cell_is_cut() {
    let table = EntityId(10);
    let row = EntityId(11);
    let cell = EntityId(12);
    let story = EntityId(13);
    let paragraph = EntityId(14);
    let cell_paragraph = EntityId(15);
    let atom = |kind: AtomKind, id: EntityId| Run {
        text: kind.placeholder().to_string(),
        marks: BTreeMap::from([(ATOM_KEY.to_owned(), Value::Str(encode_binding(kind, id)))]),
    };
    let mut raw = RawDocument {
        main: vec![
            atom(AtomKind::TableBlock, table),
            atom(AtomKind::ParagraphEnd, paragraph),
        ],
        ..RawDocument::default()
    };
    raw.stories.insert(
        story,
        vec![
            atom(AtomKind::TableBlock, table),
            atom(AtomKind::ParagraphEnd, cell_paragraph),
        ],
    );
    raw.paragraphs.insert(paragraph, BTreeMap::new());
    raw.paragraphs.insert(cell_paragraph, BTreeMap::new());
    raw.tables.insert(
        table,
        RawTable {
            props: BTreeMap::new(),
            rows: vec![Value::Str(row.to_string())],
        },
    );
    raw.rows.insert(
        row,
        RawRow {
            props: BTreeMap::new(),
            cells: vec![Value::Str(cell.to_string())],
        },
    );
    raw.cells.insert(
        cell,
        BTreeMap::from([(registry::STORY.to_owned(), Value::Str(story.to_string()))]),
    );
    let (view, report) = normalize(&raw);
    assert!(
        check_invariants(&view).is_empty(),
        "{:?}",
        check_invariants(&view)
    );
    assert_eq!(report.n7, 1);
    assert_eq!(
        view.stories[&story],
        vec![Item::Atom {
            kind: AtomKind::ParagraphEnd,
            id: Some(cell_paragraph),
            marks: BTreeMap::new()
        }]
    );
}

/// Tables nested deeper than the limit are dropped instead of exhausting the stack.
#[test]
fn deeply_nested_tables_are_dropped_at_the_limit() {
    let mut raw = RawDocument::default();
    let atom = |kind: AtomKind, id: EntityId| Run {
        text: kind.placeholder().to_string(),
        marks: BTreeMap::from([(ATOM_KEY.to_owned(), Value::Str(encode_binding(kind, id)))]),
    };
    let levels: u128 = 10_000;
    for level in 0..levels {
        let table = EntityId(1_000_000 + level);
        let row = EntityId(2_000_000 + level);
        let cell = EntityId(3_000_000 + level);
        let story = EntityId(4_000_000 + level);
        raw.tables.insert(
            table,
            RawTable {
                props: BTreeMap::new(),
                rows: vec![Value::Str(row.to_string())],
            },
        );
        raw.rows.insert(
            row,
            RawRow {
                props: BTreeMap::new(),
                cells: vec![Value::Str(cell.to_string())],
            },
        );
        raw.cells.insert(
            cell,
            BTreeMap::from([(registry::STORY.to_owned(), Value::Str(story.to_string()))]),
        );
        // Each cell holds the next table.
        let next = EntityId(1_000_000 + level + 1);
        raw.stories
            .insert(story, vec![atom(AtomKind::TableBlock, next)]);
    }
    raw.main = vec![atom(AtomKind::TableBlock, EntityId(1_000_000))];
    let (view, report) = normalize(&raw);
    assert!(
        check_invariants(&view).is_empty(),
        "{:?}",
        check_invariants(&view)
    );
    assert_eq!(report.too_deep, 1);
    assert_eq!(view.tables.len(), MAX_TABLE_DEPTH);
}

/// A view is the same type that layout reads; check one small document end to end, including the plain text export.
#[test]
fn a_small_document_normalizes_as_expected() {
    let paragraph = EntityId(1);
    let field = EntityId(2);
    let mut raw = RawDocument::default();
    let atom = |kind: AtomKind, id: EntityId| Run {
        text: kind.placeholder().to_string(),
        marks: BTreeMap::from([(ATOM_KEY.to_owned(), Value::Str(encode_binding(kind, id)))]),
    };
    let text = |text: &str| Run {
        text: text.to_owned(),
        marks: BTreeMap::new(),
    };
    raw.main = vec![
        text("Page "),
        atom(AtomKind::FieldBegin, field),
        text("PAGE"),
        atom(AtomKind::FieldSeparator, field),
        text("7"),
        atom(AtomKind::FieldEnd, field),
        text("\u{01}\tend"),
    ];
    raw.paragraphs.insert(paragraph, BTreeMap::new());
    raw.fields.insert(field, BTreeMap::new());
    let (view, report) = normalize(&raw);
    assert!(
        check_invariants(&view).is_empty(),
        "{:?}",
        check_invariants(&view)
    );
    // The stray control character is dropped, the tab is an atom, and N1 adds the final paragraph end.
    assert_eq!(report.dropped_characters, 1);
    assert_eq!(report.n1, 1);
    assert_eq!(view.plain_text(EntityId::MAIN_STORY), "Page 7\tend\n");
    // Text, begin, code, separator, result, end, tab, text, paragraph end.
    assert_eq!(view.main().len(), 9);
    let as_view: &View = &view;
    assert_eq!(as_view.fields.len(), 1);
}

fn atom(kind: AtomKind, id: EntityId) -> Run {
    Run {
        text: kind.placeholder().to_string(),
        marks: BTreeMap::from([(ATOM_KEY.to_owned(), Value::Str(encode_binding(kind, id)))]),
    }
}

fn text(text: &str) -> Run {
    Run {
        text: text.to_owned(),
        marks: BTreeMap::new(),
    }
}

/// Adds table `table` with one row and one cell whose story is `story` holding `runs`.
fn add_table(raw: &mut RawDocument, table: EntityId, story: EntityId, runs: Vec<Run>) {
    let row = EntityId(table.0 + 1);
    let cell = EntityId(table.0 + 2);
    raw.tables.insert(
        table,
        RawTable {
            props: BTreeMap::new(),
            rows: vec![Value::Str(row.to_string())],
        },
    );
    raw.rows.insert(
        row,
        RawRow {
            props: BTreeMap::new(),
            cells: vec![Value::Str(cell.to_string())],
        },
    );
    raw.cells.insert(
        cell,
        BTreeMap::from([(registry::STORY.to_owned(), Value::Str(story.to_string()))]),
    );
    raw.stories.insert(story, runs);
}

/// The identifier N4 derives for the paragraph end before table `table`, found by normalizing a document where that table stands mid-paragraph in the main story.
fn n4_identifier(table: EntityId) -> EntityId {
    let mut raw = RawDocument::default();
    let paragraph = EntityId(0xA);
    raw.paragraphs.insert(paragraph, BTreeMap::new());
    raw.main = vec![
        text("x"),
        atom(AtomKind::TableBlock, table),
        atom(AtomKind::ParagraphEnd, paragraph),
    ];
    add_table(&mut raw, table, EntityId(0x5000), vec![]);
    let (_, report) = normalize(&raw);
    let split: Vec<EntityId> = report
        .virtual_paragraph_ends
        .iter()
        .filter(|end| end.rule == bayan_model::Rule::N4)
        .map(|end| end.id)
        .collect();
    assert_eq!(split.len(), 1);
    split[0]
}

/// A stored paragraph end whose identifier is the one N4 would derive for a table (because an earlier materialization stored it) keeps its identifier when that table stands mid-paragraph again inside a cell that is normalized before the stored end is reached (review of CORE-004, item 14).
#[test]
fn a_derived_paragraph_end_never_takes_over_a_stored_one() {
    let outer = EntityId(0x1000);
    let inner = EntityId(0x2000);
    let derived = n4_identifier(inner);
    let [a, b, c] = [EntityId(0xA1), EntityId(0xB1), EntityId(0xC1)];
    let mut raw = RawDocument::default();
    for paragraph in [a, b, c] {
        raw.paragraphs.insert(paragraph, BTreeMap::new());
    }
    raw.paragraphs.insert(
        derived,
        BTreeMap::from([("style".to_owned(), Value::from("Heading1"))]),
    );
    // Main story: [outer] ¶a "x" ¶derived "z" ¶b
    raw.main = vec![
        atom(AtomKind::TableBlock, outer),
        atom(AtomKind::ParagraphEnd, a),
        text("x"),
        atom(AtomKind::ParagraphEnd, derived),
        text("z"),
        atom(AtomKind::ParagraphEnd, b),
    ];
    // The outer table's cell: "y" [inner] ¶c, with the inner table mid-paragraph.
    add_table(
        &mut raw,
        outer,
        EntityId(0x6000),
        vec![
            text("y"),
            atom(AtomKind::TableBlock, inner),
            atom(AtomKind::ParagraphEnd, c),
        ],
    );
    add_table(&mut raw, inner, EntityId(0x7000), vec![]);
    let (view, report) = normalize(&raw);
    assert!(
        check_invariants(&view).is_empty(),
        "{:?}",
        check_invariants(&view)
    );
    assert_eq!(report.n7, 0);
    assert_eq!(view.plain_text(EntityId::MAIN_STORY), "\nx\nz\n");
    assert_eq!(
        view.paragraphs[&derived].get("style"),
        Some(&Value::from("Heading1"))
    );
    // The cell's split got an identifier of its own, with the properties of the paragraph it splits.
    let cell = &view.stories[&EntityId(0x6000)];
    let Item::Atom {
        id: Some(split), ..
    } = &cell[1]
    else {
        panic!("the cell does not start with text and a paragraph end: {cell:?}");
    };
    assert_ne!(*split, derived);
    assert_eq!(view.paragraphs[split], BTreeMap::new());
}

/// `tables` tables in the middle of one paragraph.
fn tables_in_one_paragraph(tables: u128) -> RawDocument {
    let mut raw = RawDocument::default();
    let paragraph = EntityId(0xA);
    raw.paragraphs.insert(
        paragraph,
        BTreeMap::from([("style".to_owned(), Value::from("Quote"))]),
    );
    for index in 0..tables {
        let table = EntityId(0x1_0000 + index * 4);
        raw.main.push(text("x"));
        raw.main.push(atom(AtomKind::TableBlock, table));
        add_table(&mut raw, table, EntityId(0x1_0000 + index * 4 + 3), vec![]);
    }
    raw.main.push(atom(AtomKind::ParagraphEnd, paragraph));
    raw
}

/// The shortest of five runs of `work`: background load can only lengthen a run.
fn shortest(mut work: impl FnMut()) -> Duration {
    (0..5)
        .map(|_| {
            let start = Instant::now();
            work();
            start.elapsed()
        })
        .min()
        .unwrap_or_default()
}

/// Fails unless `work` grows linearly: eight times the input may take at most 24 times as long, plus 50 ms for timer and load noise. Linear work takes about 8 times as long; the quadratic versions these tests guard against took about 64 times as long, which fails.
fn assert_linear(name: &str, small: impl FnMut(), large: impl FnMut()) {
    let small = shortest(small);
    let large = shortest(large);
    assert!(
        large <= small * 24 + Duration::from_millis(50),
        "{name}: eight times the input took {large:?} instead of at most 24 times {small:?}"
    );
}

/// Many tables in the middle of one paragraph: N4 splits before each in linear time (with the earlier quadratic search, 40,000 tables took about 4 s in a release build), and the view is valid (review of CORE-004, item 15). The time is checked too, so that a quadratic search fails the test (second round, item A4).
#[test]
fn many_tables_in_one_paragraph_are_split_in_linear_time() {
    let small = tables_in_one_paragraph(2_500);
    let raw = tables_in_one_paragraph(20_000);
    let (view, report) = normalize(&raw);
    assert_eq!(report.n4, 20_000);
    assert!(check_invariants(&view).is_empty());
    // Every split carries the properties of the paragraph it splits.
    assert!(
        report
            .virtual_paragraph_ends
            .iter()
            .filter(|end| end.rule == bayan_model::Rule::N4)
            .all(|end| end.props.get("style") == Some(&Value::from("Quote")))
    );
    assert_linear("N4", || drop(normalize(&small)), || drop(normalize(&raw)));
}

/// N7 keeps the first of two atoms that reference the same entity in one story, not the last (review of CORE-004, item 21).
#[test]
fn a_repeated_reference_keeps_its_first_place() {
    let paragraph = EntityId(0xA);
    let mut raw = RawDocument::default();
    raw.paragraphs.insert(paragraph, BTreeMap::new());
    raw.main = vec![
        text("x"),
        atom(AtomKind::ParagraphEnd, paragraph),
        text("y"),
        atom(AtomKind::ParagraphEnd, paragraph),
    ];
    let (view, report) = normalize(&raw);
    assert_eq!(report.n7, 1);
    assert_eq!(
        view.main()[1],
        Item::Atom {
            kind: AtomKind::ParagraphEnd,
            id: Some(paragraph),
            marks: BTreeMap::new()
        }
    );
    assert_eq!(view.plain_text(EntityId::MAIN_STORY), "x\ny\n");
}

/// Which atoms keep their marks in the view, written out independently of `AtomKind::keeps_marks` (review of CORE-004, item 21): those with a glyph or an anchor keep them; paragraph ends (their formatting lives in the paragraph's properties), range delimiters and tables do not.
#[test]
fn atoms_keep_their_marks_exactly_when_they_have_a_glyph() {
    let expected = [
        (AtomKind::ParagraphEnd, false),
        (AtomKind::Tab, true),
        (AtomKind::FieldBegin, true),
        (AtomKind::FieldSeparator, true),
        (AtomKind::FieldEnd, true),
        (AtomKind::ObjectAnchor, true),
        (AtomKind::RangeStart, false),
        (AtomKind::RangeEnd, false),
        (AtomKind::TableBlock, false),
        (AtomKind::CommentReference, true),
    ];
    assert_eq!(expected.len(), AtomKind::ALL.len());
    let bold = || BTreeMap::from([("r:b".to_owned(), Value::Bool(true))]);
    for (kind, keeps) in expected {
        let entity = EntityId(0x10);
        let mut raw = RawDocument::default();
        let paragraph = EntityId(0xA);
        raw.paragraphs.insert(paragraph, BTreeMap::new());
        let mut marked = atom(kind, entity);
        marked.marks.extend(bold());
        // A complete field or range around the atom where its kind needs one, so that N2 and N3 keep it.
        raw.main = match kind {
            AtomKind::FieldBegin | AtomKind::FieldSeparator | AtomKind::FieldEnd => vec![
                if kind == AtomKind::FieldBegin {
                    marked.clone()
                } else {
                    atom(AtomKind::FieldBegin, entity)
                },
                if kind == AtomKind::FieldSeparator {
                    marked.clone()
                } else {
                    atom(AtomKind::FieldSeparator, entity)
                },
                if kind == AtomKind::FieldEnd {
                    marked.clone()
                } else {
                    atom(AtomKind::FieldEnd, entity)
                },
            ],
            AtomKind::RangeStart => vec![marked, atom(AtomKind::RangeEnd, entity)],
            AtomKind::RangeEnd => vec![atom(AtomKind::RangeStart, entity), marked],
            _ => vec![marked],
        };
        raw.main.push(atom(AtomKind::ParagraphEnd, paragraph));
        match kind {
            AtomKind::ParagraphEnd => {
                raw.main = vec![{
                    let mut end = atom(AtomKind::ParagraphEnd, paragraph);
                    end.marks.extend(bold());
                    end
                }];
            }
            AtomKind::FieldBegin | AtomKind::FieldSeparator | AtomKind::FieldEnd => {
                raw.fields.insert(entity, BTreeMap::new());
            }
            AtomKind::ObjectAnchor => {
                raw.objects.insert(entity, BTreeMap::new());
            }
            AtomKind::RangeStart | AtomKind::RangeEnd => {
                raw.ranges.insert(entity, BTreeMap::new());
            }
            AtomKind::TableBlock => add_table(&mut raw, entity, EntityId(0x20), vec![]),
            AtomKind::CommentReference => {
                raw.comments.insert(
                    entity,
                    BTreeMap::from([(
                        registry::STORY.to_owned(),
                        Value::Str(EntityId(0x30).to_string()),
                    )]),
                );
                raw.stories.insert(EntityId(0x30), vec![]);
            }
            AtomKind::Tab => {}
        }
        let (view, _) = normalize(&raw);
        assert!(
            check_invariants(&view).is_empty(),
            "{kind:?}: {:?}",
            check_invariants(&view)
        );
        let found = view
            .main()
            .iter()
            .find(|item| item.is(kind))
            .unwrap_or_else(|| panic!("{kind:?} left the view"));
        let Item::Atom { marks, .. } = found else {
            unreachable!("an atom")
        };
        assert_eq!(marks == &bold(), keeps, "{kind:?}");
        assert_eq!(marks.is_empty(), !keeps, "{kind:?}");
    }
}

/// A field nested in another field's code is part of that code: its result is hidden too (review of CORE-004, item 21).
#[test]
fn the_plain_text_hides_fields_nested_in_a_field_code() {
    let [outer, inner] = [EntityId(0xF1), EntityId(0xF2)];
    let paragraph = EntityId(0xA);
    let mut raw = RawDocument::default();
    raw.paragraphs.insert(paragraph, BTreeMap::new());
    raw.fields.insert(outer, BTreeMap::new());
    raw.fields.insert(inner, BTreeMap::new());
    // ⟦outer: IF ⟦inner: PAGE | 7⟧ = 1 | yes⟧
    raw.main = vec![
        text("<"),
        atom(AtomKind::FieldBegin, outer),
        text("IF "),
        atom(AtomKind::FieldBegin, inner),
        text("PAGE"),
        atom(AtomKind::FieldSeparator, inner),
        text("7"),
        atom(AtomKind::FieldEnd, inner),
        text(" = 1"),
        atom(AtomKind::FieldSeparator, outer),
        text("yes"),
        atom(AtomKind::FieldEnd, outer),
        text(">"),
        atom(AtomKind::ParagraphEnd, paragraph),
    ];
    let (view, report) = normalize(&raw);
    assert_eq!(report.n2, 0);
    assert!(check_invariants(&view).is_empty());
    assert_eq!(view.plain_text(EntityId::MAIN_STORY), "<yes>\n");
}

/// `depth` fields nested in each other's codes.
fn nested_field_codes(depth: u128) -> RawDocument {
    let paragraph = EntityId(0xA);
    let mut raw = RawDocument::default();
    raw.paragraphs.insert(paragraph, BTreeMap::new());
    for level in 0..depth {
        let field = EntityId(0x1_0000 + level);
        raw.fields.insert(field, BTreeMap::new());
        raw.main.push(atom(AtomKind::FieldBegin, field));
        raw.main.push(text("c"));
    }
    raw.main.push(text("hidden"));
    for level in (0..depth).rev() {
        let field = EntityId(0x1_0000 + level);
        raw.main.push(atom(AtomKind::FieldSeparator, field));
        raw.main.push(text("r"));
        raw.main.push(atom(AtomKind::FieldEnd, field));
    }
    raw.main.push(atom(AtomKind::ParagraphEnd, paragraph));
    raw
}

/// Deeply nested field codes cost linear time in the plain text export too (the earlier version looked at every open field for every item); the time is checked, so that a quadratic export fails the test (review of CORE-004, second round, item A4).
#[test]
fn deeply_nested_field_codes_export_in_linear_time() {
    let (small, _) = normalize(&nested_field_codes(2_500));
    let (view, _) = normalize(&nested_field_codes(20_000));
    // Only the outermost field's result is outside every code.
    assert_eq!(view.plain_text(EntityId::MAIN_STORY), "r\n");
    assert_linear(
        "plain text",
        || drop(small.plain_text(EntityId::MAIN_STORY)),
        || drop(view.plain_text(EntityId::MAIN_STORY)),
    );
}

/// A cell whose story is missing gets an empty stand-in story with a derived identifier, and that identifier is never one that a stored story already has: here an orphan story that nothing owns holds "secret", which the cell would otherwise show (review of CORE-004, second round, item A4).
#[test]
fn a_stand_in_story_never_takes_the_identifier_of_a_stored_story() {
    let table = EntityId(0x1000);
    let cell = EntityId(table.0 + 2);
    let build = |orphan: Option<EntityId>| {
        let mut raw = RawDocument::default();
        let paragraph = EntityId(0xA);
        raw.paragraphs.insert(paragraph, BTreeMap::new());
        raw.main = vec![
            atom(AtomKind::TableBlock, table),
            atom(AtomKind::ParagraphEnd, paragraph),
        ];
        add_table(&mut raw, table, EntityId(0x5000), vec![]);
        raw.stories.remove(&EntityId(0x5000));
        if let Some(orphan) = orphan {
            let end = EntityId(0xB);
            raw.paragraphs.insert(end, BTreeMap::new());
            raw.stories.insert(
                orphan,
                vec![text("secret"), atom(AtomKind::ParagraphEnd, end)],
            );
        }
        raw
    };
    // The identifier the stand-in gets when no stored story is in its way.
    let (view, report) = normalize(&build(None));
    let derived = view.cells[&cell].story;
    assert_eq!(report.virtual_stories, vec![(cell, derived)]);
    // A stored story with that identifier is in the way: the stand-in takes another, and the cell stays empty.
    let (view, report) = normalize(&build(Some(derived)));
    assert!(
        check_invariants(&view).is_empty(),
        "{:?}",
        check_invariants(&view)
    );
    let stand_in = view.cells[&cell].story;
    assert_ne!(stand_in, derived);
    assert_eq!(report.virtual_stories, vec![(cell, stand_in)]);
    assert_eq!(view.plain_text(stand_in), "\n");
    assert!(!view.stories.contains_key(&derived));
}
