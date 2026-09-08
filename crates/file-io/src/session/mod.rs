// SPDX-License-Identifier: MPL-2.0
//! Bounded session manifests. Call storage methods on the I/O pool after first frame.
//! Decoding never opens document paths; callers must evaluate PathOrigin::Session trust.
use bareline_platform::{LocalFileSystem, SerializedPath};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

mod salvage;
pub use salvage::{decode_report,DecodedSession,SessionDiagnostic,SessionDiagnostics,SessionIssue};
pub const SESSION_VERSION: u32 = 1;
pub const MAX_SESSION_BYTES: usize = 8 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;

/// Optional per-view selection. Identifiers only: never a path or embedded executable definition.
#[derive(Clone,Debug,PartialEq,Eq,Serialize,Deserialize)]
#[serde(tag="kind",content="id",rename_all="snake_case",deny_unknown_fields)]
pub enum LanguageSelection {
    Builtin(#[serde(deserialize_with="language_id")] String),
    Udl(#[serde(deserialize_with="language_id")] String),
}
fn language_id<'de,D:serde::Deserializer<'de>>(d:D)->Result<String,D::Error>{
    let id=String::deserialize(d)?;
    if id.is_empty()||id.len()>64||!id.bytes().all(|b|b.is_ascii_alphanumeric()||matches!(b,b'-'|b'_')){return Err(serde::de::Error::custom("invalid language identifier"));}Ok(id)
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct ViewState {
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub language: Option<LanguageSelection>,
    pub caret: u64,
    pub anchor: u64,
    pub scroll_line: u64,
    /// Canonical paged viewport byte anchor; absent in older session manifests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll_byte: Option<u64>,
    /// Exact logical-pixel scroll value, encoded with f64::to_bits. Finite and nonnegative.
    pub scroll_y_bits: u64,
    pub scroll_x: u32,
    pub split: u32,
    pub folds: Vec<std::ops::Range<u64>>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionDocument {
    pub id: u64,
    pub path: Option<SerializedPath>,
    pub title: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionTab {
    pub id: u64,
    pub document_id: u64,
    pub pinned: bool,
    pub view: ViewState,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SplitOrientation {
    Horizontal,
    #[default]
    Vertical,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionLayout {
    pub tab_colors: std::collections::BTreeMap<u64, u32>,
    pub vertical_tabs: bool,
    pub tab_sort: String,
    pub split: bool,
    pub orientation: SplitOrientation,
    pub ratio_bits: u64,
    pub active_pane: u32,
    pub active_tabs: [Option<u64>; 2],
    pub sync_horizontal: bool,
    pub sync_vertical: bool,
}
impl Default for SessionLayout {
    fn default() -> Self {
        Self {
            tab_colors: Default::default(),
            vertical_tabs: false,
            tab_sort: "manual".into(),
            split: false,
            orientation: SplitOrientation::Vertical,
            ratio_bits: 0.5f64.to_bits(),
            active_pane: 0,
            active_tabs: [None, None],
            sync_horizontal: false,
            sync_vertical: false,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCompare {
    pub version: u32,
    pub left_document: u64,
    pub right_document: u64,
    /// Versioned application compare options, bounded separately from the manifest.
    pub options_json: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionManifest {
    pub version: u32,
    pub documents: Vec<SessionDocument>,
    pub tabs: Vec<SessionTab>,
    pub active_tab: Option<u64>,
    #[serde(default)]
    pub mru: Vec<u64>,
    #[serde(default)]
    pub recent: Vec<SerializedPath>,
    #[serde(default, deserialize_with = "decode_layout")]
    pub layout: SessionLayout,
    #[serde(default)]
    pub compare: Option<SessionCompare>,
}
fn decode_layout<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<SessionLayout, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}
impl Default for SessionManifest {
    fn default() -> Self {
        Self {
            version: SESSION_VERSION,
            documents: vec![],
            tabs: vec![],
            active_tab: None,
            mru: vec![],
            recent: vec![],
            layout: SessionLayout::default(),
            compare: None,
        }
    }
}
fn invalid(message: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}
impl SessionManifest {
    pub fn validate(&self) -> io::Result<()> {
        if self.version != SESSION_VERSION {
            return Err(invalid("unsupported session version"));
        }
        if [
            self.documents.len(),
            self.tabs.len(),
            self.mru.len(),
            self.recent.len(),
        ]
        .iter()
        .any(|n| *n > MAX_ENTRIES)
        {
            return Err(invalid("session entry limit"));
        }
        let mut docs = HashSet::new();
        for doc in &self.documents {
            if !docs.insert(doc.id) || doc.title.len() > 4096 {
                return Err(invalid("duplicate document or oversized title"));
            }
            if let Some(path) = &doc.path {
                validate_path(path)?;
            }
        }
        let mut tabs = HashSet::new();
        if let Some(compare) = &self.compare {
            if compare.version != 1
                || compare.options_json.len() > 65536
                || !docs.contains(&compare.left_document)
                || !docs.contains(&compare.right_document)
                || compare.left_document == compare.right_document
            {
                return Err(invalid("invalid comparison session"));
            }
        }
        let mut unpinned = false;
        for tab in &self.tabs {
            if tab.view.folds.len() > 100_000
                || tab.view.folds.iter().any(|range| range.start >= range.end)
            {
                return Err(invalid("invalid or oversized fold state"));
            }
            let scroll_y = f64::from_bits(tab.view.scroll_y_bits);
            if !scroll_y.is_finite() || scroll_y < 0.0 {
                return Err(invalid("invalid scroll position"));
            }
            if !tabs.insert(tab.id) || !docs.contains(&tab.document_id) {
                return Err(invalid("invalid tab identity"));
            }
            if tab.pinned && unpinned {
                return Err(invalid("pinned tabs must precede unpinned tabs"));
            }
            unpinned |= !tab.pinned;
        }
        if self.active_tab.is_some_and(|id| !tabs.contains(&id)) {
            return Err(invalid("missing active tab"));
        }
        let mut mru = HashSet::new();
        for id in &self.mru {
            if !docs.contains(id) || !mru.insert(id) {
                return Err(invalid("invalid MRU document"));
            }
        }
        for path in &self.recent {
            validate_path(path)?;
        }
        if self.layout.tab_colors.len()>MAX_ENTRIES || self.layout.tab_colors.iter().any(|(id,color)|!tabs.contains(id)||*color>0xff_ffff) {return Err(invalid("invalid session tab color"));}
        if !self.valid_layout() {
            return Err(invalid("invalid session layout"));
        }
        Ok(())
    }
    fn valid_layout(&self) -> bool {
        let ratio = f64::from_bits(self.layout.ratio_bits);
        ratio.is_finite()
            && (0.1..=0.9).contains(&ratio)
            && self.layout.active_pane <= u32::from(self.layout.split)
            && self
                .tabs
                .iter()
                .all(|tab| tab.view.split <= u32::from(self.layout.split))
            && self
                .layout
                .active_tabs
                .iter()
                .enumerate()
                .all(|(pane, id)| {
                    id.is_none_or(|id| {
                        self.tabs
                            .iter()
                            .any(|tab| tab.id == id && tab.view.split == pane as u32)
                    })
                })
    }
    /// Active document first, then MRU, tab documents and surviving orphan documents.
    /// Stable IDs/order are retained even when a corrupt tab entry was discarded.
    /// This is scheduling data only. The app limits concurrent opens to two.
    pub fn restore_order(&self) -> Vec<u64> {
        let active = self
            .tabs
            .iter()
            .find(|tab| Some(tab.id) == self.active_tab)
            .map(|tab| tab.document_id);
        let mut seen = HashSet::new();
        active
            .into_iter()
            .chain(self.mru.iter().copied())
            .chain(self.tabs.iter().map(|tab| tab.document_id))
            .chain(self.documents.iter().map(|document|document.id))
            .filter(|id| seen.insert(*id))
            .collect()
    }
}
fn validate_path(path: &SerializedPath) -> io::Result<()> {
    if path.data.len() > 128 * 1024 || path.display.len() > 128 * 1024 {
        return Err(invalid("session path limit"));
    }
    // Validate canonical representation without converting foreign-platform paths or touching disk.
    path.validate().map_err(invalid)
}
/// Import untrusted machine-written JSON without filesystem access. Version zero used
/// the same layout without MRU/recent; those default empty during migration.
pub fn decode(bytes: &[u8]) -> io::Result<SessionManifest> {decode_report(bytes).map(|decoded|decoded.manifest)}
pub fn encode(manifest: &SessionManifest) -> io::Result<Vec<u8>> {
    manifest.validate()?;
    struct Bounded(Vec<u8>);
    impl Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > MAX_SESSION_BYTES.saturating_sub(self.0.len()) {
                return Err(invalid("session byte limit"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut output = Bounded(Vec::new());
    serde_json::to_writer(&mut output, manifest).map_err(invalid)?;
    Ok(output.0)
}
#[derive(Debug)]
pub struct LoadedSession {
    pub manifest: SessionManifest,
    /// True means the primary could not be read/validated; show a recovery warning.
    pub recovered_previous: bool,
    pub diagnostics: SessionDiagnostics,
}
pub struct SessionStore {
    path: PathBuf,
}
impl SessionStore {
    /// Caller selects an app-owned local state path, never a path supplied by imported JSON.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
    pub fn load(&self) -> io::Result<LoadedSession> {
        match read_manifest(&self.path) {
            Ok(mut decoded) if decoded.manifest.documents.is_empty() && decoded.diagnostics.skipped_documents>0 => {
                if let Ok(previous)=read_manifest(&self.previous()) && !previous.manifest.documents.is_empty() {
                    decoded.diagnostics.merge(previous.diagnostics);
                    Ok(LoadedSession {manifest:previous.manifest,recovered_previous:true,diagnostics:decoded.diagnostics})
                } else {Ok(LoadedSession {manifest:decoded.manifest,recovered_previous:false,diagnostics:decoded.diagnostics})}
            }
            Ok(decoded) => Ok(LoadedSession {
                manifest:decoded.manifest,
                diagnostics:decoded.diagnostics,
                recovered_previous: false,
            }),
            Err(primary) => match read_manifest(&self.previous()) {
                Ok(decoded) => Ok(LoadedSession {
                    manifest:decoded.manifest,
                    diagnostics:decoded.diagnostics,
                    recovered_previous: true,
                }),
                Err(_) => Err(primary),
            },
        }
    }
    /// Serialized by the owning I/O service. Retains a validated previous generation
    /// before publishing the new one. A failed commit never truncates the current file.
    pub fn save(
        &self,
        manifest: &SessionManifest,
        platform: &dyn LocalFileSystem,
    ) -> io::Result<()> {
        let bytes = encode(manifest)?;
        platform.validate_target(&self.path)?;
        if let Ok(previous) = read_manifest(&self.path) && previous.diagnostics.is_empty() {
            atomic_write(&self.previous(), &encode(&previous.manifest)?, platform)?;
        }
        atomic_write(&self.path, &bytes, platform)
    }
    fn previous(&self) -> PathBuf {
        let mut name = self.path.as_os_str().to_os_string();
        name.push(".previous");
        PathBuf::from(name)
    }
}
fn read_manifest(path: &Path) -> io::Result<DecodedSession> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take((MAX_SESSION_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    decode_report(&bytes)
}
fn atomic_write(path: &Path, bytes: &[u8], platform: &dyn LocalFileSystem) -> io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    platform.validate_target(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| invalid("session path has no parent"))?;
    let staged = parent.join(format!(
        ".bareline-session-{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        platform.commit(&staged, path, path.try_exists()?)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    result
}
/// Publish caller-versioned machine state using the same bounded atomic local contract.
/// The caller owns the schema and serializes concurrent writes to this app-owned path.
pub fn publish_json(path: &Path, bytes: &[u8], platform: &dyn LocalFileSystem) -> io::Result<()> {
    if bytes.len() > MAX_SESSION_BYTES {
        return Err(invalid("machine state byte limit"));
    }
    serde_json::from_slice::<serde_json::Value>(bytes).map_err(invalid)?;
    atomic_write(path, bytes, platform)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_platform::FileIdentity;
    use std::sync::atomic::AtomicBool;
    fn fixture() -> SessionManifest {
        SessionManifest {
            documents: vec![
                SessionDocument {
                    id: 7,
                    path: Some(SerializedPath::from_native(Path::new("../untrusted.txt"))),
                    title: "test".into(),
                },
                SessionDocument {
                    id: 8,
                    path: None,
                    title: "Untitled".into(),
                },
            ],
            tabs: vec![
                SessionTab {
                    id: 1,
                    document_id: 7,
                    pinned: true,
                    view: ViewState {
                        language: None,
                        caret: 42,
                        anchor: 20,
                        scroll_line: 12,
                        scroll_byte: Some(4096),
                        scroll_y_bits: 12.25f64.to_bits(),
                        scroll_x: 3,
                        split: 1,
                        folds: vec![3..8],
                    },
                },
                SessionTab {
                    id: 2,
                    document_id: 8,
                    pinned: false,
                    view: ViewState::default(),
                },
            ],
            active_tab: Some(2),
            mru: vec![7, 8],
            layout: SessionLayout {
                split: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }
    #[test]
    fn mixed_corrupt_entries_preserve_ids_order_views_and_safe_references() {
        let original=fixture();let mut value=serde_json::to_value(&original).unwrap();
        let mut bad_path=serde_json::to_value(SerializedPath::from_native(Path::new("must-not-be-opened"))).unwrap();bad_path["data"]=serde_json::json!("!!!!");
        value["documents"]=serde_json::json!([value["documents"][0].clone(),{"id":9,"title":"secret-invalid-title","path":bad_path.clone()},value["documents"][1].clone(),{"id":7,"title":"duplicate","path":null}]);
        value["tabs"][1]["view"]["folds"]=serde_json::json!([{"start":9,"end":2}]);
        let tabs=value["tabs"].as_array_mut().unwrap();tabs.push(serde_json::json!({"id":3,"document_id":9,"pinned":false,"view":{}}));tabs.push(serde_json::json!({"id":1,"document_id":8,"pinned":false,"view":{}}));tabs.push(serde_json::json!({"id":4,"document_id":8,"pinned":true,"view":{}}));
        value["active_tab"]=serde_json::json!(3);value["mru"]=serde_json::json!([9,8,8,7]);value["recent"]=serde_json::json!([original.documents[0].path.clone().unwrap(),bad_path]);
        value["compare"]=serde_json::json!({"version":1,"left_document":7,"right_document":9,"options_json":[]});
        let decoded=decode_report(&serde_json::to_vec(&value).unwrap()).unwrap();let manifest=decoded.manifest;
        assert_eq!(manifest.documents.iter().map(|doc|doc.id).collect::<Vec<_>>(),vec![7,8]);assert_eq!(manifest.documents[0],original.documents[0]);
        assert_eq!(manifest.tabs.iter().map(|tab|tab.id).collect::<Vec<_>>(),vec![1,2,4]);assert_eq!(manifest.tabs[0].view,original.tabs[0].view);assert_eq!(manifest.tabs[1].view,ViewState::default());assert!(!manifest.tabs[2].pinned);
        assert_eq!(manifest.active_tab,Some(1));assert_eq!(manifest.mru,vec![8,7]);assert_eq!(manifest.recent.len(),1);assert!(manifest.compare.is_none());manifest.validate().unwrap();
        assert!(decoded.diagnostics.entries.iter().any(|entry|entry.issue==SessionIssue::InvalidPath));assert!(decoded.diagnostics.entries.iter().any(|entry|entry.issue==SessionIssue::InvalidView));assert!(!decoded.diagnostics.summary().contains("secret-invalid-title"));assert!(!decoded.diagnostics.summary().contains("must-not-be-opened"));
    }
    #[test]
    fn bounded_entry_failure_does_not_consume_following_valid_document() {
        let value=serde_json::json!({"version":1,"documents":[{"id":1,"path":null,"title":"x".repeat(2*1024*1024+1)},{"id":2,"path":null,"title":"kept"}],"tabs":[],"active_tab":null});
        let decoded=decode_report(&serde_json::to_vec(&value).unwrap()).unwrap();assert_eq!(decoded.manifest.restore_order(),vec![2]);assert_eq!(decoded.diagnostics.skipped,1);assert_eq!(decoded.diagnostics.entries[0].issue,SessionIssue::ResourceLimit);
        let mut entries=vec![serde_json::json!({"id":2,"path":null,"title":"kept"})];entries.extend((0..MAX_ENTRIES+20).map(|_|serde_json::Value::Null));
        let value=serde_json::json!({"version":1,"documents":entries,"tabs":[],"active_tab":null});let decoded=decode_report(&serde_json::to_vec(&value).unwrap()).unwrap();assert_eq!(decoded.manifest.documents.len(),1);assert_eq!(decoded.diagnostics.entries.len(),128);assert!(decoded.diagnostics.omitted>0);
    }
    #[test]
    fn ambiguous_entry_fields_are_skipped_but_global_corruption_stays_fatal() {
        let decoded=decode_report(br#"{"version":1,"documents":[{"id":1,"id":2,"path":null,"title":"bad"},{"id":3,"path":null,"title":"good"}],"tabs":[],"active_tab":null}"#).unwrap();assert_eq!(decoded.manifest.restore_order(),vec![3]);assert_eq!(decoded.diagnostics.entries[0].issue,SessionIssue::DuplicateField);
        for bytes in [br#"{"version":1,"version":1,"documents":[],"tabs":[]}"#.as_slice(),br#"{"version":999,"documents":[],"tabs":[]}"#,br#"{"version":1,"documents":["#]{assert!(decode_report(bytes).is_err());}
    }
    #[test]
    fn round_trip_and_migration_preserve_views_and_paths() {
        let manifest = fixture();
        assert_eq!(decode(&encode(&manifest).unwrap()).unwrap(), manifest);
        let old = String::from_utf8(encode(&manifest).unwrap())
            .unwrap()
            .replacen("\"version\":1", "\"version\":0", 1);
        assert_eq!(decode(old.as_bytes()).unwrap(), manifest);
        assert_eq!(manifest.restore_order(), vec![8, 7]);
        let without_byte = String::from_utf8(encode(&manifest).unwrap()).unwrap().replace("\"scroll_byte\":4096,", "");
        let legacy = decode(without_byte.as_bytes()).unwrap();
        assert_eq!(legacy.tabs[0].view.scroll_byte, None);
        assert_eq!(legacy.tabs[0].view.anchor, manifest.tabs[0].view.anchor);
    }
    #[test]
    fn malformed_and_oversized_manifests_fail_closed() {
        assert!(decode(&vec![b' '; MAX_SESSION_BYTES + 1]).is_err());
        assert!(decode(b"{\"version\":999}").is_err());
        let mut manifest = fixture();
        manifest.tabs[1].id = 1;
        assert!(encode(&manifest).is_err());
        manifest = fixture();
        manifest.mru.push(7);
        assert!(encode(&manifest).is_err());
    }
    #[test]
    fn invalid_layout_falls_back_without_losing_documents() {
        let original = fixture();
        let mut value = serde_json::to_value(&original).unwrap();
        value["layout"]["ratio_bits"] = serde_json::json!(f64::NAN.to_bits());
        let restored = decode(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(restored.documents, original.documents);
        assert!(!restored.layout.split);
        assert!(restored.tabs.iter().all(|tab| tab.view.split == 0));
        value["layout"] = serde_json::json!({"orientation":"unknown"});
        assert_eq!(
            decode(&serde_json::to_vec(&value).unwrap())
                .unwrap()
                .documents,
            original.documents
        );
    }
    #[test]
    fn five_hundred_tabs_order_without_document_io() {
        let mut manifest = SessionManifest::default();
        for id in 0..500 {
            manifest.documents.push(SessionDocument {
                id,
                path: None,
                title: id.to_string(),
            });
            manifest.tabs.push(SessionTab {
                id,
                document_id: id,
                pinned: false,
                view: ViewState::default(),
            });
        }
        manifest.active_tab = Some(499);
        let decoded = decode(&encode(&manifest).unwrap()).unwrap();
        assert_eq!(decoded.restore_order()[0], 499);
        assert_eq!(decoded.restore_order().len(), 500);
    }
    struct FakeFs {
        fail: AtomicBool,
    }
    impl LocalFileSystem for FakeFs {
        fn identity(&self, _: &File) -> io::Result<FileIdentity> {
            Err(io::Error::other("unused"))
        }
        fn validate_target(&self, _: &Path) -> io::Result<()> {
            Ok(())
        }
        fn commit(&self, staged: &Path, target: &Path, _: bool) -> io::Result<()> {
            if self.fail.load(Ordering::Relaxed) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected lock",
                ));
            }
            fs::rename(staged, target)
        }
    }
    #[test]
    fn failed_publication_preserves_original_and_corruption_recovers_previous() {
        static NEXT_TEST: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "bareline-session-test-{}-{}",
            std::process::id(),
            NEXT_TEST.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let store = SessionStore::new(dir.join("session.json"));
        let platform = FakeFs {
            fail: AtomicBool::new(false),
        };
        let original = fixture();
        store.save(&original, &platform).unwrap();
        let before = fs::read(&store.path).unwrap();
        platform.fail.store(true, Ordering::Relaxed);
        assert!(store.save(&SessionManifest::default(), &platform).is_err());
        assert_eq!(fs::read(&store.path).unwrap(), before);
        platform.fail.store(false, Ordering::Relaxed);
        store.save(&SessionManifest::default(), &platform).unwrap();
        fs::write(&store.path, b"broken").unwrap();
        let loaded = store.load().unwrap();
        assert!(loaded.recovered_previous);
        assert_eq!(loaded.manifest, original);
        for data in ["!!!!", "YQ=="] {
            let mut corrupt = original.clone();
            let path = corrupt.documents[0].path.as_mut().unwrap();
            path.encoding = bareline_platform::PathEncoding::WindowsUtf16Le;
            path.data = data.into();
            fs::write(&store.path, serde_json::to_vec(&corrupt).unwrap()).unwrap();
            let salvaged=store.load().unwrap();
            assert!(!salvaged.recovered_previous);
            assert_eq!(salvaged.manifest.documents.iter().map(|doc|doc.id).collect::<Vec<_>>(),vec![8]);
            assert!(!salvaged.diagnostics.is_empty());
        }
        let backup_before=fs::read(store.previous()).unwrap();
        store.save(&SessionManifest::default(),&platform).unwrap();
        assert_eq!(fs::read(store.previous()).unwrap(),backup_before,"salvaged primary must not overwrite healthy backup");
        let mut all_bad=serde_json::to_value(&original).unwrap();all_bad["documents"]=serde_json::json!([null,null]);fs::write(&store.path,serde_json::to_vec(&all_bad).unwrap()).unwrap();
        let previous=store.load().unwrap();assert!(previous.recovered_previous);assert_eq!(previous.manifest,original);assert_eq!(previous.diagnostics.skipped_documents,2);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn missing_storage_reports_error() {
        let store = SessionStore::new(
            std::env::temp_dir()
                .join("bareline-nonexistent-session-dir")
                .join("missing.json"),
        );
        assert!(store.load().is_err());
        assert!(
            store
                .save(
                    &SessionManifest::default(),
                    &FakeFs {
                        fail: AtomicBool::new(false)
                    }
                )
                .is_err()
        );
    }
}

#[cfg(test)]mod language_selection_tests{
    use super::*;
    #[test]fn per_view_language_roundtrip_and_legacy_default(){
        let mut view=ViewState::default();assert_eq!(serde_json::from_str::<ViewState>("{}").unwrap().language,None);
        for language in [LanguageSelection::Builtin("rust".into()),LanguageSelection::Udl("my_language-1".into())]{view.language=Some(language);let bytes=serde_json::to_vec(&view).unwrap();assert_eq!(serde_json::from_slice::<ViewState>(&bytes).unwrap(),view);}
        assert!(serde_json::from_str::<ViewState>(r#"{"language":{"kind":"udl","id":"\\\\host\\share"}}"#).is_err());
        assert!(serde_json::from_str::<ViewState>(r#"{"language":{"kind":"builtin","id":"rust","path":"x"}}"#).is_err());
    }
}