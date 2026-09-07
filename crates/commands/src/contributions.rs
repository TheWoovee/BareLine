// SPDX-License-Identifier: MPL-2.0
//! Owned extension command identities; no leaked strings or reused slot aliases.
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DynamicCommandIdentity {
    pub owner: String,
    pub id: String,
    pub generation: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DynamicCommandRecord {
    pub identity: DynamicCommandIdentity,
    pub title: String,
    pub enabled: bool,
    pub disabled_reason: Option<String>,
}
#[derive(Default)]
pub struct DynamicContributions {
    records: BTreeMap<(String, String), DynamicCommandRecord>,
}
impl DynamicContributions {
    /// All validation precedes replacement, so invalid updates retain old entries.
    pub fn replace_owner(
        &mut self,
        owner: &str,
        records: Vec<DynamicCommandRecord>,
    ) -> Result<(), &'static str> {
        fn valid(value: &str) -> bool {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        }
        if !valid(owner) || records.len() > 256 {
            return Err("Contribution owner/count limit");
        }
        let other = self
            .records
            .keys()
            .filter(|(existing, _)| existing != owner)
            .count();
        if other + records.len() > 1024 {
            return Err("Total contribution limit");
        }
        let mut replacement = BTreeMap::new();
        for record in records {
            if record.identity.owner != owner
                || !valid(&record.identity.id)
                || record.title.is_empty()
                || record.title.len() > 256
                || record
                    .disabled_reason
                    .as_ref()
                    .is_some_and(|reason| reason.len() > 512)
            {
                return Err("Invalid contribution");
            }
            let key = (owner.to_owned(), record.identity.id.clone());
            if replacement.insert(key, record).is_some() {
                return Err("Duplicate contribution");
            }
        }
        self.remove_owner(owner);
        self.records.extend(replacement);
        Ok(())
    }
    pub fn remove_owner(&mut self, owner: &str) {
        self.records.retain(|(existing, _), _| existing != owner);
    }
    pub fn entries(&self) -> impl Iterator<Item = &DynamicCommandRecord> {
        self.records.values()
    }
    /// A palette result is invalid after revocation or any owner generation change.
    pub fn resolve(&self, identity: &DynamicCommandIdentity) -> Option<&DynamicCommandRecord> {
        self.records
            .get(&(identity.owner.clone(), identity.id.clone()))
            .filter(|record| record.enabled && record.identity == *identity)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn command(generation: u64) -> DynamicCommandRecord {
        DynamicCommandRecord {
            identity: DynamicCommandIdentity {
                owner: "fixture".into(),
                id: "fixture.run".into(),
                generation,
            },
            title: "Fixture command".into(),
            enabled: true,
            disabled_reason: None,
        }
    }
    #[test]
    fn update_is_atomic_and_old_palette_identity_is_revoked() {
        let mut model = DynamicContributions::default();
        model.replace_owner("fixture", vec![command(1)]).unwrap();
        let old = command(1).identity;
        assert!(
            model
                .replace_owner("fixture", vec![command(2), command(2)])
                .is_err()
        );
        assert!(model.resolve(&old).is_some());
        model.replace_owner("fixture", vec![command(2)]).unwrap();
        assert!(model.resolve(&old).is_none());
        model.remove_owner("fixture");
        assert!(model.resolve(&command(2).identity).is_none());
    }
}
