// SPDX-License-Identifier: MPL-2.0
//! Invocation-scoped editor broker. Guest commits remain proposals until the child
//! exits successfully; the editor actor checks captured identity/revision again.
use bareline_document::{DocumentSnapshot, Edit, EditTransaction, TextOffset};
use bareline_extensions_protocol::{
    broker::{BrokerError, ExtensionSession},
    *,
};
use std::{collections::BTreeSet, time::Instant};

pub struct InvocationOutput {
    pub source: DocumentSnapshot,
    pub transaction: Option<EditTransaction>,
    pub panels: Vec<(String, String)>,
}
pub struct InvocationBroker {
    invocation: Invocation,
    source: DocumentSnapshot,
    session: ExtensionSession,
    allowed_panels: BTreeSet<String>,
    seen: BTreeSet<u64>,
    edits: Option<Vec<TextEdit>>,
    panels: Vec<(String, String)>,
    panel_bytes: usize,
    external_text: bool,
}
impl InvocationBroker {
    pub fn new(
        invocation: Invocation,
        source: DocumentSnapshot,
        session: ExtensionSession,
        panels: Vec<String>,
    ) -> Result<Self, String> {
        if invocation.revision != source.revision.0
            || invocation.text_length != source.len() as u64
            || invocation.grant_generation != session.generation()
            || !source.is_complete()
        {
            return Err("Invocation snapshot or grant changed".into());
        }
        Ok(Self {
            invocation,
            source,
            session,
            allowed_panels: panels.into_iter().collect(),
            seen: BTreeSet::new(),
            edits: None,
            panels: vec![],
            panel_bytes: 0,
            external_text: false,
        })
    }
    /// Read-only paged authority is captured separately from the viewport adapter.
    pub fn new_paged(invocation: Invocation, source: DocumentSnapshot, session: ExtensionSession, panels: Vec<String>) -> Result<Self, String> {
        if invocation.grant_generation != session.generation() { return Err("Invocation grant changed".into()); }
        Ok(Self { invocation, source, session, allowed_panels: panels.into_iter().collect(), seen: BTreeSet::new(), edits: None, panels: vec![], panel_bytes: 0, external_text: true })
    }
    pub fn request(
        &mut self,
        message: Envelope,
        original: impl FnMut(u64, RawRange) -> Result<Vec<u8>, String>,
    ) -> BrokerResponse {
        self.request_with_text(message, original, |_| Err("External text reader unavailable".into()))
    }
    pub fn request_with_text(
        &mut self,
        message: Envelope,
        mut original: impl FnMut(u64, RawRange) -> Result<Vec<u8>, String>,
        mut text: impl FnMut(TextRange) -> Result<Vec<u8>, String>,
    ) -> BrokerResponse {
        let request_id = message.request_id;
        let result = (|| {
            self.session
                .authorize(&message)
                .map_err(|e| format!("{e:?}"))?;
            if self.seen.len() >= 65536 || !self.seen.insert(request_id) {
                return Err("Request replay or invocation request limit".into());
            }
            let now = Instant::now();
            match &message.request {
                Request::ReadTextRange {
                    document,
                    revision,
                    range,
                } => {
                    if *document != self.invocation.document
                        || *revision != self.invocation.revision
                    {
                        return Err("Stale text snapshot".into());
                    }
                    if range.start > range.end || range.end > self.invocation.text_length || range.end - range.start > (MAX_CHUNK_BYTES - 128) as u64 { return Err("Text range limit".into()); }
                    let bytes = if self.external_text { text(range.clone())? } else { read_bytes(&self.source, range)? };
                    if bytes.len() as u64 != range.end - range.start { return Err("Text range unavailable".into()); }
                    Ok(BrokerValue::Bytes(bytes))
                }
                Request::ReadOriginalBytes {
                    document,
                    generation,
                    range,
                } => {
                    if *document != self.invocation.document
                        || *generation != self.invocation.source_generation
                        || range.start > range.end
                        || range.end > self.invocation.raw_length
                        || range.end - range.start > (MAX_CHUNK_BYTES - 128) as u64
                    {
                        return Err("Stale or invalid original range".into());
                    }
                    let bytes = original(*generation, range.clone())?;
                    if bytes.len() as u64 != range.end - range.start {
                        return Err("Original range unavailable".into());
                    }
                    Ok(BrokerValue::Bytes(bytes))
                }
                Request::BeginEdits { document, .. } => {
                    if self.external_text { return Err("Paged invocation is read-only".into()); }
                    if *document != self.invocation.document || self.edits.is_some() {
                        return Err("One document transaction per invocation".into());
                    }
                    self.session
                        .begin_edits(&message, self.invocation.revision, now)
                        .map_err(|e| format!("{e:?}"))?;
                    Ok(BrokerValue::Transaction(request_id))
                }
                Request::AppendChunk { .. } => {
                    self.session
                        .append(&message, now)
                        .map_err(|e| format!("{e:?}"))?;
                    Ok(BrokerValue::Acknowledged)
                }
                Request::CommitEdits { .. } => {
                    if self.edits.is_some() {
                        return Err("One document transaction per invocation".into());
                    }
                    let source = &self.source;
                    self.edits = Some(
                        self.session
                            .commit_checked(&message, self.invocation.revision, now, |edits| {
                                validate(source, edits)
                            })
                            .map_err(|e| format!("{e:?}"))?,
                    );
                    Ok(BrokerValue::Acknowledged)
                }
                Request::ApplyEdits {
                    document,
                    revision,
                    edits,
                } => {
                    if self.external_text { return Err("Paged invocation is read-only".into()); }
                    if *document != self.invocation.document
                        || *revision != self.invocation.revision
                        || self.edits.is_some()
                    {
                        return Err("Stale or duplicate transaction".into());
                    }
                    validate(&self.source, edits).map_err(|e| format!("{e:?}"))?;
                    self.edits = Some(edits.clone());
                    Ok(BrokerValue::Acknowledged)
                }
                Request::Panel { panel, text } => {
                    if !self.allowed_panels.contains(panel)
                        || self.panels.len() >= 16
                        || self.panel_bytes.saturating_add(text.len()) > MAX_CHUNK_BYTES
                    {
                        return Err("Undeclared panel or panel output limit".into());
                    }
                    self.panel_bytes += text.len();
                    self.panels.push((panel.clone(), text.clone()));
                    Ok(BrokerValue::Acknowledged)
                }
                Request::Cancel { request } => {
                    self.session.cancel(*request);
                    self.edits = None;
                    Ok(BrokerValue::Acknowledged)
                }
                _ => Err("API unavailable in this invocation".into()),
            }
        })();
        BrokerResponse { request_id, result }
    }
    /// Call only with the real authenticated child outcome. Failure/cancel drops
    /// proposals and panels; no partial mutation crosses back to the UI.
    pub fn finish(mut self, child: Result<(), String>) -> Result<InvocationOutput, String> {
        self.session.revoke();
        child?;
        let transaction = self.edits.map(|edits| EditTransaction {
            base_revision: self.source.revision,
            edits: edits
                .into_iter()
                .map(|e| Edit {
                    range: TextOffset(e.range.start as usize)..TextOffset(e.range.end as usize),
                    insert: e.replacement,
                })
                .collect(),
        });
        Ok(InvocationOutput {
            source: self.source,
            transaction,
            panels: self.panels,
        })
    }
}
fn validate(source: &DocumentSnapshot, edits: &[TextEdit]) -> Result<(), BrokerError> {
    if edits.len() > 4096 {
        return Err(BrokerError::InvalidEdits);
    }
    let mut previous = 0;
    for edit in edits {
        let start = usize::try_from(edit.range.start).map_err(|_| BrokerError::InvalidEdits)?;
        let end = usize::try_from(edit.range.end).map_err(|_| BrokerError::InvalidEdits)?;
        if start < previous
            || start > end
            || !source.is_boundary(TextOffset(start))
            || !source.is_boundary(TextOffset(end))
        {
            return Err(BrokerError::InvalidEdits);
        }
        previous = end;
    }
    Ok(())
}
fn read_bytes(source: &DocumentSnapshot, range: &TextRange) -> Result<Vec<u8>, String> {
    let start = usize::try_from(range.start).map_err(|_| "TextOffset overflow")?;
    let end = usize::try_from(range.end).map_err(|_| "TextOffset overflow")?;
    if start > end || end > source.len() || end - start > MAX_CHUNK_BYTES - 128 {
        return Err("Text range limit".into());
    }
    // Text range transport streams bytes, so a chunk may bisect a UTF-8 scalar.
    let mut left = start;
    let mut right = end;
    while !source.is_boundary(TextOffset(left)) {
        left -= 1;
    }
    while !source.is_boundary(TextOffset(right)) {
        right += 1;
    }
    let text = source
        .read(TextOffset(left)..TextOffset(right), MAX_CHUNK_BYTES)
        .map_err(|e| format!("{e:?}"))?;
    Ok(text.as_bytes()[start - left..end - left].to_vec())
}
#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Document};
    #[test]
    fn byte_chunks_cross_unicode_and_failed_child_discards_edits() {
        let document =
            Document::from_utf8("a🙂z", Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
        let source = document.snapshot();
        assert_eq!(
            read_bytes(&source, &TextRange { start: 2, end: 4 }).unwrap(),
            "🙂".as_bytes()[1..3]
        );
        let mut session = ExtensionSession::new("fixture".into()).unwrap();
        session.approve(vec![broker::Grant {
            capability: Capability::DocumentEdit,
            scope: Scope::Document(1),
        }]);
        let invocation = Invocation {
            extension_id: "fixture".into(),
            command: "fixture.run".into(),
            arguments: String::new(),
            document: 1,
            revision: source.revision.0,
            source_generation: 1,
            text_length: source.len() as u64,
            raw_length: 0,
            grant_generation: session.generation(),
        };
        let generation = session.generation();
        let revision = invocation.revision;
        let mut broker = InvocationBroker::new(invocation, source, session, vec![]).unwrap();
        let response = broker.request(
            Envelope {
                protocol: 1,
                request_id: 1,
                extension_id: "fixture".into(),
                context: CapabilityContext {
                    capability: Capability::DocumentEdit,
                    scope: Scope::Document(1),
                    grant_generation: generation,
                },
                request: Request::ApplyEdits {
                    document: 1,
                    revision,
                    edits: vec![TextEdit {
                        range: TextRange { start: 0, end: 1 },
                        replacement: "b".into(),
                    }],
                },
            },
            |_, _| unreachable!(),
        );
        assert!(response.result.is_ok());
        assert!(broker.finish(Err("host crash".into())).is_err());
        assert_eq!(
            document
                .snapshot()
                .read(TextOffset(0)..TextOffset(6), 6)
                .unwrap(),
            "a🙂z"
        );
    }
}

pub mod manager;
