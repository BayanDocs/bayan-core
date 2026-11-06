//! The alignment of two text streams: which characters of Word's text are the same characters in BayanDocs' text, so that the breaks of both can be compared where the texts differ a little (Fidelity Lab specification §6, step 1).
//!
//! The streams are the comparable form of each side's lines ([`crate::stream`]), so a difference that the page does not show (a space drawn or left as a gap, a ligature's spelling) is already gone. What remains are real differences of the text: a numbering label or a field result written differently, text that one side has and the other lacks, or text in another order. The alignment finds the longest run of common characters it can, in order, and calls everything else a difference.
//!
//! ## How
//!
//! [`align`] works like `diff`, on characters, in three steps that keep it fast on long documents and bounded on hostile ones:
//!
//! 1. **Common ends.** The characters both streams start and end with are aligned at once.
//! 2. **Anchors.** In what remains, every run of [`ANCHOR_LENGTH`] characters that occurs exactly once in each stream is an anchor, as in patience diff. The longest chain of anchors that is in the same order in both streams is aligned, and the gaps between them are aligned in turn, with anchors unique within each gap, up to [`MAX_DEPTH`] levels deep.
//! 3. **Myers' algorithm.** A gap without anchors is aligned with the shortest edit script (Myers 1986), as long as it needs at most [`MAX_EDIT_DISTANCE`] insertions and deletions; a gap that needs more is one difference.
//!
//! The work is counted ([`Limits::max_work`]): when it runs out, what is not aligned yet becomes one difference and the alignment says it is incomplete ([`Alignment::complete`]). The same streams always give the same alignment on every platform: integers only, and every choice made in a fixed order.
//!
//! ## Breaks through the alignment
//!
//! A break is a position between two characters, such as where a line starts. [`Alignment::images`] says where a break of Word's text lies in BayanDocs' text:
//!
//! - inside or at the end of aligned text, at exactly one position;
//! - where BayanDocs' text has something Word's lacks (a numbering label, say), anywhere from before to after that insertion;
//! - inside a short difference of both texts (at most [`TOLERATED_DIFFERENCE`] characters on each side, such as a date that differs), anywhere strictly inside the other side's difference;
//! - inside a longer difference, or inside text that BayanDocs lacks, nowhere: such a break cannot agree.
//!
//! [`match_breaks`] then pairs each break of Word's text with a break of BayanDocs' text at a position its image allows, each used once. Breaks followed by aligned text are paired first, and breaks followed by text that differs or that BayanDocs lacks get what remains: where BayanDocs lacks a whole line, the start of the missing line and the start of the next line fall on the same position of BayanDocs' text, and the next line, which both texts have, is the one that agrees there.

use core::ops::Range;

/// The length of an anchor, in characters of the comparable form: long enough that a run which occurs once in each stream is almost always the same text, short enough that most lines hold one.
pub const ANCHOR_LENGTH: usize = 32;

/// How many levels of anchors the alignment nests at most, each within a gap of the level above.
pub const MAX_DEPTH: usize = 8;

/// The most insertions and deletions that Myers' algorithm looks for in one gap between anchors. A gap that needs more is one difference: its texts have too little in common to say which characters correspond.
pub const MAX_EDIT_DISTANCE: usize = 2048;

/// The longest difference, in characters of the comparable form on either side, inside which a break agrees with a break of the other side's difference (a field result or a numbering label written differently).
pub const TOLERATED_DIFFERENCE: usize = 32;

/// The limits of an alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The most steps of work an alignment may take: one for each character compared or hashed, each diagonal Myers' algorithm visits, and each comparison while anchors are sorted.
    pub max_work: u64,
}

impl Limits {
    /// The default: four billion steps, which aligns two different renderings of a document of a few million characters in seconds and stops crafted ones in about a minute.
    pub const DEFAULT: Self = Self {
        max_work: 4_000_000_000,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A piece of an alignment: characters of both streams that are the same (`equal`), or that differ. In a difference, either side may be empty: text that only one stream has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// The characters of Word's stream (the reference).
    pub reference: Range<usize>,
    /// The characters of BayanDocs' stream (the candidate).
    pub candidate: Range<usize>,
    /// Whether the characters are the same, one by one.
    pub equal: bool,
}

impl Hunk {
    /// The position in the candidate's stream of the position `position` of the reference's stream, inside or at an end of an equal hunk.
    const fn equal_partner(&self, position: usize) -> usize {
        self.candidate.start + (position - self.reference.start)
    }
}

/// Where a break of the reference's text may lie in the candidate's text: every position from `low` to `high`, both included; none when `low > high`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Image {
    /// The first position.
    pub low: usize,
    /// The last position.
    pub high: usize,
    /// Whether the text after the break is aligned (or the break ends the stream): such a break is paired before the others.
    pub strong: bool,
}

impl Image {
    /// No position.
    pub const NONE: Self = Self {
        low: 1,
        high: 0,
        strong: false,
    };

    /// The positions from `low` to `high`, for a break followed by aligned text.
    #[must_use]
    pub const fn strong(low: usize, high: usize) -> Self {
        Self {
            low,
            high,
            strong: true,
        }
    }

    /// The positions from `low` to `high`, for a break followed by text that differs.
    #[must_use]
    pub const fn weak(low: usize, high: usize) -> Self {
        Self {
            low,
            high,
            strong: false,
        }
    }

    /// Whether the image holds no position.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.low > self.high
    }

    /// Whether the image holds `position`.
    #[must_use]
    pub const fn contains(self, position: usize) -> bool {
        self.low <= position && position <= self.high
    }

    fn include(&mut self, position: usize) {
        if self.is_empty() {
            self.low = position;
            self.high = position;
        } else {
            self.low = self.low.min(position);
            self.high = self.high.max(position);
        }
    }
}

/// The alignment of two streams: hunks that cover both, in order, alternately equal and different.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alignment {
    hunks: Vec<Hunk>,
    reference_len: usize,
    candidate_len: usize,
    complete: bool,
}

impl Alignment {
    /// The hunks, in order: they cover both streams from start to end, and no two neighbours are both equal or both different.
    #[must_use]
    pub fn hunks(&self) -> &[Hunk] {
        &self.hunks
    }

    /// The number of characters of the reference's stream that are aligned with the same characters of the candidate's.
    #[must_use]
    pub fn matched(&self) -> usize {
        self.hunks
            .iter()
            .filter(|hunk| hunk.equal)
            .map(|hunk| hunk.reference.len())
            .sum()
    }

    /// Whether the alignment ran to its end: `false` when [`Limits::max_work`] stopped it, so that the text it had not reached yet counts as one difference.
    #[must_use]
    pub const fn complete(&self) -> bool {
        self.complete
    }

    /// The image of each break of the reference's stream, given as positions in ascending order, from 0 to the length of the stream (see the module's documentation).
    ///
    /// # Panics
    ///
    /// Never: positions beyond the stream have no image.
    #[must_use]
    pub fn images(&self, breaks: &[usize]) -> Vec<Image> {
        let mut images = Vec::with_capacity(breaks.len());
        let mut first = 0;
        for &position in breaks {
            while self
                .hunks
                .get(first)
                .is_some_and(|hunk| hunk.reference.end < position)
            {
                first += 1;
            }
            let mut image = Image::NONE;
            if position <= self.reference_len {
                // Strong when the reference's character after the break is in an equal hunk, or there is none.
                image.strong = position == self.reference_len;
                for hunk in self.hunks[first.min(self.hunks.len())..]
                    .iter()
                    .take_while(|hunk| hunk.reference.start <= position)
                {
                    image_in_hunk(hunk, position, &mut image);
                    if hunk.equal && hunk.reference.contains(&position) {
                        image.strong = true;
                    }
                }
                if self.hunks.is_empty() && position == 0 {
                    image.include(0);
                }
            }
            if image.is_empty() {
                image = Image::NONE;
            }
            images.push(image);
        }
        images
    }

    /// Moves each deletion (text that only the reference has) between two equal hunks, as far as the text allows, so that as few of `breaks` (positions of the reference's stream, ascending) as possible fall strictly inside it, and preferably one falls at its end.
    ///
    /// A deletion can often lie in several places: deleting `two` from `onetwothree` leaves `onethree`, and so does deleting `wot`. `diff` takes the first characters it can match, which pushes deletions to the right, so that where a whole line is missing, the start of the next line would fall inside the deletion and have no image. Moved to where a line starts, the deletion covers exactly the missing text. A deletion moves by one character when the character it would move past equals the one it would leave at its other end, and never empties its neighbours.
    pub fn slide_deletions(&mut self, reference: &[char], breaks: &[usize]) {
        for index in 1..self.hunks.len().saturating_sub(1) {
            let (previous, hunk, next) = (
                &self.hunks[index - 1],
                &self.hunks[index],
                &self.hunks[index + 1],
            );
            if hunk.equal || !hunk.candidate.is_empty() || !previous.equal || !next.equal {
                continue;
            }
            let (start, end) = (hunk.reference.start, hunk.reference.end);
            let mut left = 0;
            while left + 1 < previous.reference.len()
                && reference[start - left - 1] == reference[end - left - 1]
            {
                left += 1;
            }
            let mut right = 0;
            while right + 1 < next.reference.len()
                && reference[start + right] == reference[end + right]
            {
                right += 1;
            }
            if left == 0 && right == 0 {
                continue;
            }
            // The breaks strictly inside, and whether one is at the end, for the deletion moved to `from`.
            let score = |from: usize| {
                let to = from + (end - start);
                let first = breaks.partition_point(|&position| position <= from);
                let last = breaks.partition_point(|&position| position < to);
                let inside = last.saturating_sub(first);
                let at_end = breaks.binary_search(&to).is_ok();
                (core::cmp::Reverse(inside), at_end)
            };
            // The best position; of equally good ones, the nearest, and to the left on a tie.
            let mut best = start;
            let mut best_score = score(start);
            for distance in 1..=left.max(right) {
                for from in [
                    start.checked_sub(distance).filter(|_| distance <= left),
                    (distance <= right).then_some(start + distance),
                ]
                .into_iter()
                .flatten()
                {
                    let candidate_score = score(from);
                    if candidate_score > best_score {
                        best = from;
                        best_score = candidate_score;
                    }
                }
            }
            if best != start {
                let length = end - start;
                // The candidate's side of a deletion is empty: it moves with the deletion.
                let at = self.hunks[index].candidate.start;
                let candidate_at = if best < start {
                    at - (start - best)
                } else {
                    at + (best - start)
                };
                self.hunks[index - 1].reference.end = best;
                self.hunks[index - 1].candidate.end = candidate_at;
                self.hunks[index].reference = best..best + length;
                self.hunks[index].candidate = candidate_at..candidate_at;
                self.hunks[index + 1].reference.start = best + length;
                self.hunks[index + 1].candidate.start = candidate_at;
            }
        }
    }

    /// For each position of the reference's stream from `range`, the position of the same character in the candidate's stream, where the alignment pairs them.
    pub fn partners(&self, range: Range<usize>) -> impl Iterator<Item = (usize, usize)> + '_ {
        let first = self
            .hunks
            .partition_point(|hunk| hunk.reference.end <= range.start);
        self.hunks[first..]
            .iter()
            .take_while(move |hunk| hunk.reference.start < range.end)
            .filter(|hunk| hunk.equal)
            .flat_map(move |hunk| {
                let start = hunk.reference.start.max(range.start);
                let end = hunk.reference.end.min(range.end);
                (start..end).map(move |position| (position, hunk.equal_partner(position)))
            })
    }
}

/// Adds to `image` where `position`, a break of the reference's stream that `hunk` reaches, lies in the candidate's stream.
fn image_in_hunk(hunk: &Hunk, position: usize, image: &mut Image) {
    let reference = &hunk.reference;
    let candidate = &hunk.candidate;
    if hunk.equal {
        image.include(hunk.equal_partner(position));
    } else if position == reference.start && position == reference.end {
        // Text that only the candidate has: the break may lie before it, after it, or inside it.
        image.include(candidate.start);
        image.include(candidate.end);
    } else if position == reference.start {
        image.include(candidate.start);
    } else if position == reference.end {
        image.include(candidate.end);
    } else if reference.len() <= TOLERATED_DIFFERENCE
        && candidate.len() <= TOLERATED_DIFFERENCE
        && candidate.len() >= 2
    {
        // Inside a short difference of both texts: anywhere strictly inside the candidate's side.
        image.include(candidate.start + 1);
        image.include(candidate.end - 1);
    }
}

/// Pairs breaks of the reference's stream with breaks of the candidate's stream, both in ascending order. Returns, for each reference break, the index of its candidate break.
///
/// Strong breaks (followed by aligned text) are paired first, each with the first unused candidate break that its image allows; then weak ones, from the candidate breaks left. The images of ascending breaks ascend too, both their first and their last positions, so within each pass taking the first allowed candidate break pairs as many breaks as can be paired.
#[must_use]
pub fn match_breaks(images: &[Image], candidate: &[usize]) -> Vec<Option<usize>> {
    let mut matched = vec![None; images.len()];
    let mut used = vec![false; candidate.len()];
    for strong in [true, false] {
        let mut next = 0;
        for (index, image) in images.iter().enumerate() {
            if image.strong != strong || image.is_empty() {
                continue;
            }
            // Candidate breaks before this image are before every later one too, and used ones stay used.
            while next < candidate.len() && (candidate[next] < image.low || used[next]) {
                next += 1;
            }
            if next < candidate.len() && candidate[next] <= image.high {
                used[next] = true;
                matched[index] = Some(next);
                next += 1;
            }
        }
    }
    matched
}

/// Aligns `reference` (Word's stream) with `candidate` (BayanDocs' stream) within `limits`.
#[must_use]
pub fn align(reference: &[char], candidate: &[char], limits: &Limits) -> Alignment {
    let mut aligner = Aligner {
        a: reference,
        b: candidate,
        hunks: Vec::new(),
        work: 0,
        max_work: limits.max_work,
        complete: true,
    };
    aligner.range(0..reference.len(), 0..candidate.len(), 0);
    let hunks = merge(aligner.hunks);
    Alignment {
        hunks,
        reference_len: reference.len(),
        candidate_len: candidate.len(),
        complete: aligner.complete,
    }
}

/// Joins neighbouring hunks of the same kind.
fn merge(hunks: Vec<Hunk>) -> Vec<Hunk> {
    let mut merged: Vec<Hunk> = Vec::with_capacity(hunks.len());
    for hunk in hunks {
        match merged.last_mut() {
            Some(last) if last.equal == hunk.equal => {
                last.reference.end = hunk.reference.end;
                last.candidate.end = hunk.candidate.end;
            }
            _ => merged.push(hunk),
        }
    }
    merged
}

/// An aligned run that anchors found: `len` characters from `a` and from `b`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Block {
    a: usize,
    b: usize,
    len: usize,
}

/// The state of one alignment.
struct Aligner<'s> {
    a: &'s [char],
    b: &'s [char],
    hunks: Vec<Hunk>,
    work: u64,
    max_work: u64,
    complete: bool,
}

impl Aligner<'_> {
    fn spend(&mut self, steps: usize) {
        self.work = self
            .work
            .saturating_add(u64::try_from(steps).unwrap_or(u64::MAX));
    }

    const fn exhausted(&self) -> bool {
        self.work > self.max_work
    }

    fn equal(&mut self, a: usize, b: usize, len: usize) {
        if len > 0 {
            self.hunks.push(Hunk {
                reference: a..a + len,
                candidate: b..b + len,
                equal: true,
            });
        }
    }

    fn differ(&mut self, a: Range<usize>, b: Range<usize>) {
        if !a.is_empty() || !b.is_empty() {
            self.hunks.push(Hunk {
                reference: a,
                candidate: b,
                equal: false,
            });
        }
    }

    /// Aligns `a` of the reference with `b` of the candidate.
    fn range(&mut self, mut a: Range<usize>, mut b: Range<usize>, depth: usize) {
        let prefix = self.a[a.clone()]
            .iter()
            .zip(&self.b[b.clone()])
            .take_while(|(x, y)| x == y)
            .count();
        self.spend(prefix + 1);
        self.equal(a.start, b.start, prefix);
        a.start += prefix;
        b.start += prefix;
        let suffix = self.a[a.clone()]
            .iter()
            .rev()
            .zip(self.b[b.clone()].iter().rev())
            .take_while(|(x, y)| x == y)
            .count();
        self.spend(suffix + 1);
        a.end -= suffix;
        b.end -= suffix;
        let suffix_start = (a.end, b.end);
        if a.is_empty() || b.is_empty() {
            self.differ(a, b);
        } else if self.exhausted() {
            self.complete = false;
            self.differ(a, b);
        } else {
            let blocks = if depth < MAX_DEPTH
                && a.len() >= 2 * ANCHOR_LENGTH
                && b.len() >= 2 * ANCHOR_LENGTH
            {
                self.anchors(a.clone(), b.clone())
            } else {
                Vec::new()
            };
            if blocks.is_empty() {
                self.myers(a, b);
            } else {
                let (mut x, mut y) = (a.start, b.start);
                for block in blocks {
                    self.range(x..block.a, y..block.b, depth + 1);
                    self.equal(block.a, block.b, block.len);
                    x = block.a + block.len;
                    y = block.b + block.len;
                }
                self.range(x..a.end, y..b.end, depth + 1);
            }
        }
        self.equal(suffix_start.0, suffix_start.1, suffix);
    }

    /// The anchors of `a` and `b`: runs of [`ANCHOR_LENGTH`] characters that occur once in each, the longest chain of them in the same order in both, joined into blocks.
    fn anchors(&mut self, a: Range<usize>, b: Range<usize>) -> Vec<Block> {
        self.spend(sort_cost(a.len()) + sort_cost(b.len()) + a.len() + b.len());
        let grams_a = unique_grams(self.a, a);
        let grams_b = unique_grams(self.b, b);
        // Runs that occur once in each, as (position in a, position in b).
        let mut pairs = Vec::new();
        let (mut i, mut j) = (0, 0);
        while let (Some(&(hash_a, at_a)), Some(&(hash_b, at_b))) = (grams_a.get(i), grams_b.get(j))
        {
            self.spend(1);
            match hash_a.cmp(&hash_b) {
                core::cmp::Ordering::Less => i += 1,
                core::cmp::Ordering::Greater => j += 1,
                core::cmp::Ordering::Equal => {
                    // Two different runs can share a hash: compare the runs themselves.
                    self.spend(ANCHOR_LENGTH);
                    if self.a[at_a..at_a + ANCHOR_LENGTH] == self.b[at_b..at_b + ANCHOR_LENGTH] {
                        pairs.push((at_a, at_b));
                    }
                    i += 1;
                    j += 1;
                }
            }
        }
        pairs.sort_unstable();
        self.spend(2 * sort_cost(pairs.len()));
        blocks(&longest_increasing(&pairs))
    }

    /// Myers' shortest edit script for `a` and `b`, both not empty: the path with the fewest insertions and deletions, found diagonal by diagonal, at most [`MAX_EDIT_DISTANCE`] of them. One difference if it needs more, or if the work runs out.
    fn myers(&mut self, a: Range<usize>, b: Range<usize>) {
        let (n, m) = (a.len(), b.len());
        // Every script needs at least as many edits as the lengths differ: one that needs more than the limit is a difference at once, without the search.
        if n.abs_diff(m) > MAX_EDIT_DISTANCE {
            self.differ(a, b);
            return;
        }
        let limit = MAX_EDIT_DISTANCE.min(n + m);
        // Diagonal k (x - y) is kept at index k + offset, so that every index is positive.
        let offset = limit + 1;
        let mut v = vec![0_usize; 2 * limit + 3];
        // trace[d] holds v for the diagonals -d to d after round d, at index k + d.
        let mut trace: Vec<Vec<usize>> = Vec::new();
        for d in 0..=limit {
            let mut kk = offset - d;
            while kk <= offset + d {
                let down = kk == offset - d || (kk != offset + d && v[kk - 1] < v[kk + 1]);
                let mut x = if down { v[kk + 1] } else { v[kk - 1] + 1 };
                let mut y = x + offset - kk;
                let start = x;
                while x < n && y < m && self.a[a.start + x] == self.b[b.start + y] {
                    x += 1;
                    y += 1;
                }
                self.spend(x - start + 1);
                v[kk] = x;
                if x >= n && y >= m {
                    trace.push(v[offset - d..=offset + d].to_vec());
                    self.path(&trace, offset, &a, &b);
                    return;
                }
                kk += 2;
            }
            trace.push(v[offset - d..=offset + d].to_vec());
            if self.exhausted() {
                self.complete = false;
                break;
            }
        }
        self.differ(a, b);
    }

    /// Follows the trace of [`Aligner::myers`] back from the end of `a` and `b` and adds the path's hunks.
    fn path(&mut self, trace: &[Vec<usize>], offset: usize, a: &Range<usize>, b: &Range<usize>) {
        let mut reversed = Vec::new();
        let (mut x, mut y) = (a.len(), b.len());
        for d in (1..trace.len()).rev() {
            let previous = &trace[d - 1];
            // The value of diagonal index kk after round d - 1.
            let at = |kk: usize| previous[kk + d - 1 - offset];
            let kk = x + offset - y;
            let down = kk == offset - d || (kk != offset + d && at(kk - 1) < at(kk + 1));
            let previous_kk = if down { kk + 1 } else { kk - 1 };
            let previous_x = at(previous_kk);
            let previous_y = previous_x + offset - previous_kk;
            let (snake_x, snake_y) = if down {
                (previous_x, previous_y + 1)
            } else {
                (previous_x + 1, previous_y)
            };
            reversed.push(Hunk {
                reference: a.start + snake_x..a.start + x,
                candidate: b.start + snake_y..b.start + y,
                equal: true,
            });
            reversed.push(Hunk {
                reference: a.start + previous_x..a.start + snake_x,
                candidate: b.start + previous_y..b.start + snake_y,
                equal: false,
            });
            x = previous_x;
            y = previous_y;
        }
        reversed.push(Hunk {
            reference: a.start..a.start + x,
            candidate: b.start..b.start + y,
            equal: true,
        });
        self.hunks.extend(
            reversed
                .into_iter()
                .rev()
                .filter(|hunk| !hunk.reference.is_empty() || !hunk.candidate.is_empty()),
        );
    }
}

/// The base of the rolling hash of runs: odd, so that multiplying by it modulo 2^64 loses nothing.
const HASH_BASE: u64 = 0x9E37_79B9_7F4A_7C15;

/// The runs of [`ANCHOR_LENGTH`] characters that occur exactly once in `range` of `text`, as (hash, position), sorted by hash. Hashes are polynomial in the characters, computed with wrapping arithmetic, so they are the same on every platform.
fn unique_grams(text: &[char], range: Range<usize>) -> Vec<(u64, usize)> {
    if range.len() < ANCHOR_LENGTH {
        return Vec::new();
    }
    let top = (1..ANCHOR_LENGTH).fold(1_u64, |power, _| power.wrapping_mul(HASH_BASE));
    let value = |character: char| u64::from(u32::from(character)) + 1;
    let mut grams = Vec::with_capacity(range.len() + 1 - ANCHOR_LENGTH);
    let mut hash = 0_u64;
    for (index, &character) in text[range.clone()].iter().enumerate() {
        if index >= ANCHOR_LENGTH {
            let leaving = text[range.start + index - ANCHOR_LENGTH];
            hash = hash.wrapping_sub(value(leaving).wrapping_mul(top));
        }
        hash = hash.wrapping_mul(HASH_BASE).wrapping_add(value(character));
        if index + 1 >= ANCHOR_LENGTH {
            grams.push((hash, range.start + index + 1 - ANCHOR_LENGTH));
        }
    }
    grams.sort_unstable();
    let mut unique = Vec::new();
    let mut index = 0;
    while let Some(&(hash, position)) = grams.get(index) {
        let mut next = index + 1;
        while grams.get(next).is_some_and(|&(other, _)| other == hash) {
            next += 1;
        }
        if next == index + 1 {
            unique.push((hash, position));
        }
        index = next;
    }
    unique
}

/// The work of sorting `n` items: `n` times the number of bits of `n`.
fn sort_cost(n: usize) -> usize {
    let bits = usize::try_from(usize::BITS - n.leading_zeros()).unwrap_or(usize::MAX);
    n.saturating_mul(bits)
}

/// The longest chain of `pairs` (sorted by their first position, which differ) whose second positions increase too, by patience sorting; of chains equally long, the one whose last pair has the smallest second position.
fn longest_increasing(pairs: &[(usize, usize)]) -> Vec<(usize, usize)> {
    // tails[k]: the index of the pair that ends the best chain of length k + 1 found so far.
    let mut tails: Vec<usize> = Vec::new();
    let mut previous: Vec<Option<usize>> = vec![None; pairs.len()];
    for (index, &(_, second)) in pairs.iter().enumerate() {
        let length = tails.partition_point(|&tail| pairs[tail].1 < second);
        if length > 0 {
            previous[index] = Some(tails[length - 1]);
        }
        if length == tails.len() {
            tails.push(index);
        } else {
            tails[length] = index;
        }
    }
    let mut chain = Vec::with_capacity(tails.len());
    let mut at = tails.last().copied();
    while let Some(index) = at {
        chain.push(pairs[index]);
        at = previous[index];
    }
    chain.reverse();
    chain
}

/// Joins a chain of anchors into blocks: anchors on one diagonal that overlap or touch become one block; an anchor that overlaps a block on another diagonal is left out.
fn blocks(chain: &[(usize, usize)]) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    for &(a, b) in chain {
        match blocks.last_mut() {
            Some(last) if a - last.a == b - last.b && a <= last.a + last.len => {
                last.len = last.len.max(a + ANCHOR_LENGTH - last.a);
            }
            Some(last) if a < last.a + last.len || b < last.b + last.len => {}
            _ => blocks.push(Block {
                a,
                b,
                len: ANCHOR_LENGTH,
            }),
        }
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn chars(text: &str) -> Vec<char> {
        text.chars().collect()
    }

    fn aligned(reference: &str, candidate: &str) -> Alignment {
        align(&chars(reference), &chars(candidate), &Limits::DEFAULT)
    }

    fn equal(reference: Range<usize>, candidate: Range<usize>) -> Hunk {
        Hunk {
            reference,
            candidate,
            equal: true,
        }
    }

    fn differ(reference: Range<usize>, candidate: Range<usize>) -> Hunk {
        Hunk {
            reference,
            candidate,
            equal: false,
        }
    }

    /// The length of the longest common subsequence, by dynamic programming.
    fn lcs(a: &[char], b: &[char]) -> usize {
        let mut row = vec![0_usize; b.len() + 1];
        for x in a {
            let mut diagonal = 0;
            for (j, y) in b.iter().enumerate() {
                let above = row[j + 1];
                row[j + 1] = if x == y {
                    diagonal + 1
                } else {
                    above.max(row[j])
                };
                diagonal = above;
            }
        }
        row[b.len()]
    }

    /// Checks that the hunks cover both streams in order, alternate, and pair equal characters.
    fn check_cover(alignment: &Alignment, a: &[char], b: &[char]) {
        let (mut x, mut y) = (0, 0);
        let mut last_equal = None;
        for hunk in alignment.hunks() {
            assert_eq!(hunk.reference.start, x);
            assert_eq!(hunk.candidate.start, y);
            assert!(!hunk.reference.is_empty() || !hunk.candidate.is_empty());
            assert_ne!(last_equal, Some(hunk.equal), "neighbours of the same kind");
            if hunk.equal {
                assert_eq!(a[hunk.reference.clone()], b[hunk.candidate.clone()]);
            }
            last_equal = Some(hunk.equal);
            x = hunk.reference.end;
            y = hunk.candidate.end;
        }
        assert_eq!((x, y), (a.len(), b.len()));
    }

    #[test]
    fn identical_and_empty_streams() {
        assert_eq!(aligned("abc", "abc").hunks(), [equal(0..3, 0..3)]);
        assert_eq!(aligned("abc", "").hunks(), [differ(0..3, 0..0)]);
        assert_eq!(aligned("", "abc").hunks(), [differ(0..0, 0..3)]);
        assert!(aligned("", "").hunks().is_empty());
        assert_eq!(aligned("", "").images(&[0]), [Image::strong(0, 0)]);
    }

    #[test]
    fn a_difference_between_common_text() {
        let alignment = aligned("abcdef", "abXYdef");
        assert_eq!(
            alignment.hunks(),
            [equal(0..2, 0..2), differ(2..3, 2..4), equal(3..6, 4..7)]
        );
        assert_eq!(alignment.matched(), 5);
        assert!(alignment.complete());
        let partners: Vec<_> = alignment.partners(1..5).collect();
        assert_eq!(partners, [(1, 1), (3, 4), (4, 5)]);
    }

    #[test]
    fn breaks_map_through_insertions_deletions_and_short_differences() {
        // Reference "1.Item" against "Item": the label is text only the reference has.
        let alignment = aligned("ab1.Item", "abItem");
        assert_eq!(
            alignment.images(&[0, 2, 3, 4, 8]),
            [
                Image::strong(0, 0),
                // Before text the candidate lacks: weak.
                Image::weak(2, 2),
                // Strictly inside text the candidate lacks: nowhere.
                Image::NONE,
                Image::strong(2, 2),
                Image::strong(6, 6),
            ]
        );
        // The candidate has a label the reference lacks: the break may lie before or after it.
        let alignment = aligned("abItem", "aba)Item");
        assert_eq!(alignment.images(&[2]), [Image::strong(2, 4)]);
        // A short difference on both sides (a date): a break inside it lies strictly inside the other side's.
        let alignment = aligned("x5Novy", "x16Novy");
        assert_eq!(alignment.hunks()[1], differ(1..2, 1..3));
        let alignment = aligned("xWednesdayy", "xThursdayy");
        assert_eq!(
            alignment.hunks(),
            [equal(0..1, 0..1), differ(1..6, 1..5), equal(6..11, 5..10)]
        );
        assert_eq!(alignment.images(&[3]), [Image::weak(2, 4)]);
    }

    #[test]
    fn breaks_inside_long_differences_have_no_image() {
        let reference: String = core::iter::repeat_n('a', 40).collect();
        let candidate: String = core::iter::repeat_n('b', 40).collect();
        let alignment = aligned(&reference, &candidate);
        assert_eq!(alignment.hunks(), [differ(0..40, 0..40)]);
        assert_eq!(
            alignment.images(&[0, 20, 40]),
            [Image::weak(0, 0), Image::NONE, Image::strong(40, 40)]
        );
        // A short one on both sides has an image: anywhere strictly inside the other side.
        let alignment = aligned("aaaa", "bbbbbb");
        assert_eq!(alignment.images(&[2]), [Image::weak(1, 5)]);
        // Unless the other side has no position strictly inside.
        let alignment = aligned("aaaa", "b");
        assert_eq!(alignment.images(&[2]), [Image::NONE]);
    }

    #[test]
    fn breaks_pair_in_order_and_each_once() {
        let images = [
            Image::strong(0, 0),
            Image::strong(2, 4),
            Image::strong(2, 4),
            Image::NONE,
            Image::strong(9, 9),
        ];
        assert_eq!(
            match_breaks(&images, &[0, 3, 7, 9]),
            [Some(0), Some(1), None, None, Some(3)]
        );
        assert_eq!(match_breaks(&images, &[]), [None, None, None, None, None]);
    }

    #[test]
    fn breaks_before_aligned_text_are_paired_first() {
        // A missing line: its start and the next line's start both fall where the next line starts in the candidate. `diff` deletes `wot`, so the next line would start inside the deletion; moved, the deletion is the missing line.
        let mut alignment = aligned("onetwothree", "onethree");
        assert_eq!(alignment.hunks()[1], differ(4..7, 4..4));
        alignment.slide_deletions(&chars("onetwothree"), &[0, 3, 6]);
        assert_eq!(
            alignment.hunks(),
            [equal(0..3, 0..3), differ(3..6, 3..3), equal(6..11, 3..8)]
        );
        let images = alignment.images(&[0, 3, 6]);
        assert_eq!(
            images,
            [Image::strong(0, 0), Image::weak(3, 3), Image::strong(3, 3)]
        );
        assert_eq!(
            match_breaks(&images, &[0, 3]),
            [Some(0), None, Some(1)],
            "the line both texts have agrees, not the missing one"
        );
        // A weak break alone still pairs, as a numbering label written differently does.
        let alignment = aligned("x1.Item", "xa)Item");
        let images = alignment.images(&[1]);
        assert_eq!(images, [Image::weak(1, 1)]);
        assert_eq!(match_breaks(&images, &[1]), [Some(0)]);
    }

    #[test]
    fn anchors_align_long_texts_with_scattered_differences() {
        let words = [
            "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india",
            "juliet", "kilo", "lima", "mike", "november", "oscar", "papa", "quebec", "romeo",
        ];
        let mut reference = String::new();
        let mut candidate = String::new();
        for round in 0..60 {
            for (index, word) in words.iter().enumerate() {
                reference.push_str(word);
                reference.push_str(&round.to_string());
                // Every seventh word differs in the candidate, and every 97th is missing.
                if !(round * words.len() + index).is_multiple_of(97) {
                    candidate.push_str(if (round + index) % 7 == 0 { "XX" } else { word });
                    candidate.push_str(&round.to_string());
                }
            }
        }
        let (a, b) = (chars(&reference), chars(&candidate));
        let alignment = align(&a, &b, &Limits::DEFAULT);
        check_cover(&alignment, &a, &b);
        assert!(alignment.complete());
        // Nearly everything that both have is aligned.
        assert!(alignment.matched() * 100 >= lcs(&a, &b) * 99);
    }

    #[test]
    fn the_work_limit_stops_the_alignment_but_keeps_it_whole() {
        let a = chars(&"abcdefghij".repeat(50));
        let b = chars(&"abcXefghij".repeat(50));
        let alignment = align(&a, &b, &Limits { max_work: 10 });
        check_cover(&alignment, &a, &b);
        assert!(!alignment.complete());
        let full = align(&a, &b, &Limits::DEFAULT);
        assert!(full.complete());
        assert_eq!(full.matched(), 450);
    }

    #[test]
    fn a_gap_whose_sides_differ_too_much_in_length_is_one_difference_at_once() {
        // A million characters against a few: no anchor, and at least a million edits.
        let a: Vec<char> = (0..1_000_000_u32).map(|i| char::from(b'a' + u8::try_from(i % 7).unwrap())).collect();
        let b = chars("xyz");
        let alignment = align(&a, &b, &Limits { max_work: 5_000_000 });
        assert!(alignment.complete(), "decided without spending the work limit");
        assert_eq!(alignment.hunks(), [differ(0..1_000_000, 0..3)]);
    }

    #[test]
    fn a_gap_needing_more_edits_than_the_limit_is_one_difference() {
        // Two texts over different letters, longer than the limit allows for Myers' algorithm, and without anchors.
        let a: Vec<char> = (0..3000)
            .map(|i| if i % 2 == 0 { 'a' } else { 'b' })
            .collect();
        let b: Vec<char> = (0..3000)
            .map(|i| if i % 3 == 0 { 'c' } else { 'a' })
            .collect();
        let alignment = align(&a, &b, &Limits::DEFAULT);
        check_cover(&alignment, &a, &b);
        assert!(alignment.complete());
    }

    proptest! {
        /// Without anchors (texts shorter than two anchors), the alignment is a shortest edit script: it matches as many characters as the longest common subsequence.
        #[test]
        fn short_alignments_are_optimal(a in proptest::collection::vec(0_u8..3, 0..40), b in proptest::collection::vec(0_u8..3, 0..40)) {
            let letters = |values: Vec<u8>| -> Vec<char> { values.into_iter().map(|value| char::from(b'a' + value)).collect() };
            let (a, b) = (letters(a), letters(b));
            let alignment = align(&a, &b, &Limits::DEFAULT);
            check_cover(&alignment, &a, &b);
            prop_assert_eq!(alignment.matched(), lcs(&a, &b));
        }

        /// Long alignments, with anchors, still cover both streams and pair only equal characters, and their images ascend.
        #[test]
        fn long_alignments_cover_both_streams(seed in proptest::collection::vec(0_u8..4, 0..400), edits in proptest::collection::vec((0_usize..400, 0_u8..5), 0..30)) {
            let a: Vec<char> = seed.iter().map(|&value| char::from(b'a' + value)).collect();
            let mut b = a.clone();
            for (position, value) in edits {
                let position = position.min(b.len());
                match value {
                    0 if position < b.len() => { b.remove(position); }
                    1 => b.insert(position, 'x'),
                    _ if position < b.len() => b[position] = char::from(b'a' + value),
                    _ => {}
                }
            }
            let mut alignment = align(&a, &b, &Limits::DEFAULT);
            check_cover(&alignment, &a, &b);
            let matched = alignment.matched();
            // Moving deletions to breaks keeps the alignment whole and as good.
            let some_breaks: Vec<usize> = (0..=a.len()).filter(|position| position % 7 == 3).collect();
            alignment.slide_deletions(&a, &some_breaks);
            check_cover(&alignment, &a, &b);
            prop_assert_eq!(alignment.matched(), matched);
            let breaks: Vec<usize> = (0..=a.len()).collect();
            let images = alignment.images(&breaks);
            let mut previous = Image::strong(0, 0);
            for image in images.iter().filter(|image| !image.is_empty()) {
                prop_assert!(image.low >= previous.low && image.high >= previous.high);
                prop_assert!(image.high <= b.len());
                previous = *image;
            }
        }
    }
}
