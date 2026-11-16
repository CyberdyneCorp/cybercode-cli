//! Foreground (synchronous) subagent links that lend their parent call's input cohort.
//!
//! A foreground child has no Job: its parent's dispatched `agent` call drives it and
//! waits for its answer. While that call runs in this server process the child is the
//! call's work, so the call's complete recorded input cohort is the link's cohort, as
//! a running background Job's immutable origin is. The cohort itself comes from durable
//! step evidence ([`super::Runtime::reserve_child_job_for_call`]); only the link's
//! liveness is process-local, so a restart drops it and the child stamps nothing.
use std::collections::HashMap;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicU64, Ordering};

/// The live foreground links of a runtime, by child Session.
#[derive(Default)]
pub(super) struct Links {
    next: AtomicU64,
    by_child: std::sync::Mutex<HashMap<String, Link>>,
}

#[derive(Clone)]
struct Link {
    parent: String,
    input_message_ids: Vec<String>,
    token: u64,
}

/// What a foreground link lends: the parent Session and its call's input cohort.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Lent {
    by_child: HashMap<String, (String, Vec<String>)>,
}

impl Lent {
    /// The cohort `parent`'s live foreground call lends `child`, if any.
    pub(crate) fn cohort(&self, parent: &str, child: &str) -> Option<&[String]> {
        self.by_child
            .get(child)
            .filter(|(owner, _)| owner == parent)
            .map(|(_, ids)| ids.as_slice())
    }

    #[cfg(test)]
    pub(crate) fn with(mut self, parent: &str, child: &str, ids: &[&str]) -> Self {
        self.by_child.insert(
            child.into(),
            (
                parent.into(),
                ids.iter().map(|id| (*id).to_owned()).collect(),
            ),
        );
        self
    }
}

impl Links {
    pub(super) fn lent(&self) -> Lent {
        let links = self.by_child.lock().unwrap_or_else(PoisonError::into_inner);
        Lent {
            by_child: links
                .iter()
                .map(|(child, link)| {
                    (
                        child.clone(),
                        (link.parent.clone(), link.input_message_ids.clone()),
                    )
                })
                .collect(),
        }
    }

    fn bind(&self, parent: &str, child: &str, input_message_ids: Vec<String>) -> u64 {
        let token = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.by_child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                child.into(),
                Link {
                    parent: parent.into(),
                    input_message_ids,
                    token,
                },
            );
        token
    }

    fn unbind(&self, child: &str, token: u64) {
        let mut links = self.by_child.lock().unwrap_or_else(PoisonError::into_inner);
        if links.get(child).is_some_and(|link| link.token == token) {
            links.remove(child);
        }
    }
}

/// A live foreground link; dropping it (the parent call returned, failed or was
/// cancelled) withdraws the lent cohort.
pub struct ForegroundChild {
    runtime: super::WeakRuntime,
    child: String,
    token: Option<u64>,
}

impl Drop for ForegroundChild {
    fn drop(&mut self) {
        if let (Some(token), Some(runtime)) = (self.token, self.runtime.upgrade()) {
            runtime.inner.foreground_children.unbind(&self.child, token);
        }
    }
}

impl super::jobs::JobAdmission {
    /// Lend the reserved call's complete input cohort to `child` while it runs in the
    /// foreground. A call without a complete recorded cohort lends nothing.
    pub fn lend_foreground(&self, child: &str) -> ForegroundChild {
        let runtime = self.runtime();
        let token = self
            .origin()
            .filter(|origin| super::requests::descendant_inputs::complete(origin))
            .zip(runtime.upgrade())
            .map(|(origin, runtime)| {
                runtime.inner.foreground_children.bind(
                    self.parent(),
                    child,
                    origin.input_message_ids.clone(),
                )
            });
        ForegroundChild {
            runtime,
            child: child.into(),
            token,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_lends_only_while_bound_and_a_stale_unbind_keeps_a_newer_link() {
        let links = Links::default();
        let first = links.bind("parent", "child", vec!["one".into()]);
        assert_eq!(
            links.lent().cohort("parent", "child"),
            Some(&["one".to_owned()][..])
        );
        assert_eq!(links.lent().cohort("other", "child"), None);
        // A later call of the same child replaces the link; the earlier guard's
        // drop must not withdraw it.
        let second = links.bind("parent", "child", vec!["two".into()]);
        links.unbind("child", first);
        assert_eq!(
            links.lent().cohort("parent", "child"),
            Some(&["two".to_owned()][..])
        );
        links.unbind("child", second);
        assert_eq!(links.lent(), Lent::default());
    }
}
