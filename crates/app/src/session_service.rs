// SPDX-License-Identifier: MPL-2.0
//! Lazily created bounded worker for session manifests and named import/export.
use bareline_file_io::{
    cancellation::Cancellation,
    session::{self, LoadedSession, SessionManifest, SessionStore},
};
use bareline_platform::{LocalFileSystem, PathOrigin, PathTrustProvider, TrustedRead};
use std::{
    fs::File,
    io::{self, Read},
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender, TryRecvError},
    },
};

pub enum SessionRequest {
    /// At most two candidates per batch, supplied in active/MRU order.
    ResolvePaths {
        documents: Vec<session::SessionDocument>,
        provider: Arc<dyn PathTrustProvider + Send + Sync>,
    },
    Load {
        path: PathBuf,
    },
    Save {
        path: PathBuf,
        manifest: Box<SessionManifest>,
    },
    Import {
        path: PathBuf,
    },
    Export {
        path: PathBuf,
        manifest: Box<SessionManifest>,
    },
}
pub enum SessionCompletion {
    Resolved(Vec<(u64, io::Result<TrustedRead>)>),
    Loaded(io::Result<LoadedSession>),
    Written(io::Result<()>),
}
struct Job {
    request: SessionRequest,
    reply: SyncSender<SessionCompletion>,
    cancel: Cancellation,
}
pub struct SessionService {
    sender: SyncSender<Job>,
}
pub struct SessionTicket {
    receiver: Receiver<SessionCompletion>,
    cancel: Cancellation,
}
impl SessionTicket {
    pub fn try_recv(&self) -> Result<SessionCompletion, TryRecvError> {
        self.receiver.try_recv()
    }
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}
impl Drop for SessionTicket {
    fn drop(&mut self) {
        self.cancel();
    }
}
impl SessionService {
    /// The Shell calls this only after its first frame, and only if session work is needed.
    pub fn new(
        platform: Arc<dyn LocalFileSystem>,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<Job>(4);
        std::thread::Builder::new()
            .name("bareline-session".into())
            .spawn(move || {
                while let Ok(job) = receiver.recv() {
                    let completion = execute(job.request, &job.cancel, platform.as_ref());
                    let _ = job.reply.try_send(completion);
                    notify();
                }
            })?;
        Ok(Self { sender })
    }
    pub fn submit(&self, request: SessionRequest) -> io::Result<SessionTicket> {
        let (reply, receiver) = mpsc::sync_channel(1);
        let cancel = Cancellation::default();
        self.sender
            .try_send(Job {
                request,
                reply,
                cancel: cancel.clone(),
            })
            .map_err(|error| io::Error::new(io::ErrorKind::WouldBlock, error.to_string()))?;
        Ok(SessionTicket { receiver, cancel })
    }
}
fn execute(
    request: SessionRequest,
    cancel: &Cancellation,
    platform: &dyn LocalFileSystem,
) -> SessionCompletion {
    let allowed = || {
        cancel
            .check()
            .map_err(|_| io::Error::new(io::ErrorKind::Interrupted, "session job cancelled"))
    };
    match request {
        SessionRequest::ResolvePaths {
            documents,
            provider,
        } => {
            let oversized = documents.len() > 2;
            SessionCompletion::Resolved(
                documents
                    .into_iter()
                    .map(|doc| {
                        let result = allowed().and_then(|()| {
                            if oversized {
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidInput,
                                    "resolve at most two session paths per batch",
                                ));
                            }
                            let path = doc
                                .path
                                .ok_or_else(|| {
                                    io::Error::new(
                                        io::ErrorKind::InvalidInput,
                                        "untitled document has no path",
                                    )
                                })?
                                .to_native()
                                .map_err(io::Error::other)?;
                            provider.open_read(&path, PathOrigin::Session)
                        });
                        (doc.id, result)
                    })
                    .collect(),
            )
        }
        SessionRequest::Load { path } => {
            SessionCompletion::Loaded(allowed().and_then(|()| SessionStore::new(path).load()))
        }
        SessionRequest::Import { path } => SessionCompletion::Loaded(allowed().and_then(|()| {
            let mut bytes = Vec::new();
            File::open(path)?
                .take((session::MAX_SESSION_BYTES + 1) as u64)
                .read_to_end(&mut bytes)?;
            allowed()?;
            let decoded=session::decode_report(&bytes)?;
            Ok(LoadedSession {
                manifest: decoded.manifest,
                diagnostics: decoded.diagnostics,
                recovered_previous: false,
            })
        })),
        SessionRequest::Save { path, manifest } => {
            SessionCompletion::Written(allowed().and_then(|()| {
                if let Some(parent) = path
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                {
                    std::fs::create_dir_all(parent)?;
                }
                SessionStore::new(path).save(&manifest, platform)
            }))
        }
        SessionRequest::Export { path, manifest } => SessionCompletion::Written(
            allowed().and_then(|()| SessionStore::new(path).save(&manifest, platform)),
        ),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use bareline_platform::FileIdentity;
    use std::{
        path::Path,
        sync::atomic::{AtomicUsize, Ordering},
    };
    struct NoAccess(AtomicUsize);
    impl LocalFileSystem for NoAccess {
        fn identity(&self, _: &File) -> io::Result<FileIdentity> {
            unreachable!()
        }
        fn validate_target(&self, _: &Path) -> io::Result<()> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"))
        }
        fn commit(&self, _: &Path, _: &Path, _: bool) -> io::Result<()> {
            unreachable!()
        }
    }
    #[test]
    fn queued_cancellation_prevents_storage_and_failures_are_returned() {
        let platform = NoAccess(AtomicUsize::new(0));
        let cancel = Cancellation::default();
        cancel.cancel();
        let request = || SessionRequest::Save {
            path: PathBuf::from("not-written.json"),
            manifest: Box::default(),
        };
        assert!(
            matches!(execute(request(), &cancel, &platform),SessionCompletion::Written(Err(error)) if error.kind()==io::ErrorKind::Interrupted)
        );
        assert_eq!(platform.0.load(Ordering::Relaxed), 0);
        assert!(
            matches!(execute(request(), &Cancellation::default(), &platform),SessionCompletion::Written(Err(error)) if error.kind()==io::ErrorKind::PermissionDenied)
        );
    }
    #[test]
    fn worker_delivers_load_error_and_notifies_without_ui_wait() {
        let (notify, notified) = mpsc::sync_channel(1);
        let service = SessionService::new(
            Arc::new(NoAccess(AtomicUsize::new(0))),
            Arc::new(move || {
                let _ = notify.try_send(());
            }),
        )
        .unwrap();
        let ticket = service
            .submit(SessionRequest::Load {
                path: std::env::temp_dir().join("bareline-session-missing-test/none.json"),
            })
            .unwrap();
        notified
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert!(matches!(
            ticket.try_recv().unwrap(),
            SessionCompletion::Loaded(Err(_))
        ));
    }
    #[test]
    fn trust_resolution_is_batched_and_invalid_paths_never_reach_provider() {
        struct Trust(AtomicUsize);
        impl PathTrustProvider for Trust {
            fn canonicalize(
                &self,
                _: &Path,
                _: PathOrigin,
            ) -> io::Result<bareline_platform::PathTrust> {
                unreachable!()
            }
            fn open_read(&self, _: &Path, origin: PathOrigin) -> io::Result<TrustedRead> {
                assert_eq!(origin, PathOrigin::Session);
                self.0.fetch_add(1, Ordering::Relaxed);
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "trust denied",
                ))
            }
        }
        let provider = Arc::new(Trust(AtomicUsize::new(0)));
        let document = |id| session::SessionDocument {
            id,
            path: Some(bareline_platform::SerializedPath::from_native(Path::new(
                "C:/candidate",
            ))),
            title: "candidate".into(),
        };
        let platform = NoAccess(AtomicUsize::new(0));
        let result = execute(
            SessionRequest::ResolvePaths {
                documents: (0..3).map(document).collect(),
                provider: provider.clone(),
            },
            &Cancellation::default(),
            &platform,
        );
        assert!(
            matches!(result,SessionCompletion::Resolved(results) if results.len()==3 && results.iter().all(|(_,r)|r.is_err()))
        );
        assert_eq!(provider.0.load(Ordering::Relaxed), 0);
        let mut corrupt = document(1);
        corrupt.path.as_mut().unwrap().data = "!!!!".into();
        execute(
            SessionRequest::ResolvePaths {
                documents: vec![corrupt],
                provider: provider.clone(),
            },
            &Cancellation::default(),
            &platform,
        );
        assert_eq!(provider.0.load(Ordering::Relaxed), 0);
        execute(
            SessionRequest::ResolvePaths {
                documents: vec![document(1)],
                provider: provider.clone(),
            },
            &Cancellation::default(),
            &platform,
        );
        assert_eq!(provider.0.load(Ordering::Relaxed), 1);
    }
}
