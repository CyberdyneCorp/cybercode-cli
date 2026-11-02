//! Server-lifetime ownership for live binary views. IDs never confer authority.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyber_core::binary::{Document, Error, Metadata};

pub(crate) enum Content {
    Live(Document),
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    Draft(Box<cyber_core::binary::editor::Editor>),
}
impl Content {
    pub fn metadata(&self) -> Metadata {
        match self {
            Self::Live(doc) => doc.metadata(),
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            Self::Draft(editor) => Metadata {
                path: cyber_core::vcs::slash_path(&editor.target().relative),
                length: editor.draft().length(),
                revision: editor.draft().revision(),
                source: cyber_core::binary::Source::Draft,
                within_edit_size_limit: true,
            },
        }
    }
    pub fn validate_resource(&self) -> Result<(), Error> {
        match self {
            Self::Live(doc) => doc.validate(),
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            Self::Draft(editor) => editor.validate_read_resource(),
        }
    }
    pub fn page(
        &mut self,
        revision: &str,
        offset: cyber_core::binary::Offset,
        limit: usize,
    ) -> Result<cyber_core::binary::Page, Error> {
        match self {
            Self::Live(doc) => doc.page(revision, offset, limit),
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            Self::Draft(editor) => editor.draft().page(revision, offset, limit),
        }
    }
    pub fn search(
        &mut self,
        request: &cyber_core::binary::SearchRequest,
        cancelled: impl Fn() -> bool,
    ) -> Result<cyber_core::binary::SearchResult, Error> {
        match self {
            Self::Live(doc) => doc.search(request, cancelled),
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            Self::Draft(editor) => editor.draft().search(request, cancelled),
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[derive(Clone)]
pub(crate) struct EditAdmission {
    pub turn: super::TurnContext,
    pub grant: super::FileDecision,
    pub authority: super::AdmissionAuthority,
}

/// Retained host authority for the actual filesystem worker. Verification only
/// refreshes trusted policy; prompts and hook/model execution belong before it.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(crate) struct BinaryWork {
    runtime: super::Runtime,
    executor: tokio::runtime::Handle,
    turn: super::TurnContext,
    resource: cyber_core::binary::Resource,
    grants: (super::FileDecision, super::FileDecision),
    authority: super::AdmissionAuthority,
    cancel: tokio_util::sync::CancellationToken,
    post: Arc<Mutex<Option<Arc<cyber_core::binary::editor::AcknowledgedChange>>>>,
}
#[cfg(any(target_os = "macos", target_os = "linux"))]
impl BinaryWork {
    /// Metadata recovery of a previously acknowledged source change. Failure
    /// leaves its notification pending and never changes a staged edit's receipt.
    pub fn resume_notifications(&self, editor: &mut cyber_core::binary::editor::Editor) {
        let result = editor.pending_notification_change().and_then(|change| {
            let Some(change) = change else {
                return Ok(());
            };
            if self
                .runtime
                .binary_handoff(change.key())
                .map_err(|_| Error::Io)?
                .is_some()
            {
                let handoff = self
                    .runtime
                    .reconcile_binary_handoff(change.key())
                    .map_err(|_| Error::Io)?;
                match handoff.state {
                    super::binary_effects::State::Observed { .. } => {
                        return editor.notified(&change.version().operation_id);
                    }
                    super::binary_effects::State::Suppressed { reason } => {
                        return editor
                            .notification_suppressed(&change.version().operation_id, &reason);
                    }
                    _ => {}
                }
            }
            self.acknowledge_source(Arc::new(change))
        });
        if result.is_err() {
            self.runtime.hook_notice(
                &self.turn.session_id,
                "FileChanged",
                "A previously acknowledged binary save still has pending notification metadata",
            );
        }
    }
    /// Retain known source acknowledgement for publication after path/checkout
    /// ownership is released, including when the HTTP future was disposed.
    pub fn acknowledge_source(
        &self,
        change: Arc<cyber_core::binary::editor::AcknowledgedChange>,
    ) -> Result<(), Error> {
        if change.owner().session_id != self.turn.session_id
            || change.resource().path() != self.resource.path()
        {
            return Err(Error::Changed);
        }
        let storage = self.runtime.binary_documents().storage()?;
        change.belongs_to(&storage)?;
        {
            let mut post = self.post.lock().map_err(|_| Error::Io)?;
            if post
                .as_ref()
                .is_some_and(|prior| prior.key() != change.key())
            {
                return Err(Error::Limit);
            }
            *post = Some(change.clone());
        }
        self.runtime
            .queue_binary_change(&change)
            .map_err(|_| Error::Io)?;
        Ok(())
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
    /// Invoke from the blocking filesystem worker, immediately before its effect.
    pub fn verify(&self) -> Result<(), Error> {
        if self.is_cancelled() {
            return Err(Error::Cancelled);
        }
        self.authority
            .verify(&self.runtime, &self.turn.session_id)
            .map_err(|_| Error::Changed)?;
        self.executor.block_on(async {
            let current = self
                .runtime
                .file_turn(
                    Some(&self.turn.session_id),
                    std::path::Path::new(&self.turn.directory),
                )
                .await
                .map_err(|_| Error::Changed)?;
            if current != self.turn {
                return Err(Error::Changed);
            }
            self.runtime
                .verify_binary_grants(&self.turn, &self.resource, self.grants)
                .await
                .map_err(|_| Error::Changed)
        })?;
        self.authority
            .verify(&self.runtime, &self.turn.session_id)
            .map_err(|_| Error::Changed)?;
        if self.is_cancelled() {
            return Err(Error::Cancelled);
        }
        Ok(())
    }
}

pub(crate) struct Entry {
    pub session_id: String,
    pub directory: String,
    pub root: PathBuf,
    pub resource: cyber_core::binary::Resource,
    pub document: Mutex<Content>,
    pub read_only: bool,
    pub editing: std::sync::atomic::AtomicBool,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub edit_admission: Mutex<Option<EditAdmission>>,
    used: Mutex<std::time::Instant>,
}

#[derive(Default)]
pub(crate) struct Registry(
    Mutex<HashMap<String, Arc<Entry>>>,
    std::sync::OnceLock<Arc<tokio::sync::Semaphore>>,
    std::sync::OnceLock<Arc<cyber_core::binary::storage::StorageRoot>>,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    std::sync::OnceLock<Arc<tokio::sync::Semaphore>>,
);

impl Registry {
    pub fn worker(&self) -> Result<tokio::sync::OwnedSemaphorePermit, Error> {
        self.1
            .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(16)))
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Limit)
    }

    pub fn storage(&self) -> Result<Arc<cyber_core::binary::storage::StorageRoot>, Error> {
        self.2.get().cloned().ok_or(Error::File)
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub fn edit_worker(&self) -> Result<tokio::sync::OwnedSemaphorePermit, Error> {
        self.3
            .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(4)))
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Limit)
    }

    pub fn insert(
        &self,
        session_id: String,
        directory: String,
        root: PathBuf,
        document: Document,
        read_only: bool,
    ) -> Result<(String, Metadata), Error> {
        let mut documents = self.0.lock().map_err(|_| Error::Io)?;
        documents.retain(|_, entry| {
            entry.editing.load(std::sync::atomic::Ordering::Acquire)
                || entry
                    .used
                    .lock()
                    .is_ok_and(|used| used.elapsed() < std::time::Duration::from_secs(24 * 60 * 60))
        });
        if documents.len() >= 16
            || documents
                .values()
                .filter(|d| d.session_id == session_id)
                .count()
                >= 4
        {
            return Err(Error::Limit);
        }
        let id = cyber_core::ids::new_id("bin");
        let metadata = document.metadata();
        documents.insert(
            id.clone(),
            Arc::new(Entry {
                session_id,
                directory,
                root,
                resource: document.resource(),
                document: Mutex::new(Content::Live(document)),
                read_only,
                editing: std::sync::atomic::AtomicBool::new(false),
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                edit_admission: Mutex::new(None),
                used: Mutex::new(std::time::Instant::now()),
            }),
        );
        Ok((id, metadata))
    }

    pub fn get(&self, id: &str) -> Option<Arc<Entry>> {
        let entry = self.0.lock().ok()?.get(id).cloned()?;
        *entry.used.lock().ok()? = std::time::Instant::now();
        Some(entry)
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub fn insert_recovered(
        &self,
        id: String,
        owner: cyber_core::binary::editor::Owner,
        editor: cyber_core::binary::editor::Editor,
        read_only: bool,
    ) -> Result<Arc<Entry>, Error> {
        let mut documents = self.0.lock().map_err(|_| Error::Io)?;
        if documents.contains_key(&id)
            || documents.len() >= 16
            || documents
                .values()
                .filter(|entry| entry.session_id == owner.session_id)
                .count()
                >= 4
        {
            return Err(Error::Limit);
        }
        let entry = Arc::new(Entry {
            session_id: owner.session_id,
            directory: owner.directory.to_str().ok_or(Error::File)?.into(),
            root: owner.root,
            resource: editor.resource(),
            document: Mutex::new(Content::Draft(Box::new(editor))),
            read_only,
            editing: std::sync::atomic::AtomicBool::new(true),
            edit_admission: Mutex::new(None),
            used: Mutex::new(std::time::Instant::now()),
        });
        documents.insert(id, entry.clone());
        Ok(entry)
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub fn check_session_deletion(&self, ids: &[String]) -> Result<(), Error> {
        // Idle scopes have already been stopped and fenced. Drop clean cache handles
        // so the durable index can acquire each exclusive journal lock.
        let mut documents = self.0.lock().map_err(|_| Error::Io)?;
        for entry in documents
            .values()
            .filter(|entry| ids.contains(&entry.session_id))
        {
            let content = entry.document.try_lock().map_err(|_| Error::Changed)?;
            if let Content::Draft(editor) = &*content
                && (editor.unresolved() || editor.draft().can_undo())
            {
                return Err(Error::Changed);
            }
        }
        documents.retain(|_, entry| !ids.contains(&entry.session_id));
        drop(documents);
        if let Some(storage) = self.2.get() {
            storage.check_session_deletion(ids)?;
        }
        Ok(())
    }

    pub fn remove_sessions(&self, ids: &[String]) {
        if let Ok(mut documents) = self.0.lock() {
            documents.retain(|_, entry| !ids.contains(&entry.session_id));
        }
    }

    pub fn remove(&self, id: &str, entry: &Arc<Entry>) {
        if let Ok(mut documents) = self.0.lock()
            && documents
                .get(id)
                .is_some_and(|current| Arc::ptr_eq(current, entry))
        {
            documents.remove(id);
        }
    }
}

impl super::Runtime {
    /// Install only after bootstrap has prepared and hidden the existing root from
    /// all child launches. An unconfigured runtime keeps binary editing disabled.
    pub fn configure_binary_storage(
        &self,
        root: Arc<cyber_core::binary::storage::StorageRoot>,
    ) -> Result<(), Error> {
        self.inner
            .binary_documents
            .2
            .set(root)
            .map_err(|_| Error::Changed)
    }

    pub(crate) fn binary_documents(&self) -> &Registry {
        &self.inner.binary_documents
    }
}

impl super::Runtime {
    /// Filesystem-only work retains its actual native Location/path/lifecycle owners
    /// even if the HTTP request is disposed. It never launches a subprocess.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub(crate) async fn binary_file_work<T: Send + 'static>(
        &self,
        turn: super::TurnContext,
        document: String,
        resource: cyber_core::binary::Resource,
        grants: (super::FileDecision, super::FileDecision),
        work: impl FnOnce(BinaryWork) -> T + Send + 'static,
    ) -> Result<T, super::RuntimeError> {
        let edit_permit = self
            .binary_documents()
            .edit_worker()
            .map_err(|error| super::RuntimeError::Invalid(error.to_string()))?;
        let lifecycle = self.memory_mutation_lease().await?;
        self.inner.ensure_idle(&turn.session_id)?;
        let handle = self.inner.handle(&turn.session_id).await?;
        let info = handle.state.lock().await.info.clone();
        if self
            .file_turn(
                Some(&turn.session_id),
                std::path::Path::new(&turn.directory),
            )
            .await?
            != turn
        {
            return Err(super::RuntimeError::Conflict(
                "Binary operation scope changed".into(),
            ));
        }
        let authority = self.capture_child_admission(&turn.session_id)?;
        let mut activity =
            super::activity::Scope::reserve_binary(&self.inner, &handle, document).await?;
        let cancel = activity.control.stop.clone();
        let _cancel_on_disposal = cancel.clone().drop_guard();
        let location = self
            .inner
            .claim_location(&info, false, cancel.clone())
            .await?;
        if location.worktree_id != info.worktree_id {
            return Err(super::RuntimeError::Conflict(
                "Binary checkout binding changed".into(),
            ));
        }
        // This scope owns no subprocess. The retained proof keeps the native
        // checkout lock through source mutation and journal settlement, including crash.
        let location = location
            .settle_retained()
            .map_err(super::RuntimeError::Invalid)?;
        let paths = tokio::select! {
            result = self.inner.tools.file_mutation_guard(&turn, &resource) => result.map_err(super::RuntimeError::Invalid)?,
            _ = cancel.cancelled() => return Err(super::RuntimeError::Invalid("Binary operation cancelled before mutation".into())),
        };
        if self
            .file_turn(
                Some(&turn.session_id),
                std::path::Path::new(&turn.directory),
            )
            .await?
            != turn
        {
            return Err(super::RuntimeError::Conflict(
                "Binary operation scope changed".into(),
            ));
        }
        self.verify_binary_grants(&turn, &resource, grants).await?;
        let permit = self
            .binary_documents()
            .worker()
            .map_err(|error| super::RuntimeError::Invalid(error.to_string()))?;
        let post = Arc::new(Mutex::new(None));
        let executor = tokio::runtime::Handle::current();
        let runtime = self.clone();
        let context = BinaryWork {
            runtime: self.clone(),
            executor: tokio::runtime::Handle::current(),
            turn,
            resource,
            grants,
            authority,
            cancel,
            post: post.clone(),
        };
        tokio::task::spawn_blocking(move || {
            let (_lifecycle, _location, _paths, _permit, _edit_permit) =
                (lifecycle, location, paths, permit, edit_permit);
            activity.launch()?;
            let result = work(context);
            // A returned unknown file outcome is not a live/unowned native process.
            // Its durable intent remains unresolved in the document journal.
            let settlement = activity.settle();
            // Keep lifecycle/capacity ownership until the post-commit handoff
            // finishes, but release every filesystem mutation owner first.
            drop((_location, _paths));
            let change = post
                .lock()
                .map_err(|_| {
                    super::RuntimeError::Invalid("Binary handoff registry unavailable".into())
                })?
                .take();
            if let Some(change) = change {
                let session = change.owner().session_id.clone();
                if let Err(error) = executor.block_on(runtime.publish_binary_change(change)) {
                    runtime.hook_notice(
                        &session,
                        "FileChanged",
                        &format!("Binary save notification remains pending: {error}"),
                    );
                }
            }
            settlement?;
            Ok(result)
        })
        .await
        .map_err(|_| {
            super::RuntimeError::Conflict(
                "Binary worker interrupted; query its operation before retrying".into(),
            )
        })?
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    async fn verify_binary_grants(
        &self,
        turn: &super::TurnContext,
        resource: &cyber_core::binary::Resource,
        grants: (super::FileDecision, super::FileDecision),
    ) -> Result<(), super::RuntimeError> {
        for (action, granted) in [("read", grants.0), ("edit", grants.1)] {
            let decision = Some(self.binary_file_decision(turn, action, resource).await);
            if !matches!(decision, Some(super::FileDecision::Allow))
                && !(matches!(decision, Some(super::FileDecision::Ask))
                    && granted == super::FileDecision::Ask)
            {
                return Err(super::RuntimeError::Conflict(
                    "Binary file authority changed".into(),
                ));
            }
        }
        Ok(())
    }
}
