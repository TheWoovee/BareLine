// SPDX-License-Identifier: MPL-2.0
use crate::source::MemorySource;
use crate::{Budget, Error, Reservation};
use std::{ops::Range, sync::Arc};

const CHUNK: usize = 64 * 1024;
pub(crate) type Root = Option<Arc<Node>>;
pub(crate) struct Segment {
    pub(crate) text: Box<str>,
    _reservation: Reservation,
    origin: Option<(MemorySource, Range<u64>)>,
}
#[derive(Clone)]
pub(crate) struct Piece {
    pub(crate) segment: Arc<Segment>,
    pub(crate) range: Range<usize>,
    pub(crate) summary: Summary,
}
impl Piece {
    pub(crate) fn origin(&self) -> Option<(&MemorySource, Range<u64>)> {
        self.segment.origin.as_ref().map(|(source, range)| {
            (
                source,
                range.start + self.range.start as u64..range.start + self.range.end as u64,
            )
        })
    }
    fn new(segment: Arc<Segment>, range: Range<usize>) -> Self {
        let summary = Summary::scan(&segment.text[range.clone()]);
        Self {
            segment,
            range,
            summary,
        }
    }
    pub(crate) fn text(&self) -> &str {
        &self.segment.text[self.range.clone()]
    }
}
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct Summary {
    pub unknown: bool,
    pub bytes: usize,
    pub breaks: usize,
    pub cr: usize,
    pub lf: usize,
    pub crlf: usize,
    first: Option<u8>,
    last: Option<u8>,
}
impl Summary {
    fn scan(text: &str) -> Self {
        let mut breaks = 0;
        let mut previous = None;
        let mut cr = 0;
        let mut lf = 0;
        let mut crlf = 0;
        for byte in text.bytes() {
            if byte == b'\r' {
                cr += 1;
            } else if byte == b'\n' {
                if previous == Some(b'\r') {
                    cr -= 1;
                    crlf += 1;
                } else {
                    lf += 1;
                }
            }
            if byte == b'\r' || (byte == b'\n' && previous != Some(b'\r')) {
                breaks += 1;
            }
            previous = Some(byte);
        }
        Self {
            unknown: false,
            bytes: text.len(),
            breaks,
            cr,
            lf,
            crlf,
            first: text.as_bytes().first().copied(),
            last: previous,
        }
    }
    fn combine(self, rhs: Self) -> Self {
        if self.unknown || rhs.unknown {
            return Self {
                bytes: self.bytes + rhs.bytes,
                unknown: true,
                ..Self::default()
            };
        }
        let cross = usize::from(self.last == Some(b'\r') && rhs.first == Some(b'\n'));
        Self {
            unknown: false,
            bytes: self.bytes + rhs.bytes,
            breaks: self.breaks + rhs.breaks
                - usize::from(self.last == Some(b'\r') && rhs.first == Some(b'\n')),
            cr: self.cr + rhs.cr - cross,
            lf: self.lf + rhs.lf - cross,
            crlf: self.crlf + rhs.crlf + cross,
            first: self.first.or(rhs.first),
            last: rhs.last.or(self.last),
        }
    }
}
pub(crate) enum Node {
    Leaf(Piece),
    Source {
        source: MemorySource,
        range: Range<u64>,
    },
    OwnedSource {
        source: MemorySource,
        range: Range<u64>,
        original: Option<(MemorySource, Range<u64>)>,
        summary: Summary,
    },
    Branch {
        left: Arc<Node>,
        right: Arc<Node>,
        height: u16,
        summary: Summary,
    },
}
impl Node {
    fn height(&self) -> u16 {
        match self {
            Self::Leaf(_) | Self::Source { .. } | Self::OwnedSource { .. } => 1,
            Self::Branch { height, .. } => *height,
        }
    }
    pub fn summary(&self) -> Summary {
        match self {
            Self::Leaf(piece) => piece.summary,
            Self::OwnedSource { summary, .. } => *summary,
            Self::Source { range, .. } => Summary {
                bytes: (range.end - range.start) as usize,
                unknown: true,
                ..Summary::default()
            },
            Self::Branch { summary, .. } => *summary,
        }
    }
}
pub(crate) fn summary(root: &Root) -> Summary {
    root.as_ref().map_or(Summary::default(), |n| n.summary())
}
fn branch(left: Arc<Node>, right: Arc<Node>) -> Arc<Node> {
    let summary = left.summary().combine(right.summary());
    let height = 1 + left.height().max(right.height());
    Arc::new(Node::Branch {
        left,
        right,
        height,
        summary,
    })
}
fn balance(left: Arc<Node>, right: Arc<Node>) -> Arc<Node> {
    if left.height() > right.height() + 1 {
        let Node::Branch {
            left: a, right: b, ..
        } = left.as_ref()
        else {
            unreachable!()
        };
        if a.height() >= b.height() {
            return branch(a.clone(), branch(b.clone(), right));
        }
        let Node::Branch {
            left: c, right: d, ..
        } = b.as_ref()
        else {
            unreachable!()
        };
        return branch(branch(a.clone(), c.clone()), branch(d.clone(), right));
    }
    if right.height() > left.height() + 1 {
        let Node::Branch {
            left: a, right: b, ..
        } = right.as_ref()
        else {
            unreachable!()
        };
        if b.height() >= a.height() {
            return branch(branch(left, a.clone()), b.clone());
        }
        let Node::Branch {
            left: c, right: d, ..
        } = a.as_ref()
        else {
            unreachable!()
        };
        return branch(branch(left, c.clone()), branch(d.clone(), b.clone()));
    }
    branch(left, right)
}
pub(crate) fn concat(left: Root, right: Root) -> Root {
    match (left, right) {
        (None, r) => r,
        (l, None) => l,
        (Some(l), Some(r)) => {
            if l.height() > r.height() + 1 {
                let Node::Branch {
                    left: a, right: b, ..
                } = l.as_ref()
                else {
                    unreachable!()
                };
                return Some(balance(
                    a.clone(),
                    concat(Some(b.clone()), Some(r)).unwrap(),
                ));
            }
            if r.height() > l.height() + 1 {
                let Node::Branch {
                    left: a, right: b, ..
                } = r.as_ref()
                else {
                    unreachable!()
                };
                return Some(balance(
                    concat(Some(l), Some(a.clone())).unwrap(),
                    b.clone(),
                ));
            }
            Some(branch(l, r))
        }
    }
}
pub(crate) fn split(root: Root, offset: usize) -> (Root, Root) {
    let Some(node) = root else {
        return (None, None);
    };
    if offset == 0 {
        return (None, Some(node));
    }
    if offset == node.summary().bytes {
        return (Some(node), None);
    }
    match node.as_ref() {
        Node::OwnedSource {
            source,
            range,
            original,
            ..
        } => {
            let middle = range.start + offset as u64;
            let left_origin = original
                .as_ref()
                .map(|(s, r)| (s.clone(), r.start..r.start + offset as u64));
            let right_origin = original
                .as_ref()
                .map(|(s, r)| (s.clone(), r.start + offset as u64..r.end));
            (
                from_owned_source(source.clone(), range.start..middle, left_origin),
                from_owned_source(source.clone(), middle..range.end, right_origin),
            )
        }
        Node::Source { source, range } => {
            let middle = range.start + offset as u64;
            (
                from_source(source.clone(), range.start..middle),
                from_source(source.clone(), middle..range.end),
            )
        }
        Node::Leaf(piece) => {
            let middle = piece.range.start + offset;
            (
                Some(Arc::new(Node::Leaf(Piece::new(
                    piece.segment.clone(),
                    piece.range.start..middle,
                )))),
                Some(Arc::new(Node::Leaf(Piece::new(
                    piece.segment.clone(),
                    middle..piece.range.end,
                )))),
            )
        }
        Node::Branch { left, right, .. } => {
            let size = left.summary().bytes;
            if offset < size {
                let (a, b) = split(Some(left.clone()), offset);
                (a, concat(b, Some(right.clone())))
            } else {
                let (a, b) = split(Some(right.clone()), offset - size);
                (concat(Some(left.clone()), a), b)
            }
        }
    }
}
pub(crate) fn from_text(text: &str, budget: &Budget) -> Result<Root, Error> {
    let mut root = None;
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + CHUNK).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let reservation = budget.reserve(end - start)?;
        let segment = Arc::new(Segment {
            text: text[start..end].into(),
            _reservation: reservation,
            origin: None,
        });
        root = concat(
            root,
            Some(Arc::new(Node::Leaf(Piece::new(segment, 0..end - start)))),
        );
        start = end;
    }
    Ok(root)
}
pub(crate) fn own_inverse(
    root: &Root,
    range: Range<usize>,
    text: &str,
    budget: &Budget,
) -> Result<Root, Error> {
    let mut output = None;
    let mut cursor = range.start;
    while cursor < range.end {
        let (count, origin) = match span_at(root, cursor).ok_or(Error::OutOfBounds)? {
            Span::Owned(bytes) => (bytes.len().min(range.end - cursor), None),
            Span::OwnedSource(_, source_range) => (
                (source_range.end - source_range.start).min((range.end - cursor) as u64) as usize,
                None,
            ),
            Span::Source(source, source_range) => {
                let count = (source_range.end - source_range.start).min((range.end - cursor) as u64)
                    as usize;
                (
                    count,
                    Some((
                        source.clone(),
                        source_range.start..source_range.start + count as u64,
                    )),
                )
            }
        };
        let local = cursor - range.start;
        let mut count = count.min(CHUNK);
        while !text.is_char_boundary(local + count) {
            count -= 1;
        }
        let owned = if let Some(origin) = origin {
            let origin = (origin.0, origin.1.start..origin.1.start + count as u64);
            let reservation = budget.reserve(count)?;
            let segment = Arc::new(Segment {
                text: text[local..local + count].into(),
                _reservation: reservation,
                origin: Some(origin),
            });
            Some(Arc::new(Node::Leaf(Piece::new(segment, 0..count))))
        } else {
            // Preserve any existing owned provenance by reusing the immutable subroot.
            let (prefix, _) = split(root.clone(), cursor + count);
            split(prefix, cursor).1
        };
        output = concat(output, owned);
        cursor += count;
    }
    Ok(output)
}
pub(crate) fn from_source(source: MemorySource, range: Range<u64>) -> Root {
    (!range.is_empty()).then(|| Arc::new(Node::Source { source, range }))
}
pub(crate) fn from_owned_source(
    source: MemorySource,
    range: Range<u64>,
    original: Option<(MemorySource, Range<u64>)>,
) -> Root {
    (!range.is_empty()).then(|| {
        Arc::new(Node::OwnedSource {
            source,
            summary: Summary {
                bytes: (range.end - range.start) as usize,
                unknown: true,
                ..Summary::default()
            },
            range,
            original,
        })
    })
}
pub(crate) enum Span<'a> {
    OwnedSource(&'a MemorySource, Range<u64>),
    Owned(&'a [u8]),
    Source(&'a MemorySource, Range<u64>),
}
/// The suffix of the leaf containing offset, without forcing source availability.
pub(crate) fn span_at(root: &Root, mut offset: usize) -> Option<Span<'_>> {
    let mut node = root.as_deref()?;
    loop {
        match node {
            Node::Leaf(piece) => return Some(Span::Owned(&piece.text().as_bytes()[offset..])),
            Node::OwnedSource { source, range, .. } => {
                return Some(Span::OwnedSource(
                    source,
                    range.start + offset as u64..range.end,
                ));
            }
            Node::Source { source, range } => {
                return Some(Span::Source(source, range.start + offset as u64..range.end));
            }
            Node::Branch { left, right, .. } => {
                if offset < left.summary().bytes {
                    node = left;
                } else {
                    offset -= left.summary().bytes;
                    node = right;
                }
            }
        }
    }
}
pub(crate) fn byte_at(root: &Root, mut offset: usize) -> Option<u8> {
    let mut node = root.as_deref()?;
    loop {
        match node {
            Node::Source { .. } | Node::OwnedSource { .. } => return None,
            Node::Leaf(piece) => return piece.text().as_bytes().get(offset).copied(),
            Node::Branch { left, right, .. } => {
                if offset < left.summary().bytes {
                    node = left;
                } else {
                    offset -= left.summary().bytes;
                    node = right;
                }
            }
        }
    }
}
pub(crate) fn boundary(root: &Root, offset: usize) -> bool {
    offset == summary(root).bytes || byte_at(root, offset).is_some_and(|b| b & 0xc0 != 0x80)
}
pub(crate) fn line_start(root: &Root, line: usize) -> Option<usize> {
    if line == 0 {
        return Some(0);
    }
    let node = root.as_ref()?;
    if line > node.summary().breaks {
        return None;
    }
    fn find(node: &Node, mut ordinal: usize, preceding_cr: bool) -> usize {
        match node {
            Node::Source { .. } | Node::OwnedSource { .. } => {
                unreachable!("source roots use Pending-aware paged APIs")
            }
            Node::Leaf(piece) => {
                let mut previous_cr = preceding_cr;
                for (i, byte) in piece.text().bytes().enumerate() {
                    if byte == b'\r' || (byte == b'\n' && !previous_cr) {
                        ordinal -= 1;
                        if ordinal == 0 {
                            return i;
                        }
                    }
                    previous_cr = byte == b'\r';
                }
                unreachable!("validated newline ordinal")
            }
            Node::Branch { left, right, .. } => {
                let left_summary = left.summary();
                let count = left_summary.breaks
                    - usize::from(preceding_cr && left_summary.first == Some(b'\n'));
                if ordinal <= count {
                    find(left, ordinal, preceding_cr)
                } else {
                    left_summary.bytes
                        + find(right, ordinal - count, left_summary.last == Some(b'\r'))
                }
            }
        }
    }
    let separator = find(node, line, false);
    Some(
        separator
            + if byte_at(root, separator) == Some(b'\r')
                && byte_at(root, separator + 1) == Some(b'\n')
            {
                2
            } else {
                1
            },
    )
}
pub(crate) fn line_at(root: &Root, offset: usize) -> usize {
    fn prefix(node: &Node, offset: usize) -> Summary {
        if offset == node.summary().bytes {
            return node.summary();
        }
        match node {
            Node::Source { .. } | Node::OwnedSource { .. } => {
                unreachable!("source roots use Pending-aware paged APIs")
            }
            Node::Leaf(piece) => Summary::scan(&piece.text()[..offset]),
            Node::Branch { left, right, .. } => {
                let middle = left.summary().bytes;
                if offset <= middle {
                    prefix(left, offset)
                } else {
                    left.summary().combine(prefix(right, offset - middle))
                }
            }
        }
    }
    let count = root.as_ref().map_or(0, |n| prefix(n, offset).breaks);
    count
        - usize::from(
            offset > 0
                && byte_at(root, offset - 1) == Some(b'\r')
                && byte_at(root, offset) == Some(b'\n'),
        )
}
/// A range iterator with O(tree height) scratch storage, never one entry per document piece.
pub struct Chunks<'a> {
    stack: Vec<(&'a Node, Range<usize>)>,
}
pub(crate) fn chunks(root: &Root, range: Range<usize>) -> Chunks<'_> {
    let mut stack = Vec::new();
    if !range.is_empty()
        && let Some(node) = root
    {
        stack.push((node.as_ref(), range));
    }
    Chunks { stack }
}
impl<'a> Iterator for Chunks<'a> {
    type Item = &'a str;
    fn next(&mut self) -> Option<Self::Item> {
        while let Some((node, range)) = self.stack.pop() {
            match node {
                Node::Source { .. } | Node::OwnedSource { .. } => return None,
                Node::Leaf(piece) => return Some(&piece.text()[range]),
                Node::Branch { left, right, .. } => {
                    let middle = left.summary().bytes;
                    if range.end > middle {
                        self.stack.push((
                            right,
                            range.start.saturating_sub(middle)..range.end - middle,
                        ));
                    }
                    if range.start < middle {
                        self.stack.push((left, range.start..range.end.min(middle)));
                    }
                }
            }
        }
        None
    }
}
#[cfg(test)]
pub(crate) fn assert_balanced(root: &Root) {
    fn check(node: &Node) {
        if let Node::Branch {
            left,
            right,
            height,
            summary,
        } = node
        {
            assert!(left.height().abs_diff(right.height()) <= 1);
            assert_eq!(*height, 1 + left.height().max(right.height()));
            assert_eq!(summary.bytes, left.summary().bytes + right.summary().bytes);
            check(left);
            check(right);
        }
    }
    if let Some(node) = root {
        check(node);
    }
}

pub(crate) fn has_source(root: &Root) -> bool {
    let mut stack: Vec<&Node> = root.iter().map(|node| node.as_ref()).collect();
    while let Some(node) = stack.pop() {
        match node {
            Node::Source { .. } | Node::OwnedSource { .. } => return true,
            Node::Leaf(_) => {}
            Node::Branch { left, right, .. } => {
                stack.push(left);
                stack.push(right);
            }
        }
    }
    false
}
