// SPDX-License-Identifier: MPL-2.0
//! Bounded synchronous resident-snapshot diff. Callers own scheduling.
use bareline_unicode_fold as casefold;
pub mod paged;
use bareline_document::{
    ContentStateId, DocumentSnapshot, Edit, EditTransaction, Revision, TextOffset,
};
use std::{
    collections::BTreeMap,
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};
use unicode_segmentation::UnicodeSegmentation;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Whitespace {
    #[default]
    Significant,
    TrimEdges,
    IgnoreAll,
}
#[derive(Clone, Debug)]
pub struct ResourceLimits {
    pub max_lines_exact: usize,
    pub max_bytes_exact: usize,
    pub time_budget_ms: u64,
    pub max_memory_bytes: usize,
}
impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_lines_exact: 2048,
            max_bytes_exact: 1024 * 1024,
            time_budget_ms: 100,
            max_memory_bytes: 16 * 1024 * 1024,
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct CompareOptions {
    pub whitespace: Whitespace,
    pub ignore_blank_lines: bool,
    pub ignore_case: bool,
    pub ignore_eol_style: bool,
    pub ignore_encoding_bom: bool,
    pub normalize_tabs: bool,
    pub limits: ResourceLimits,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffKind {
    Equal,
    Added,
    Removed,
    Changed,
    MovedAligned,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoarseReason {
    Lines,
    Bytes,
    Memory,
    Time,
    Windowed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareCompleteness {
    Exact,
    Coarse(CoarseReason),
    Cancelled,
    Unavailable,
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HunkId(pub u64);
#[derive(Clone, Debug)]
pub struct IntralineSpan {
    pub left: Range<TextOffset>,
    pub right: Range<TextOffset>,
}
#[derive(Clone, Debug)]
pub struct DiffHunk {
    pub stable_id: HunkId,
    pub left: Range<TextOffset>,
    pub right: Range<TextOffset>,
    pub left_line_hint: Option<usize>,
    pub right_line_hint: Option<usize>,
    pub kind: DiffKind,
    pub intraline: Vec<IntralineSpan>,
    pub left_revision: Revision,
    pub right_revision: Revision,
    left_state: ContentStateId,
    right_state: ContentStateId,
    options: CompareOptions,
}
#[derive(Clone, Debug, Default)]
pub struct DiffStats {
    pub input_bytes: usize,
    pub peak_accounted_bytes: usize,
}
#[derive(Clone, Debug)]
pub struct CompareResult {
    pub left_revision: Revision,
    pub right_revision: Revision,
    pub options: CompareOptions,
    pub hunks: Vec<DiffHunk>,
    pub completeness: CompareCompleteness,
    pub stats: DiffStats,
}
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);
impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release)
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
pub fn is_stale(r: &CompareResult, l: Revision, right: Revision) -> bool {
    r.left_revision != l || r.right_revision != right
}
struct Work<'a> {
    options: &'a CompareOptions,
    cancel: &'a CancelToken,
    start: Instant,
    used: usize,
}
impl Work<'_> {
    fn check(&self) -> Result<(), CompareCompleteness> {
        if self.cancel.is_cancelled() {
            Err(CompareCompleteness::Cancelled)
        } else if self.start.elapsed().as_millis() >= u128::from(self.options.limits.time_budget_ms)
        {
            Err(CompareCompleteness::Coarse(CoarseReason::Time))
        } else {
            Ok(())
        }
    }
    fn reserve(&mut self, n: usize) -> Result<(), CompareCompleteness> {
        let next = self.used.saturating_add(n);
        if next > self.options.limits.max_memory_bytes {
            return Err(CompareCompleteness::Coarse(CoarseReason::Memory));
        }
        self.used = next;
        Ok(())
    }
}
struct Line {
    range: Range<TextOffset>,
    text: String,
    hint: usize,
}
fn normalize(raw: &str, o: &CompareOptions, first: bool) -> String {
    let raw = if first && o.ignore_encoding_bom {
        raw.strip_prefix('\u{feff}').unwrap_or(raw)
    } else {
        raw
    };
    let (body, eol) = if let Some(body) = raw.strip_suffix("\r\n") {
        (body, "\r\n")
    } else if let Some(body) = raw.strip_suffix('\n') {
        (body, "\n")
    } else if let Some(body) = raw.strip_suffix('\r') {
        (body, "\r")
    } else {
        (raw, "")
    };
    let body = match o.whitespace {
        Whitespace::Significant => body.to_owned(),
        Whitespace::TrimEdges => body.trim().to_owned(),
        Whitespace::IgnoreAll => body.chars().filter(|c| !c.is_whitespace()).collect(),
    };
    let mut body = if o.normalize_tabs {
        let mut out = String::new();
        let mut col = 0;
        for c in body.chars() {
            if c == '\t' {
                let n = 4 - col % 4;
                out.extend(std::iter::repeat_n(' ', n));
                col += n;
            } else {
                out.push(c);
                col += 1;
            }
        }
        out
    } else {
        body
    };
    if o.ignore_case {
        body = casefold::fold(&body);
    }
    if o.ignore_eol_style && !eol.is_empty() {
        body.push('\n');
    } else {
        body.push_str(eol);
    }
    body
}

fn lines(s: &DocumentSnapshot, w: &mut Work<'_>) -> Result<Vec<Line>, CompareCompleteness> {
    let mut out = Vec::new();
    for hint in 0..s.line_count() {
        w.check()?;
        let range = s
            .line_range(hint)
            .map_err(|_| CompareCompleteness::Unavailable)?;
        if range.is_empty() {
            continue;
        }
        if out.len() >= w.options.limits.max_lines_exact {
            return Err(CompareCompleteness::Coarse(CoarseReason::Lines));
        }
        let len = range.end.0 - range.start.0;
        // Capped normalization units keep uninterrupted Unicode transformations small.
        if len > 64 * 1024 {
            return Err(CompareCompleteness::Coarse(CoarseReason::Bytes));
        }
        w.reserve(len.saturating_mul(16).saturating_add(512))?;
        let mut raw = String::with_capacity(len);
        for chunk in s
            .chunks(range.clone())
            .map_err(|_| CompareCompleteness::Unavailable)?
        {
            w.check()?;
            raw.push_str(chunk);
        }
        if w.options.ignore_blank_lines && raw.trim().is_empty() {
            continue;
        }
        let text = normalize(&raw, w.options, hint == 0);
        out.push(Line { range, text, hint });
    }
    Ok(out)
}
fn hash(s: &str) -> u64 {
    s.bytes().fold(14695981039346656037, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(1099511628211)
    })
}
fn make_hunk(
    l: &DocumentSnapshot,
    r: &DocumentSnapshot,
    left: Range<TextOffset>,
    right: Range<TextOffset>,
    hints: (usize, usize),
    id: u64,
) -> DiffHunk {
    let kind = if left.is_empty() {
        DiffKind::Added
    } else if right.is_empty() {
        DiffKind::Removed
    } else {
        DiffKind::Changed
    };
    DiffHunk {
        stable_id: HunkId(id),
        left,
        right,
        left_line_hint: Some(hints.0),
        right_line_hint: Some(hints.1),
        kind,
        intraline: Vec::new(),
        left_revision: l.revision,
        right_revision: r.revision,
        left_state: l.content_state,
        right_state: r.content_state,
        options: CompareOptions::default(),
    }
}
fn myers(
    a: &[Line],
    b: &[Line],
    w: &mut Work<'_>,
) -> Result<Vec<(usize, usize)>, CompareCompleteness> {
    let max = a.len() + b.len();
    let width = 2 * max + 3;
    let offset = max + 1;
    let row_bytes = width.saturating_mul(std::mem::size_of::<usize>());
    w.reserve(row_bytes)?;
    let mut v = vec![0usize; width];
    let mut trace = Vec::new();
    let mut final_d = 0;
    'search: for d in 0..=max {
        w.check()?;
        w.reserve(row_bytes + 64)?;
        trace.push(v.clone());
        for k in (-(d as isize)..=d as isize).step_by(2) {
            w.check()?;
            let idx = (offset as isize + k) as usize;
            let mut x = if k == -(d as isize) || (k != d as isize && v[idx - 1] < v[idx + 1]) {
                v[idx + 1]
            } else {
                v[idx - 1] + 1
            };
            let mut y = (x as isize - k) as usize;
            while x < a.len() && y < b.len() && a[x].text == b[y].text {
                w.check()?;
                x += 1;
                y += 1;
            }
            v[idx] = x;
            if x >= a.len() && y >= b.len() {
                final_d = d;
                break 'search;
            }
        }
    }
    let (mut x, mut y) = (a.len(), b.len());
    let mut pairs = Vec::new();
    for d in (0..=final_d).rev() {
        w.check()?;
        if d == 0 {
            while x > 0 && y > 0 {
                x -= 1;
                y -= 1;
                pairs.push((x, y));
            }
            break;
        }
        let v = &trace[d];
        let k = x as isize - y as isize;
        let idx = (offset as isize + k) as usize;
        let pk = if k == -(d as isize) || (k != d as isize && v[idx - 1] < v[idx + 1]) {
            k + 1
        } else {
            k - 1
        };
        let px = v[(offset as isize + pk) as usize];
        let py = (px as isize - pk) as usize;
        while x > px && y > py {
            x -= 1;
            y -= 1;
            pairs.push((x, y));
        }
        x = px;
        y = py;
    }
    pairs.reverse();
    Ok(pairs)
}
fn anchored(
    a: &[Line],
    b: &[Line],
    w: &mut Work<'_>,
    hash_line: fn(&str) -> u64,
) -> Result<Vec<(usize, usize)>, CompareCompleteness> {
    w.reserve((a.len() + b.len()).saturating_mul(256))?;
    let mut left = BTreeMap::new();
    let mut right = BTreeMap::new();
    for (i, line) in a.iter().enumerate() {
        w.check()?;
        left.entry((hash_line(&line.text), line.text.as_str()))
            .and_modify(|e: &mut (usize, usize)| e.1 += 1)
            .or_insert((i, 1));
    }
    for (i, line) in b.iter().enumerate() {
        w.check()?;
        right
            .entry((hash_line(&line.text), line.text.as_str()))
            .and_modify(|e: &mut (usize, usize)| e.1 += 1)
            .or_insert((i, 1));
    }
    // Normalized bytes break hash ties: a collision never establishes equality.
    let mut candidates = Vec::new();
    for (i, line) in a.iter().enumerate() {
        w.check()?;
        let key = (hash_line(&line.text), line.text.as_str());
        if let (1, Some(&(j, 1))) = (left[&key].1, right.get(&key)) {
            candidates.push((i, j));
        }
    }
    let mut tails: Vec<usize> = Vec::new();
    let mut prev = vec![None; candidates.len()];
    for (idx, &(_, j)) in candidates.iter().enumerate() {
        w.check()?;
        let p = tails.partition_point(|&t| candidates[t].1 < j);
        if p > 0 {
            prev[idx] = Some(tails[p - 1]);
        }
        if p == tails.len() {
            tails.push(idx)
        } else {
            tails[p] = idx;
        }
    }
    let mut anchors = Vec::new();
    let mut at = tails.last().copied();
    while let Some(idx) = at {
        anchors.push(candidates[idx]);
        at = prev[idx];
    }
    anchors.reverse();
    let mut pairs = Vec::new();
    let (mut x, mut y) = (0, 0);
    for (i, j) in anchors
        .into_iter()
        .chain(std::iter::once((a.len(), b.len())))
    {
        for (dx, dy) in myers(&a[x..i], &b[y..j], w)? {
            pairs.push((x + dx, y + dy));
        }
        if i < a.len() {
            pairs.push((i, j));
        }
        x = i + 1;
        y = j + 1;
    }
    Ok(pairs)
}
fn intraline(
    a: &Line,
    b: &Line,
    l: &DocumentSnapshot,
    r: &DocumentSnapshot,
    w: &Work<'_>,
) -> Result<IntralineSpan, CompareCompleteness> {
    let l = l
        .read(a.range.clone(), a.range.end.0 - a.range.start.0)
        .map_err(|_| CompareCompleteness::Unavailable)?;
    let r = r
        .read(b.range.clone(), b.range.end.0 - b.range.start.0)
        .map_err(|_| CompareCompleteness::Unavailable)?;
    let mut prefix = 0;
    for (x, y) in l.split_word_bounds().zip(r.split_word_bounds()) {
        w.check()?;
        if x != y {
            break;
        }
        prefix += x.len();
    }
    let mut suffix = 0;
    for (x, y) in l[prefix..]
        .split_word_bounds()
        .rev()
        .zip(r[prefix..].split_word_bounds().rev())
    {
        w.check()?;
        if x != y {
            break;
        }
        suffix += x.len();
    }
    let lm = &l[prefix..l.len() - suffix];
    let rm = &r[prefix..r.len() - suffix];
    let mut gp = 0;
    for (x, y) in lm.graphemes(true).zip(rm.graphemes(true)) {
        w.check()?;
        if x != y {
            break;
        }
        gp += x.len();
    }
    let mut gs = 0;
    for (x, y) in lm[gp..]
        .graphemes(true)
        .rev()
        .zip(rm[gp..].graphemes(true).rev())
    {
        w.check()?;
        if x != y {
            break;
        }
        gs += x.len();
    }
    Ok(IntralineSpan {
        left: TextOffset(a.range.start.0 + prefix + gp)..TextOffset(a.range.end.0 - suffix - gs),
        right: TextOffset(b.range.start.0 + prefix + gp)..TextOffset(b.range.end.0 - suffix - gs),
    })
}
/// Unique patience anchors followed by bounded Myers. Cap exhaustion emits a single
/// coarse original-range block. Output and workspace share a conservative byte cap.
pub fn compare(
    l: &DocumentSnapshot,
    r: &DocumentSnapshot,
    o: &CompareOptions,
    c: &CancelToken,
) -> CompareResult {
    compare_hashed(l, r, o, c, hash)
}
fn compare_hashed(
    l: &DocumentSnapshot,
    r: &DocumentSnapshot,
    o: &CompareOptions,
    c: &CancelToken,
    hash_line: fn(&str) -> u64,
) -> CompareResult {
    let mut w = Work {
        options: o,
        cancel: c,
        start: Instant::now(),
        used: 0,
    };
    let mut result = CompareResult {
        left_revision: l.revision,
        right_revision: r.revision,
        options: o.clone(),
        hunks: Vec::new(),
        completeness: CompareCompleteness::Exact,
        stats: DiffStats {
            input_bytes: l.len().saturating_add(r.len()),
            peak_accounted_bytes: 0,
        },
    };
    let run = (|| -> Result<(), CompareCompleteness> {
        if c.is_cancelled() {
            return Err(CompareCompleteness::Cancelled);
        }
        if !l.is_complete() || !r.is_complete() {
            return Err(CompareCompleteness::Unavailable);
        }
        if o.limits.max_memory_bytes < std::mem::size_of::<DiffHunk>() * 2 {
            return Err(CompareCompleteness::Failed);
        }
        w.check()?;
        if result.stats.input_bytes > o.limits.max_bytes_exact {
            return Err(CompareCompleteness::Coarse(CoarseReason::Bytes));
        }
        let a = lines(l, &mut w)?;
        let b = lines(r, &mut w)?;
        w.reserve(
            (a.len() + b.len() + 1).saturating_mul(std::mem::size_of::<DiffHunk>() * 2 + 128),
        )?;
        let pairs = anchored(&a, &b, &mut w, hash_line)?;
        let (mut si, mut sj) = (0, 0);
        let mut anchor = 0u64;
        let mut context_occurrences = BTreeMap::<u64, u64>::new();
        for (i, j) in pairs.into_iter().chain(std::iter::once((a.len(), b.len()))) {
            w.check()?;
            if si < i || sj < j {
                let ar = if si < i {
                    a[si].range.start..a[i - 1].range.end
                } else {
                    let p = a.get(i).map_or(TextOffset(l.len()), |x| x.range.start);
                    p..p
                };
                let br = if sj < j {
                    b[sj].range.start..b[j - 1].range.end
                } else {
                    let p = b.get(j).map_or(TextOffset(r.len()), |x| x.range.start);
                    p..p
                };
                let next = a.get(i).map_or(0, |x| hash(&x.text));
                let mut content_id = 0u64;
                for line in a[si..i].iter().chain(&b[sj..j]) {
                    w.check()?;
                    content_id = content_id.rotate_left(7) ^ hash(&line.text);
                }
                let base_id = anchor.rotate_left(17) ^ next ^ content_id.rotate_left(31);
                let ordinal = context_occurrences.entry(base_id).or_default();
                let id = base_id ^ ordinal.wrapping_mul(0x9e3779b97f4a7c15);
                *ordinal += 1;
                let mut change = make_hunk(
                    l,
                    r,
                    ar,
                    br,
                    (
                        a.get(si).map_or(l.line_count() - 1, |x| x.hint),
                        b.get(sj).map_or(r.line_count() - 1, |x| x.hint),
                    ),
                    id,
                );
                for (al, br) in a[si..i].iter().zip(&b[sj..j]) {
                    change.intraline.push(intraline(al, br, l, r, &w)?);
                }
                change.options = o.clone();
                result.hunks.push(change);
            }
            anchor = a.get(i).map_or(0, |x| hash(&x.text));
            si = i + 1;
            sj = j + 1;
        }
        for removed in 0..result.hunks.len() {
            if result.hunks[removed].kind != DiffKind::Removed {
                continue;
            }
            for added in removed.saturating_sub(64)..result.hunks.len().min(removed + 65) {
                w.check()?;
                if result.hunks[added].kind != DiffKind::Added {
                    continue;
                }
                let lr = result.hunks[removed].left.clone();
                let rr = result.hunks[added].right.clone();
                if lr.end.0 - lr.start.0 > 64 * 1024
                    || lr.end.0 - lr.start.0 != rr.end.0 - rr.start.0
                {
                    continue;
                }
                let lt = l
                    .read(lr, o.limits.max_bytes_exact)
                    .map_err(|_| CompareCompleteness::Unavailable)?;
                let rt = r
                    .read(rr, o.limits.max_bytes_exact)
                    .map_err(|_| CompareCompleteness::Unavailable)?;
                if lt == rt {
                    result.hunks[removed].kind = DiffKind::MovedAligned;
                    result.hunks[added].kind = DiffKind::MovedAligned;
                    break;
                }
            }
        }
        Ok(())
    })();
    if let Err(state) = run {
        result.hunks.clear();
        result.completeness = state;
        if matches!(state, CompareCompleteness::Coarse(_)) {
            result.hunks.push(make_hunk(
                l,
                r,
                TextOffset(0)..TextOffset(l.len()),
                TextOffset(0)..TextOffset(r.len()),
                (0, 0),
                0,
            ));
        }
    }
    for h in &mut result.hunks {
        h.options = o.clone();
    }
    result.stats.peak_accounted_bytes = w.used;
    result
}
/// Incrementally compares bounded logical-line windows and delivers acknowledged
/// batches before reading the next window. Cross-window alignment is explicitly coarse.
/// The callback owns its backpressure wait; false stops immediately without more reads.
pub fn compare_batches(
    l: &DocumentSnapshot,
    r: &DocumentSnapshot,
    o: &CompareOptions,
    c: &CancelToken,
    batch_size: usize,
    mut sink: impl FnMut(&[DiffHunk]) -> bool,
) -> CompareResult {
    use bareline_document::{Budget, Document};
    let started = Instant::now();
    let mut out = CompareResult {
        left_revision: l.revision,
        right_revision: r.revision,
        options: o.clone(),
        hunks: Vec::new(),
        completeness: CompareCompleteness::Exact,
        stats: DiffStats {
            input_bytes: l.len().saturating_add(r.len()),
            peak_accounted_bytes: 0,
        },
    };
    if c.is_cancelled() {
        out.completeness = CompareCompleteness::Cancelled;
        return out;
    }
    if !l.is_complete() || !r.is_complete() {
        out.completeness = CompareCompleteness::Unavailable;
        return out;
    }
    if o.limits.max_memory_bytes < std::mem::size_of::<DiffHunk>() * 4 {
        out.completeness = CompareCompleteness::Failed;
        return out;
    }
    let cap = (o.limits.max_memory_bytes / 32)
        .min(o.limits.max_bytes_exact / 2)
        .clamp(1, 64 * 1024);
    let (mut li, mut ri) = (0, 0);
    let (mut lp, mut rp) = (TextOffset(0), TextOffset(0));
    let mut windows = 0;
    while lp.0 < l.len() || rp.0 < r.len() {
        if c.is_cancelled() {
            out.completeness = CompareCompleteness::Cancelled;
            return out;
        }
        if started.elapsed().as_millis() >= u128::from(o.limits.time_budget_ms) {
            let mut h = make_hunk(
                l,
                r,
                lp..TextOffset(l.len()),
                rp..TextOffset(r.len()),
                (li, ri),
                0,
            );
            h.options = o.clone();
            out.completeness = if sink(&[h]) {
                CompareCompleteness::Coarse(CoarseReason::Time)
            } else {
                CompareCompleteness::Cancelled
            };
            return out;
        }
        let mut stop = || {
            c.is_cancelled() || started.elapsed().as_millis() >= u128::from(o.limits.time_budget_ms)
        };
        let (lr, ln) = window(
            l,
            li,
            lp,
            cap,
            o.limits.max_lines_exact.clamp(1, 64),
            &mut stop,
        );
        let (rr, rn) = window(
            r,
            ri,
            rp,
            cap,
            o.limits.max_lines_exact.clamp(1, 64),
            &mut stop,
        );
        if stop() {
            continue;
        }
        let too_large = lr.end.0 - lr.start.0 > cap || rr.end.0 - rr.start.0 > cap;
        if too_large {
            let mut h = make_hunk(l, r, lr.clone(), rr.clone(), (li, ri), 0);
            h.options = o.clone();
            if !sink(&[h]) {
                out.completeness = CompareCompleteness::Cancelled;
                return out;
            }
            out.completeness = CompareCompleteness::Coarse(CoarseReason::Bytes);
        } else {
            let raw_l = match l.read(lr.clone(), cap) {
                Ok(v) => v,
                Err(_) => {
                    out.completeness = CompareCompleteness::Unavailable;
                    return out;
                }
            };
            if c.is_cancelled() {
                out.completeness = CompareCompleteness::Cancelled;
                return out;
            }
            let raw_r = match r.read(rr.clone(), cap) {
                Ok(v) => v,
                Err(_) => {
                    out.completeness = CompareCompleteness::Unavailable;
                    return out;
                }
            };
            let memory = Budget::new(o.limits.max_memory_bytes / 4);
            let history = Budget::new(0);
            let (Ok(ld), Ok(rd)) = (
                Document::from_utf8(&raw_l, memory.clone(), history.clone()),
                Document::from_utf8(&raw_r, memory, history),
            ) else {
                out.completeness = CompareCompleteness::Failed;
                return out;
            };
            let mut local = o.clone();
            local.ignore_encoding_bom = o.ignore_encoding_bom && lp.0 == 0 && rp.0 == 0;
            local.limits.max_memory_bytes /= 2;
            local.limits.time_budget_ms = o
                .limits
                .time_budget_ms
                .saturating_sub(started.elapsed().as_millis() as u64);
            let mut result = compare(&ld.snapshot(), &rd.snapshot(), &local, c);
            out.stats.peak_accounted_bytes = out
                .stats
                .peak_accounted_bytes
                .max(result.stats.peak_accounted_bytes.saturating_add(cap * 4));
            if !matches!(
                result.completeness,
                CompareCompleteness::Exact | CompareCompleteness::Coarse(_)
            ) {
                out.completeness = result.completeness;
                return out;
            }
            if result.completeness != CompareCompleteness::Exact {
                out.completeness = result.completeness;
            }
            for h in &mut result.hunks {
                h.left = TextOffset(h.left.start.0 + lp.0)..TextOffset(h.left.end.0 + lp.0);
                h.right = TextOffset(h.right.start.0 + rp.0)..TextOffset(h.right.end.0 + rp.0);
                for span in &mut h.intraline {
                    span.left =
                        TextOffset(span.left.start.0 + lp.0)..TextOffset(span.left.end.0 + lp.0);
                    span.right =
                        TextOffset(span.right.start.0 + rp.0)..TextOffset(span.right.end.0 + rp.0);
                }
                h.left_line_hint = h.left_line_hint.map(|n| n + li);
                h.right_line_hint = h.right_line_hint.map(|n| n + ri);
                h.left_revision = l.revision;
                h.right_revision = r.revision;
                h.left_state = l.content_state;
                h.right_state = r.content_state;
                h.options = o.clone();
            }
            for batch in result.hunks.chunks(batch_size.max(1)) {
                if c.is_cancelled() || !sink(batch) {
                    out.completeness = CompareCompleteness::Cancelled;
                    return out;
                }
            }
        }
        lp = lr.end;
        rp = rr.end;
        li = ln;
        ri = rn;
        windows += 1;
    }
    if windows > 1 && out.completeness == CompareCompleteness::Exact {
        out.completeness = CompareCompleteness::Coarse(CoarseReason::Windowed)
    }
    out
}
fn window(
    s: &DocumentSnapshot,
    mut line: usize,
    start: TextOffset,
    cap: usize,
    max_lines: usize,
    stop: &mut impl FnMut() -> bool,
) -> (Range<TextOffset>, usize) {
    let mut end = start;
    let mut count = 0;
    while end.0 < s.len() && count < max_lines && !stop() {
        let Ok(range) = s.line_range(line) else { break };
        if count > 0 && range.end.0 - start.0 > cap {
            break;
        }
        end = range.end;
        line += 1;
        count += 1;
    }
    (start..end, line)
}
#[derive(Clone, Copy, Debug)]
pub enum Direction {
    LeftToRight,
    RightToLeft,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyError {
    Unavailable,
    Stale,
    InvalidRange,
    BudgetExceeded,
    UnsupportedPreserve,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergePolicy {
    PreserveIgnoredDestination,
    CopySelectedRange,
}
/// Default merge preserves option-ignored destination text. Unsupported precise
/// preservation returns an error; callers may explicitly preview and select full copy.
pub fn apply_hunk(
    direction: Direction,
    h: &DiffHunk,
    l: &DocumentSnapshot,
    r: &DocumentSnapshot,
    max_insert_bytes: usize,
) -> Result<EditTransaction, ApplyError> {
    apply_hunk_with_policy(
        MergePolicy::PreserveIgnoredDestination,
        direction,
        h,
        l,
        r,
        max_insert_bytes,
    )
}
/// CopySelectedRange explicitly includes ignored text inside the selected range.
pub fn apply_hunk_with_policy(
    policy: MergePolicy,
    direction: Direction,
    h: &DiffHunk,
    l: &DocumentSnapshot,
    r: &DocumentSnapshot,
    max_insert_bytes: usize,
) -> Result<EditTransaction, ApplyError> {
    if !l.is_complete() || !r.is_complete() {
        return Err(ApplyError::Unavailable);
    }
    if l.revision != h.left_revision
        || r.revision != h.right_revision
        || l.content_state != h.left_state
        || r.content_state != h.right_state
    {
        return Err(ApplyError::Stale);
    }
    let (source, target, srange, trange) = match direction {
        Direction::LeftToRight => (l, r, h.left.clone(), h.right.clone()),
        Direction::RightToLeft => (r, l, h.right.clone(), h.left.clone()),
    };
    source
        .chunks(srange.clone())
        .map_err(|_| ApplyError::InvalidRange)?;
    target
        .chunks(trange.clone())
        .map_err(|_| ApplyError::InvalidRange)?;
    if policy == MergePolicy::PreserveIgnoredDestination && ignores(&h.options) {
        return preserve(source, target, srange, trange, &h.options, max_insert_bytes);
    }
    target
        .chunks(trange.clone())
        .map_err(|_| ApplyError::InvalidRange)?;
    let insert = source.read(srange, max_insert_bytes).map_err(|e| {
        if e == bareline_document::Error::BudgetExceeded {
            ApplyError::BudgetExceeded
        } else {
            ApplyError::InvalidRange
        }
    })?;
    Ok(EditTransaction {
        base_revision: target.revision,
        edits: vec![Edit {
            range: trange,
            insert,
        }],
    })
}
fn ignores(o: &CompareOptions) -> bool {
    o.whitespace != Whitespace::Significant
        || o.ignore_blank_lines
        || o.ignore_case
        || o.ignore_eol_style
        || o.ignore_encoding_bom
        || o.normalize_tabs
}
fn tokens(
    snapshot: &DocumentSnapshot,
    range: Range<TextOffset>,
    o: &CompareOptions,
    max: usize,
) -> Result<Vec<Line>, ApplyError> {
    let raw = snapshot
        .read(range.clone(), max)
        .map_err(|_| ApplyError::BudgetExceeded)?;
    let mut out = Vec::new();
    let mut offset = range.start.0;
    for line in raw.split_inclusive('\n') {
        let skip = o.ignore_blank_lines && line.trim().is_empty();
        let body = line.trim_end_matches(['\r', '\n']);
        let trim_start = body.len() - body.trim_start().len();
        let trim_end = body.trim_end().len();
        let mut graphemes = line.grapheme_indices(true).peekable();
        let mut column = 0usize;
        while let Some((idx, g)) = graphemes.next() {
            let mut end = idx + g.len();
            let mut normalized = None;
            if o.normalize_tabs && (g == " " || g == "\t") {
                let initial = column;
                column += if g == "\t" { 4 - column % 4 } else { 1 };
                while let Some(&(next, part)) = graphemes.peek() {
                    if part != " " && part != "\t" {
                        break;
                    }
                    column += if part == "\t" { 4 - column % 4 } else { 1 };
                    end = next + part.len();
                    graphemes.next();
                }
                normalized = Some(" ".repeat(column - initial));
            } else {
                column += g.chars().count();
            }
            let whitespace = g.chars().all(char::is_whitespace);
            if skip
                || (o.ignore_encoding_bom && offset + idx == 0 && g == "\u{feff}")
                || (o.whitespace == Whitespace::IgnoreAll && whitespace && idx < body.len())
                || (o.whitespace == Whitespace::TrimEdges
                    && idx < body.len()
                    && (idx < trim_start || idx >= trim_end))
            {
                continue;
            }
            let text = if o.ignore_eol_style && g.chars().all(|c| c == '\r' || c == '\n') {
                "\n".to_owned()
            } else if let Some(normalized) = normalized {
                normalized
            } else if o.ignore_case {
                casefold::fold(g)
            } else {
                g.to_owned()
            };
            let original = TextOffset(offset + idx)..TextOffset(offset + end);
            if o.ignore_case && !whitespace {
                for part in text.graphemes(true) {
                    out.push(Line {
                        range: original.clone(),
                        text: part.to_owned(),
                        hint: 0,
                    });
                }
            } else {
                out.push(Line {
                    range: original,
                    text,
                    hint: 0,
                });
            }
        }
        offset += line.len();
    }
    Ok(out)
}
fn preserve(
    source: &DocumentSnapshot,
    target: &DocumentSnapshot,
    srange: Range<TextOffset>,
    trange: Range<TextOffset>,
    o: &CompareOptions,
    max: usize,
) -> Result<EditTransaction, ApplyError> {
    let bytes = (srange.end.0 - srange.start.0).saturating_add(trange.end.0 - trange.start.0);
    if bytes > max || bytes.saturating_mul(256) > o.limits.max_memory_bytes {
        return Err(ApplyError::BudgetExceeded);
    }
    let a = tokens(target, trange.clone(), o, max)?;
    let b = tokens(source, srange, o, max)?;
    let cancel = CancelToken::default();
    let mut w = Work {
        options: o,
        cancel: &cancel,
        start: Instant::now(),
        used: bytes.saturating_mul(256),
    };
    let pairs = myers(&a, &b, &mut w).map_err(|_| ApplyError::BudgetExceeded)?;
    let mut segments: Vec<(usize, usize, usize, usize)> = Vec::new();
    let (mut si, mut sj) = (0, 0);
    for (i, j) in pairs
        .iter()
        .copied()
        .chain(std::iter::once((a.len(), b.len())))
    {
        if si < i || sj < j {
            let mut segment = (si, i, sj, j);
            loop {
                w.check().map_err(|_| ApplyError::BudgetExceeded)?;
                let old = segment;
                let (a0, a1) = expand_original(&a, segment.0, segment.1);
                let (b0, b1) = expand_original(&b, segment.2, segment.3);
                segment = (a0, a1, b0, b1);
                for (index, &(ai, bj)) in pairs.iter().enumerate() {
                    if index % 64 == 0 {
                        w.check().map_err(|_| ApplyError::BudgetExceeded)?;
                    }
                    if ai >= segment.0 && ai < segment.1 {
                        segment.2 = segment.2.min(bj);
                        segment.3 = segment.3.max(bj + 1);
                    }
                    if bj >= segment.2 && bj < segment.3 {
                        segment.0 = segment.0.min(ai);
                        segment.1 = segment.1.max(ai + 1);
                    }
                }
                if segment == old {
                    break;
                }
            }
            if let Some(previous) = segments.last_mut() {
                if segment.0 <= previous.1
                    || segment.2 <= previous.3
                    || (segment.0 == previous.0 && segment.1 == previous.1)
                {
                    previous.0 = previous.0.min(segment.0);
                    previous.1 = previous.1.max(segment.1);
                    previous.2 = previous.2.min(segment.2);
                    previous.3 = previous.3.max(segment.3);
                } else {
                    segments.push(segment);
                }
            } else {
                segments.push(segment);
            }
        }
        si = i + 1;
        sj = j + 1;
    }
    let mut edits = Vec::new();
    for (si, i, sj, j) in segments {
        let mut insert = String::new();
        let mut previous = None;
        for token in &b[sj..j] {
            if previous.as_ref() == Some(&token.range) {
                continue;
            }
            insert.push_str(
                &source
                    .read(token.range.clone(), max)
                    .map_err(|_| ApplyError::Unavailable)?,
            );
            previous = Some(token.range.clone());
        }
        if si < i {
            let mut previous = None;
            for token in &a[si..i] {
                if previous.as_ref() == Some(&token.range) {
                    continue;
                }
                edits.push(Edit {
                    range: token.range.clone(),
                    insert: std::mem::take(&mut insert),
                });
                previous = Some(token.range.clone());
            }
        } else {
            let at = a.get(i).map_or(trange.end, |x| x.range.start);
            edits.push(Edit {
                range: at..at,
                insert,
            });
        }
    }
    Ok(EditTransaction {
        base_revision: target.revision,
        edits,
    })
}
// Expand partial case-fold units back to an indivisible original grapheme.
fn expand_original(tokens: &[Line], mut start: usize, mut end: usize) -> (usize, usize) {
    if start == end
        && start > 0
        && start < tokens.len()
        && tokens[start - 1].range == tokens[start].range
    {
        start -= 1;
        end += 1;
    }
    if start < end {
        while start > 0 && tokens[start - 1].range == tokens[start].range {
            start -= 1;
        }
        while end < tokens.len() && tokens[end - 1].range == tokens[end].range {
            end += 1;
        }
    }
    (start, end)
}
#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Document};
    fn doc(s: &str) -> Document {
        Document::from_utf8(s, Budget::new(1_000_000), Budget::new(1_000_000)).unwrap()
    }
    fn text(d: &Document) -> String {
        let s = d.snapshot();
        s.read(TextOffset(0)..TextOffset(s.len()), 100000).unwrap()
    }
    #[test]
    fn generated_short_sequences_apply_exactly() {
        let mut inputs = vec![String::new()];
        for len in 1..=3 {
            for bits in 0..(1 << len) {
                inputs.push(
                    (0..len)
                        .map(|i| if bits & (1 << i) == 0 { "a\n" } else { "b\n" })
                        .collect::<String>(),
                );
            }
        }
        for left in &inputs {
            for right in &inputs {
                let mut l = doc(left);
                let ls = l.snapshot();
                let rs = doc(right).snapshot();
                let result = compare(
                    &ls,
                    &rs,
                    &CompareOptions::default(),
                    &CancelToken::default(),
                );
                assert_eq!(result.completeness, CompareCompleteness::Exact);
                let edits = result
                    .hunks
                    .iter()
                    .flat_map(|h| {
                        apply_hunk(Direction::RightToLeft, h, &ls, &rs, 1000)
                            .unwrap()
                            .edits
                    })
                    .collect();
                l.apply(EditTransaction {
                    base_revision: ls.revision,
                    edits,
                })
                .unwrap();
                assert_eq!(text(&l), *right);
                if left != right {
                    l.undo().unwrap();
                    assert_eq!(text(&l), *left);
                }
            }
        }
    }
    #[test]
    fn unicode_crlf_apply_undo() {
        for (left, right) in [
            ("e\u{301}\n🙂\n漢\n", "e\u{301}\n🙃\n字\n"),
            ("a\r\nb\r", "x\r\nb\r"),
        ] {
            let mut l = doc(left);
            let ls = l.snapshot();
            let rs = doc(right).snapshot();
            let result = compare(
                &ls,
                &rs,
                &CompareOptions::default(),
                &CancelToken::default(),
            );
            let edits = result
                .hunks
                .iter()
                .flat_map(|h| {
                    apply_hunk(Direction::RightToLeft, h, &ls, &rs, 1000)
                        .unwrap()
                        .edits
                })
                .collect();
            l.apply(EditTransaction {
                base_revision: ls.revision,
                edits,
            })
            .unwrap();
            assert_eq!(text(&l), right);
            l.undo().unwrap();
            assert_eq!(text(&l), left);
        }
    }
    #[test]
    fn cancellation_and_caps_are_distinct() {
        let l = doc("left").snapshot();
        let r = doc("right").snapshot();
        let mut o = CompareOptions::default();
        o.limits.max_bytes_exact = 1;
        assert_eq!(
            compare(&l, &r, &o, &CancelToken::default()).completeness,
            CompareCompleteness::Coarse(CoarseReason::Bytes)
        );
        let c = CancelToken::default();
        c.cancel();
        let result = compare(&l, &r, &o, &c);
        assert_eq!(result.completeness, CompareCompleteness::Cancelled);
        assert!(result.hunks.is_empty());
        o.limits.max_memory_bytes = 0;
        assert_eq!(
            compare(&l, &r, &o, &CancelToken::default()).completeness,
            CompareCompleteness::Failed
        );
    }
    #[test]
    fn ignores_and_original_coordinates() {
        let l = doc(" same \nold\n").snapshot();
        let r = doc("same\nNEW\n").snapshot();
        let o = CompareOptions {
            whitespace: Whitespace::TrimEdges,
            ..Default::default()
        };
        let result = compare(&l, &r, &o, &CancelToken::default());
        assert_eq!(result.hunks.len(), 1);
        assert_eq!(result.hunks[0].left, TextOffset(7)..TextOffset(11));
        for (left, right, o) in [
            (
                "A",
                "a",
                CompareOptions {
                    ignore_case: true,
                    ..Default::default()
                },
            ),
            (
                "a\r\n",
                "a\n",
                CompareOptions {
                    ignore_eol_style: true,
                    ..Default::default()
                },
            ),
            (
                "\u{feff}a",
                "a",
                CompareOptions {
                    ignore_encoding_bom: true,
                    ..Default::default()
                },
            ),
            (
                "a\tb",
                "a   b",
                CompareOptions {
                    normalize_tabs: true,
                    ..Default::default()
                },
            ),
            (
                "a b",
                "ab",
                CompareOptions {
                    whitespace: Whitespace::IgnoreAll,
                    ..Default::default()
                },
            ),
            (
                "a\n\nb\n",
                "a\nb\n",
                CompareOptions {
                    ignore_blank_lines: true,
                    ..Default::default()
                },
            ),
        ] {
            assert!(
                compare(
                    &doc(left).snapshot(),
                    &doc(right).snapshot(),
                    &o,
                    &CancelToken::default()
                )
                .hunks
                .is_empty()
            );
        }
    }
    #[test]
    fn stale_and_wrong_document() {
        let mut l = doc("a");
        let ls = l.snapshot();
        let rs = doc("b").snapshot();
        let result = compare(
            &ls,
            &rs,
            &CompareOptions::default(),
            &CancelToken::default(),
        );
        l.apply(EditTransaction {
            base_revision: ls.revision,
            edits: vec![Edit {
                range: TextOffset(0)..TextOffset(1),
                insert: "c".into(),
            }],
        })
        .unwrap();
        assert!(is_stale(&result, l.snapshot().revision, rs.revision));
        assert!(matches!(
            apply_hunk(
                Direction::RightToLeft,
                &result.hunks[0],
                &l.snapshot(),
                &rs,
                100
            ),
            Err(ApplyError::Stale)
        ));
        assert!(matches!(
            apply_hunk(
                Direction::RightToLeft,
                &result.hunks[0],
                &doc("a").snapshot(),
                &rs,
                100
            ),
            Err(ApplyError::Stale)
        ));
    }
    #[test]
    fn graphemes_batches_stable_anchors() {
        let l = doc("anchor\ne\u{301} 🙂 tail\nend\n").snapshot();
        let r = doc("anchor\ne\u{301} 🙃 tail\nend\n").snapshot();
        let result = compare(&l, &r, &CompareOptions::default(), &CancelToken::default());
        let span = &result.hunks[0].intraline[0];
        assert_eq!(l.read(span.left.clone(), 100).unwrap(), "🙂");
        assert_eq!(r.read(span.right.clone(), 100).unwrap(), "🙃");
        let l2 = doc("unrelated\nanchor\ne\u{301} 🙂 tail\nend\n").snapshot();
        let r2 = doc("unrelated\nanchor\ne\u{301} 🙃 tail\nend\n").snapshot();
        assert_eq!(
            result.hunks[0].stable_id,
            compare(
                &l2,
                &r2,
                &CompareOptions::default(),
                &CancelToken::default()
            )
            .hunks[0]
                .stable_id
        );
        let mut calls = 0;
        let stopped = compare_batches(
            &l,
            &r,
            &CompareOptions::default(),
            &CancelToken::default(),
            1,
            |batch| {
                calls += 1;
                assert_eq!(batch.len(), 1);
                false
            },
        );
        assert_eq!(calls, 1);
        assert_eq!(stopped.completeness, CompareCompleteness::Cancelled);
    }
    #[test]
    fn incomplete_prefix_is_neither_exact_nor_applicable() {
        let mut builder =
            bareline_document::DocumentBuilder::new(Budget::new(10000), Budget::new(10000))
                .unwrap();
        builder.append("a").unwrap();
        let prefix = builder.prefix();
        let right = doc("b").snapshot();
        let result = compare(
            &prefix,
            &right,
            &CompareOptions::default(),
            &CancelToken::default(),
        );
        assert_eq!(result.completeness, CompareCompleteness::Unavailable);
        assert!(result.hunks.is_empty());
        let h = make_hunk(
            &prefix,
            &right,
            TextOffset(0)..TextOffset(1),
            TextOffset(0)..TextOffset(1),
            (0, 0),
            0,
        );
        assert!(matches!(
            apply_hunk(Direction::LeftToRight, &h, &prefix, &right, 100),
            Err(ApplyError::Unavailable)
        ));
    }
    #[test]
    fn preserve_ignored_text_and_explicit_copy_policy() {
        let mut l = doc(" Foo old \n");
        let ls = l.snapshot();
        let rs = doc("foo new\n").snapshot();
        let options = CompareOptions {
            whitespace: Whitespace::TrimEdges,
            ignore_case: true,
            ..Default::default()
        };
        let result = compare(&ls, &rs, &options, &CancelToken::default());
        l.apply(apply_hunk(Direction::RightToLeft, &result.hunks[0], &ls, &rs, 1000).unwrap())
            .unwrap();
        assert_eq!(text(&l), " Foo new \n");
        l.undo().unwrap();
        assert_eq!(text(&l), " Foo old \n");
        let current = l.snapshot();
        let result = compare(&current, &rs, &options, &CancelToken::default());
        l.apply(
            apply_hunk_with_policy(
                MergePolicy::CopySelectedRange,
                Direction::RightToLeft,
                &result.hunks[0],
                &current,
                &rs,
                1000,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(text(&l), "foo new\n");
        let mut target = doc("a\tb");
        let ls = target.snapshot();
        let rs = doc("a   c").snapshot();
        let options = CompareOptions {
            normalize_tabs: true,
            ..Default::default()
        };
        let result = compare(&ls, &rs, &options, &CancelToken::default());
        target
            .apply(apply_hunk(Direction::RightToLeft, &result.hunks[0], &ls, &rs, 1000).unwrap())
            .unwrap();
        assert_eq!(text(&target), "a\tc");
    }
    #[test]
    fn whitespace_and_eol_options_are_independent() {
        let l = doc(" a \r\n").snapshot();
        let r = doc("a\n").snapshot();
        let options = CompareOptions {
            whitespace: Whitespace::TrimEdges,
            ..Default::default()
        };
        assert!(
            !compare(&l, &r, &options, &CancelToken::default())
                .hunks
                .is_empty()
        );
        let options = CompareOptions {
            ignore_eol_style: true,
            ..options
        };
        assert!(
            compare(&l, &r, &options, &CancelToken::default())
                .hunks
                .is_empty()
        );
    }
    #[test]
    fn ignore_eol_preserves_line_structure_and_rejects_bad_ranges() {
        let mut target = doc("ab\n");
        let source = doc("a\nb\n").snapshot();
        let ts = target.snapshot();
        let o = CompareOptions {
            ignore_eol_style: true,
            ..Default::default()
        };
        let result = compare(&source, &ts, &o, &CancelToken::default());
        let h = &result.hunks[0];
        target
            .apply(apply_hunk(Direction::LeftToRight, h, &source, &ts, 1000).unwrap())
            .unwrap();
        assert_eq!(text(&target), "a\nb\n");
        let mut bad = h.clone();
        bad.left = TextOffset(2)..TextOffset(1);
        assert!(matches!(
            apply_hunk(Direction::LeftToRight, &bad, &source, &ts, 1000),
            Err(ApplyError::InvalidRange)
        ));
    }
    #[test]
    fn incremental_batches_are_bounded_and_reproduce_input() {
        let left = "a\n".repeat(100);
        let right = "b\n".repeat(100);
        let mut target = doc(&left);
        let ls = target.snapshot();
        let rs = doc(&right).snapshot();
        let mut options = CompareOptions::default();
        options.limits.max_lines_exact = 4;
        let mut edits = Vec::new();
        let mut batches = 0;
        let result = compare_batches(&ls, &rs, &options, &CancelToken::default(), 1, |batch| {
            batches += 1;
            assert!(batch.len() <= 1);
            for h in batch {
                edits.extend(
                    apply_hunk(Direction::RightToLeft, h, &ls, &rs, 1000)
                        .unwrap()
                        .edits,
                );
            }
            true
        });
        assert!(batches > 1);
        assert!(matches!(
            result.completeness,
            CompareCompleteness::Coarse(_)
        ));
        assert!(result.hunks.is_empty());
        target
            .apply(EditTransaction {
                base_revision: ls.revision,
                edits,
            })
            .unwrap();
        assert_eq!(text(&target), right);
        let cancel = CancelToken::default();
        let result = compare_batches(&ls, &rs, &options, &cancel, 1, |_| {
            cancel.cancel();
            true
        });
        assert_eq!(result.completeness, CompareCompleteness::Cancelled);
    }
    #[test]
    fn full_unicode_case_folding_is_locale_independent() {
        let options = CompareOptions {
            ignore_case: true,
            ..Default::default()
        };
        for (left, right) in [
            ("Stra\u{df}e", "STRASSE"),
            ("\u{3a3}\u{3c2}\u{3c3}", "\u{3c3}\u{3c3}\u{3c3}"),
            ("\u{fb03}", "ffi"),
        ] {
            let result = compare(
                &doc(left).snapshot(),
                &doc(right).snapshot(),
                &options,
                &CancelToken::default(),
            );
            assert_eq!(result.completeness, CompareCompleteness::Exact);
            assert!(result.hunks.is_empty());
        }
    }
    #[test]
    fn preserve_full_case_expansions_keeps_original_coordinates() {
        let options = CompareOptions {
            ignore_case: true,
            ..Default::default()
        };
        for (left, right, expected) in [
            ("a\u{df}z old", "ASSZ new", "a\u{df}z new"),
            ("\u{df}", "st", "st"),
            ("st", "\u{df}", "\u{df}"),
            ("a\u{fb03} old", "AFFI new", "a\u{fb03} new"),
        ] {
            let mut target = doc(left);
            let ls = target.snapshot();
            let rs = doc(right).snapshot();
            let result = compare(&ls, &rs, &options, &CancelToken::default());
            let edits = result
                .hunks
                .iter()
                .flat_map(|h| {
                    apply_hunk(Direction::RightToLeft, h, &ls, &rs, 1000)
                        .unwrap()
                        .edits
                })
                .collect();
            target
                .apply(EditTransaction {
                    base_revision: ls.revision,
                    edits,
                })
                .unwrap();
            assert_eq!(text(&target), expected);
            target.undo().unwrap();
            assert_eq!(text(&target), left);
        }
    }
    #[test]
    fn generated_fold_mapping_round_trips_preserve_normalized_equality() {
        let alphabet = ["s", "\u{df}", "t", "S", "\u{fb03}", "f", "i"];
        let mut inputs = vec![String::new()];
        for a in alphabet {
            inputs.push(a.into());
            for b in alphabet {
                inputs.push(format!("{a}{b}"));
            }
        }
        let options = CompareOptions {
            ignore_case: true,
            ..Default::default()
        };
        for left in &inputs {
            for right in &inputs {
                let mut target = doc(left);
                let ls = target.snapshot();
                let rs = doc(right).snapshot();
                let result = compare(&ls, &rs, &options, &CancelToken::default());
                let edits = result
                    .hunks
                    .iter()
                    .flat_map(|h| {
                        apply_hunk(Direction::RightToLeft, h, &ls, &rs, 1000)
                            .unwrap()
                            .edits
                    })
                    .collect();
                target
                    .apply(EditTransaction {
                        base_revision: ls.revision,
                        edits,
                    })
                    .unwrap_or_else(|error| panic!("{left:?} -> {right:?}: {error:?}"));
                assert_eq!(
                    casefold::fold(&text(&target)),
                    casefold::fold(right),
                    "{left:?} -> {right:?}"
                );
            }
        }
    }
    #[test]
    fn repeated_context_hunks_have_distinct_navigation_ids() {
        let l = doc("x\na\ny\nx\nb\ny\n").snapshot();
        let r = doc("x\nA\ny\nx\nB\ny\n").snapshot();
        let result = compare(&l, &r, &CompareOptions::default(), &CancelToken::default());
        assert_eq!(result.hunks.len(), 2);
        assert_ne!(result.hunks[0].stable_id, result.hunks[1].stable_id);
        let newer = compare(
            &doc("x\nz\ny\nx\na\ny\nx\nb\ny\n").snapshot(),
            &doc("x\nZ\ny\nx\nA\ny\nx\nB\ny\n").snapshot(),
            &CompareOptions::default(),
            &CancelToken::default(),
        );
        assert_eq!(result.hunks[0].stable_id, newer.hunks[1].stable_id);
        assert_eq!(result.hunks[1].stable_id, newer.hunks[2].stable_id);
    }

    #[test]
    fn forced_normalized_hash_collisions_retain_real_differences() {
        let options = CompareOptions {
            whitespace: Whitespace::TrimEdges,
            ignore_case: true,
            ..Default::default()
        };
        let left = doc(" ANCHOR \nStraße old\n tail \n").snapshot();
        let right = doc("anchor\nSTRASSE new\nTAIL\n").snapshot();
        let result = compare_hashed(&left, &right, &options, &CancelToken::default(), |_| 0);
        assert_eq!(result.completeness, CompareCompleteness::Exact);
        assert_eq!(result.hunks.len(), 1);
        assert_eq!(result.hunks[0].left, TextOffset(9)..TextOffset(21));
        let tx = apply_hunk(
            Direction::RightToLeft,
            &result.hunks[0],
            &left,
            &right,
            4096,
        )
        .unwrap();
        assert_eq!(
            tx.edits
                .iter()
                .map(|e| e.insert.as_str())
                .collect::<String>(),
            "new"
        );
    }
}
