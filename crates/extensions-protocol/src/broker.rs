// SPDX-License-Identifier: MIT OR Apache-2.0
//! Per-extension scoped grants. Revocation drops all queued work and staged data.
use crate::*;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};
#[derive(Debug, PartialEq, Eq)]
pub enum BrokerError {
    Denied,
    InvalidScope,
    PendingLimit,
    DuplicateRequest,
    UnknownRequest,
    Expired,
    StaleRevision,
    ChunkLimit,
    StagingLimit,
    InvalidEdits,
    ApprovalRequired,
}
#[derive(Debug, Clone)]
pub struct Grant {
    pub capability: Capability,
    pub scope: Scope,
}
struct Pending {
    deadline: Instant,
}
struct Staged {
    document: u64,
    revision: u64,
    bytes: Vec<u8>,
}
pub struct ExtensionSession {
    id: String,
    generation: u64,
    grants: Vec<Grant>,
    pending: BTreeMap<u64, Pending>,
    staged: BTreeMap<u64, Staged>,
    enabled: bool,
    budget: ExecutionBudget,
}
impl ExtensionSession {
    pub fn new(id: String) -> Result<Self, BrokerError> {
        Self::new_with_budget(id, ExecutionBudget::Interactive)
    }
    /// The invocation's declared budget decides how long staged work may stay
    /// pending; background commands stream for up to two minutes, not five seconds.
    pub fn new_with_budget(id: String, budget: ExecutionBudget) -> Result<Self, BrokerError> {
        if !valid_id(&id) {
            return Err(BrokerError::Denied);
        }
        Ok(Self {
            id,
            generation: 0,
            grants: vec![],
            pending: BTreeMap::new(),
            staged: BTreeMap::new(),
            enabled: false,
            budget,
        })
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// Caller invokes only after explicit permission approval; catalog data never grants.
    pub fn approve(&mut self, grants: Vec<Grant>) {
        self.revoke();
        self.grants = grants;
        self.enabled = true;
    }
    pub fn revoke(&mut self) {
        self.generation = self.generation.saturating_add(1);
        self.grants.clear();
        self.pending.clear();
        self.staged.clear();
        self.enabled = false;
    }
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
    pub fn authorize(&self, message: &Envelope) -> Result<(), BrokerError> {
        if !self.enabled
            || message.extension_id != self.id
            || message.protocol != PROTOCOL_VERSION
            || message.context.grant_generation != self.generation
        {
            return Err(BrokerError::Denied);
        }
        let expected = match &message.request {
            Request::ReadTextRange { document, .. } | Request::ReadOriginalBytes { document, .. } => {
                (Capability::DocumentRead, Scope::Document(*document))
            }
            Request::ApplyEdits { document, .. } | Request::BeginEdits { document, .. } => {
                (Capability::DocumentEdit, Scope::Document(*document))
            }
            Request::AppendChunk { transaction, .. } | Request::CommitEdits { transaction, .. } => (
                Capability::DocumentEdit,
                Scope::Document(
                    self.staged
                        .get(transaction)
                        .ok_or(BrokerError::UnknownRequest)?
                        .document,
                ),
            ),
            Request::Panel { .. } => (Capability::UiPanel, Scope::Extension),
            // Commands are resolved by the editor registry, with their own required grant.
            Request::InvokeCommand { .. } => return Err(BrokerError::Denied),
            Request::Cancel { request } => {
                if self.pending.contains_key(request) {
                    return Ok(());
                }
                return Err(BrokerError::UnknownRequest);
            }
        };
        if message.context.capability != expected.0 || message.context.scope != expected.1 {
            return Err(BrokerError::InvalidScope);
        }
        self.check_grant(expected.0, &expected.1)
    }
    pub fn check_grant(&self, capability: Capability, scope: &Scope) -> Result<(), BrokerError> {
        if self.enabled
            && self
                .grants
                .iter()
                .any(|g| g.capability == capability && &g.scope == scope)
        {
            Ok(())
        } else {
            Err(BrokerError::Denied)
        }
    }
    pub fn start(&mut self, message: &Envelope, now: Instant) -> Result<(), BrokerError> {
        self.expire(now);
        self.authorize(message)?;
        if self.pending.contains_key(&message.request_id) {
            return Err(BrokerError::DuplicateRequest);
        }
        if self.pending.len() >= MAX_PENDING {
            return Err(BrokerError::PendingLimit);
        }
        self.pending.insert(
            message.request_id,
            Pending {
                deadline: now + Duration::from_millis(self.budget.timeout_ms()),
            },
        );
        Ok(())
    }
    pub fn cancel(&mut self, request: u64) {
        self.pending.remove(&request);
        self.staged.remove(&request);
    }
    pub fn expire(&mut self, now: Instant) {
        let expired: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, p)| p.deadline <= now)
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            self.cancel(id);
        }
    }
    pub fn accept_reply(&mut self, request: u64, generation: u64, now: Instant) -> Result<(), BrokerError> {
        self.expire(now);
        if !self.enabled || generation != self.generation {
            return Err(BrokerError::Denied);
        }
        self.pending.remove(&request).ok_or(BrokerError::UnknownRequest)?;
        self.staged.remove(&request);
        Ok(())
    }
    pub fn begin_edits(&mut self, message: &Envelope, current_revision: u64, now: Instant) -> Result<(), BrokerError> {
        let Request::BeginEdits { document, revision } = message.request else {
            return Err(BrokerError::InvalidEdits);
        };
        if revision != current_revision {
            return Err(BrokerError::StaleRevision);
        }
        self.start(message, now)?;
        self.staged.insert(
            message.request_id,
            Staged {
                document,
                revision,
                bytes: vec![],
            },
        );
        Ok(())
    }
    pub fn append(&mut self, message: &Envelope, now: Instant) -> Result<(), BrokerError> {
        self.expire(now);
        self.authorize(message)?;
        let Request::AppendChunk { transaction, ref chunk } = message.request else {
            return Err(BrokerError::InvalidEdits);
        };
        if chunk.len() > MAX_CHUNK_BYTES {
            self.cancel(transaction);
            return Err(BrokerError::ChunkLimit);
        }
        let total: usize = self.staged.values().map(|s| s.bytes.len()).sum();
        if total.saturating_add(chunk.len()) > MEMORY_LIMIT / 2 {
            self.cancel(transaction);
            return Err(BrokerError::StagingLimit);
        }
        self.staged
            .get_mut(&transaction)
            .ok_or(BrokerError::UnknownRequest)?
            .bytes
            .extend_from_slice(chunk);
        Ok(())
    }
    /// Returns validated edits for the document actor's single atomic revision-checked
    /// transaction. This does not itself mutate documents or bypass the actor check.
    pub fn commit(
        &mut self,
        message: &Envelope,
        current_revision: u64,
        current_text: &str,
        now: Instant,
    ) -> Result<Vec<TextEdit>, BrokerError> {
        self.commit_checked(message, current_revision, now, |edits| {
            validate_edits(edits, current_text)
        })
    }
    /// Snapshot owners validate boundaries without materializing the whole document.
    pub fn commit_checked(
        &mut self,
        message: &Envelope,
        current_revision: u64,
        now: Instant,
        validate: impl FnOnce(&[TextEdit]) -> Result<(), BrokerError>,
    ) -> Result<Vec<TextEdit>, BrokerError> {
        self.expire(now);
        self.authorize(message)?;
        let Request::CommitEdits { transaction, revision } = message.request else {
            return Err(BrokerError::InvalidEdits);
        };
        let staged = self.staged.remove(&transaction).ok_or(BrokerError::UnknownRequest)?;
        self.pending.remove(&transaction);
        if revision != current_revision || staged.revision != revision {
            return Err(BrokerError::StaleRevision);
        }
        let BoundedEdits(edits) = postcard::from_bytes(&staged.bytes).map_err(|_| BrokerError::InvalidEdits)?;
        validate(&edits)?;
        Ok(edits)
    }
}
pub fn validate_edits(edits: &[TextEdit], text: &str) -> Result<(), BrokerError> {
    let mut end = 0;
    for edit in edits {
        let start = usize::try_from(edit.range.start).map_err(|_| BrokerError::InvalidEdits)?;
        let next = usize::try_from(edit.range.end).map_err(|_| BrokerError::InvalidEdits)?;
        if start < end
            || start > next
            || next > text.len()
            || !text.is_char_boundary(start)
            || !text.is_char_boundary(next)
        {
            return Err(BrokerError::InvalidEdits);
        }
        end = next;
    }
    Ok(())
}
pub fn approve_update(previous: &[Capability], requested: &[Capability], approved: bool) -> Result<(), BrokerError> {
    let old: BTreeSet<_> = previous.iter().collect();
    if !approved && requested.iter().any(|c| !old.contains(c)) {
        Err(BrokerError::ApprovalRequired)
    } else {
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn message(s: &ExtensionSession) -> Envelope {
        Envelope {
            protocol: 1,
            request_id: 1,
            extension_id: "fixture".into(),
            context: CapabilityContext {
                capability: Capability::DocumentEdit,
                scope: Scope::Document(1),
                grant_generation: s.generation(),
            },
            request: Request::BeginEdits {
                document: 1,
                revision: 2,
            },
        }
    }
    #[test]
    fn denied_scopes_revocation_and_timeout() {
        let mut session = ExtensionSession::new("fixture".into()).unwrap();
        for cap in [
            Capability::Network,
            Capability::WorkspaceWrite,
            Capability::ProcessSpawn,
        ] {
            assert_eq!(session.check_grant(cap, &Scope::Extension), Err(BrokerError::Denied));
        }
        session.approve(vec![Grant {
            capability: Capability::DocumentEdit,
            scope: Scope::Document(1),
        }]);
        let msg = message(&session);
        let now = Instant::now();
        session.begin_edits(&msg, 2, now).unwrap();
        session.expire(now + Duration::from_secs(6));
        assert_eq!(session.pending_count(), 0);
        assert!(session.staged.is_empty());
        session.begin_edits(&msg, 2, now).unwrap();
        session.revoke();
        assert_eq!(
            session.accept_reply(1, msg.context.grant_generation, now),
            Err(BrokerError::Denied)
        );
        assert!(session.staged.is_empty());
    }
    #[test]
    fn background_budget_keeps_pending_work_alive_past_the_interactive_deadline() {
        let mut session = ExtensionSession::new_with_budget("fixture".into(), ExecutionBudget::Background).unwrap();
        session.approve(vec![Grant {
            capability: Capability::DocumentEdit,
            scope: Scope::Document(1),
        }]);
        let msg = message(&session);
        let now = Instant::now();
        session.begin_edits(&msg, 2, now).unwrap();
        session.expire(now + Duration::from_secs(6));
        assert_eq!(
            session.pending_count(),
            1,
            "background work must survive the interactive deadline"
        );
        assert!(session.staged.contains_key(&1));
        session.expire(now + Duration::from_millis(ExecutionBudget::Background.timeout_ms() + 1));
        assert_eq!(session.pending_count(), 0);
        assert!(session.staged.is_empty());
    }
    #[test]
    fn stale_and_bad_ranges_and_update_rejected() {
        let mut session = ExtensionSession::new("fixture".into()).unwrap();
        session.approve(vec![Grant {
            capability: Capability::DocumentEdit,
            scope: Scope::Document(1),
        }]);
        assert_eq!(
            session.begin_edits(&message(&session), 3, Instant::now()),
            Err(BrokerError::StaleRevision)
        );
        assert_eq!(
            validate_edits(
                &[TextEdit {
                    range: TextRange { start: 1, end: 2 },
                    replacement: String::new()
                }],
                "é"
            ),
            Err(BrokerError::InvalidEdits)
        );
        assert_eq!(
            approve_update(&[], &[Capability::Network], false),
            Err(BrokerError::ApprovalRequired)
        );
    }
}

/// Decoding enforces count while reading; checking a Vec after deserialization
/// would allow tiny empty-edit encodings to expand without a bound.
struct BoundedEdits(Vec<TextEdit>);
impl<'de> serde::Deserialize<'de> for BoundedEdits {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = BoundedEdits;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("at most 4096 edits")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                if seq.size_hint().is_some_and(|n| n > 4096) {
                    return Err(serde::de::Error::custom("edit count limit"));
                }
                let mut edits = Vec::new();
                let mut replacement_bytes = 0usize;
                while let Some(edit) = seq.next_element::<TextEdit>()? {
                    if edits.len() >= 4096 {
                        return Err(serde::de::Error::custom("edit count limit"));
                    }
                    replacement_bytes = replacement_bytes.saturating_add(edit.replacement.len());
                    if replacement_bytes > MEMORY_LIMIT / 2 {
                        return Err(serde::de::Error::custom("replacement limit"));
                    }
                    edits.push(edit);
                }
                Ok(BoundedEdits(edits))
            }
        }
        deserializer.deserialize_seq(Visitor)
    }
}

pub(crate) fn deserialize_edits<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<TextEdit>, D::Error> {
    <BoundedEdits as serde::Deserialize>::deserialize(deserializer).map(|value| value.0)
}

#[cfg(test)]
mod allocation_tests {
    use super::*;
    #[test]
    fn compact_edit_count_rejected_before_allocating_elements() {
        // Postcard suppresses size_hint when bytes remaining are fewer than the
        // declared element count. A truncated frame still fails without reserving
        // the attacker-declared Vec capacity.
        assert!(postcard::from_bytes::<BoundedEdits>(&[0x81, 0x20]).is_err());
        // With enough bytes for the hint, reject the count before parsing any edit.
        // Each zero edit is range 0..0 plus an empty replacement (three varints).
        let mut complete = vec![0x81, 0x20];
        complete.resize(2 + 4097 * 3, 0);
        let error = postcard::from_bytes::<BoundedEdits>(&complete).err().unwrap();
        assert_eq!(error, postcard::Error::SerdeDeCustom);
    }
}
