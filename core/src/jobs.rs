//! Async Jobs spine — process-global ids, the resolver map, pending-job count,
//! and the owner-generation resolve-or-drop protocol.
//!
//! Feature modules (http / ws / net / db / sqldb) and the timer natives mint an
//! id here and move only plain data across threads. V8 handles never leave the
//! main thread. Isolate / context / liveness facts arrive through the narrow
//! `v8host` adapter (`jobs_owner_tag`, `jobs_record_job`, `jobs_owner_is_live`,
//! `jobs_clone_plugin_context`); this module never names `HOST`, `PLUGINS`, or
//! `REGISTRY`.
//!
//! Production adapters reserve producer-owned quota before copying native inputs and commit a
//! captured owner/generation only after submission succeeds. The unmetered begin helper exists
//! solely for deterministic adapter tests. Resolvers retain cancellation, never the producer lease.
//!
//! Timers share the id space and the resolver map but do **not** increment
//! pending jobs. Callback-timer maps and engine adapters stay out of this file.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// A pending async resolver plus the OWNING plugin's `(id, generation)` captured
/// at creation. `owner` is `None` for a resolver created from a non-plugin
/// context (the shared HOST context via the C-ABI `eval` surface): such a
/// resolver has no plugin liveness to check and is always settled. For a
/// plugin-owned resolver, [`settle_if_live`] checks generation-gated liveness
/// before settling and DROPS the continuation (never settles into a
/// disposed/replaced context) if the plugin unloaded or its generation
/// advanced.
pub(crate) struct ResolverEntry {
    owner: Option<(String, u64)>,
    resolver: v8::Global<v8::PromiseResolver>,
    cancel: Option<crate::async_limits::CancelHandle>,
}

/// Monotonic async-id allocator (1-based; 0 is reserved as "none").
///
/// PROCESS-GLOBAL, not thread_local — and that is the whole point. These ids key
/// the resolver map, but they are ALSO handed to engines that live for the
/// process: the threadpool, http/fetch, and the ws/net registries. A per-thread
/// counter feeding process-wide registries means two threads mint the SAME id
/// for unrelated work.
///
/// That is not theoretical. libtest runs every `#[test]` on its own thread, so a
/// thread_local counter restarted at 1 for each test while the threadpool's
/// completion channel carried on across all of them. A completion from an
/// earlier test arriving during a later one found the later test's resolver
/// under the same id, removed it, and resolved it with `undefined`.
///
/// Never reset during shutdown. A resettable counter is what made the stale-id
/// rule false.
static NEXT_ASYNC_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// `async id → ResolverEntry`. Dropped when the timer/job fires, when its
    /// plugin unloads, or when the async-liveness guard drops it. Cleared in
    /// `shutdown` BEFORE the isolate is dropped. Never held across the
    /// microtask checkpoint.
    static RESOLVERS: RefCell<HashMap<u64, ResolverEntry>> = RefCell::new(HashMap::new());
    /// Count of in-flight async-FFI jobs (not timers). Feeds the combined
    /// detour predicate. Decremented only when a resolver is actually removed.
    static PENDING_JOBS: Cell<usize> = const { Cell::new(0) };
}

/// Allocate the next process-global async id. Timers, jobs, and engine conn ids
/// share this space. Never reused, never reset.
pub(crate) fn next_id() -> u64 {
    NEXT_ASYNC_ID.fetch_add(1, Ordering::Relaxed)
}

/// In-flight async-FFI job count (timers are counted separately by the timer
/// queue). Read by the combined lazy-detour predicate.
pub(crate) fn pending() -> usize {
    PENDING_JOBS.with(|c| c.get())
}

/// True when the resolver map is empty. Used by teardown assertions and the
/// process-singleton reset test.
pub(crate) fn resolver_is_empty() -> bool {
    RESOLVERS.with(|m| m.borrow().is_empty())
}

/// Insert a timer resolver. Shares the id/map with jobs but does **not**
/// increment [`pending`]. The caller pushes the timer queue and reconciles the
/// detour.
pub(crate) fn insert_timer_resolver(
    scope: &mut v8::PinScope,
    id: u64,
    resolver: v8::Local<v8::PromiseResolver>,
    owner: Option<(String, u64)>,
) {
    insert(id, owner, v8::Global::new(scope.as_ref(), resolver));
}

/// Immediate begin: mint an id, create the Promise, ledger a `Job`, insert the
/// resolver, bump pending, reconcile the detour, return `(id, promise)`.
///
/// Used by fetch, `threadSleep`, WebSocket connect, and net connect/bind —
/// submission happens after this returns, keyed by `id`.
#[cfg(test)]
pub(crate) fn begin_job<'s>(scope: &mut v8::PinScope<'s, '_>) -> (u64, v8::Local<'s, v8::Value>) {
    let resolver = v8::PromiseResolver::new(scope).unwrap();
    let promise = resolver.get_promise(scope);
    let id = next_id();
    commit_job(scope, id, resolver);
    (id, promise.into())
}

/// Commit a previously-minted id after a successful submit. Ledger + map +
/// pending + detour happen here and nowhere earlier, so an immediate reject
/// (SQLite / remote DB) creates no flicker.
#[cfg(test)]
pub(crate) fn commit_job(
    scope: &mut v8::PinScope,
    id: u64,
    resolver: v8::Local<v8::PromiseResolver>,
) {
    let owner = crate::v8host::jobs_owner_tag(scope);
    if let Some((ref oid, generation)) = owner {
        crate::v8host::jobs_record_job(oid, generation, id);
    }
    insert(id, owner, v8::Global::new(scope.as_ref(), resolver));
    PENDING_JOBS.with(|c| c.set(c.get() + 1));
    crate::v8host::refresh_detour();
}

/// Remove a resolver without touching pending. Timers use this; a missing entry
/// means the timer was already dropped (e.g. by unload).
pub(crate) fn take_resolver(id: u64) -> Option<ResolverEntry> {
    RESOLVERS.with(|m| m.borrow_mut().remove(&id))
}

/// Complete a job: remove the resolver and decrement pending only if it was
/// present. A stale/missing/double completion is a no-op on the count.
pub(crate) fn complete_job(id: u64) -> Option<ResolverEntry> {
    let entry = take_resolver(id)?;
    PENDING_JOBS.with(|c| c.set(c.get().saturating_sub(1)));
    if let Some((owner, generation)) = &entry.owner {
        crate::v8host::release_resource(owner, *generation, &crate::plugin::Resource::Job(id));
    }
    Some(entry)
}

/// Release a completed timer using the resolver's captured owner generation.
pub(crate) fn release_timer(entry: &ResolverEntry, id: u64) {
    if let Some((owner, generation)) = &entry.owner {
        crate::v8host::release_resource(owner, *generation, &crate::plugin::Resource::Timer(id));
    }
}

/// Plugin `Resource::Job` teardown. Decrements pending only when a resolver was
/// actually removed, so a later drain of the same id cannot double-decrement.
pub(crate) fn drop_if_present(id: u64) -> bool {
    if let Some(entry) = complete_job(id) {
        if let Some(c) = entry.cancel {
            c.cancel();
        }
        true
    } else {
        false
    }
}

/// The single owner-generation liveness and resolve-or-drop protocol.
///
/// A plugin-tagged entry is settled only if the owner is still live at the
/// captured generation — otherwise it is DROPPED (returns without settling; the
/// `ResolverEntry` — and its `Global<PromiseResolver>` — is dropped by the
/// caller, releasing the handle while the isolate is still alive). An untagged
/// entry (`owner == None`) has no plugin liveness to check and is settled in
/// the shared HOST context.
///
/// The owner's `Global<Context>` is cloned out of the plugin table (borrow
/// released) before the settle; a settle does NOT run JS under kExplicit, so no
/// continuation re-enters here.
pub(crate) fn settle_if_live<F>(
    isolate: &mut v8::OwnedIsolate,
    host_ctx: &v8::Global<v8::Context>,
    entry: &ResolverEntry,
    settle: F,
) where
    F: for<'s, 'i> FnOnce(&mut v8::PinScope<'s, 'i>, v8::Local<'s, v8::PromiseResolver>),
{
    let g_ctx = match &entry.owner {
        Some((id, generation)) => {
            if !crate::v8host::jobs_owner_is_live(id, *generation) {
                return; // plugin unloaded or reloaded → DROP
            }
            match crate::v8host::jobs_clone_plugin_context(id) {
                Some(g) => g,
                None => return, // context gone (defensive) → drop
            }
        }
        None => host_ctx.clone(),
    };

    let mut hs_storage = v8::HandleScope::new(isolate);
    let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
    let hs = &mut hs;
    let ctx_local = v8::Local::new(hs, &g_ctx);
    let mut scope = v8::ContextScope::new(hs, ctx_local);
    let resolver = v8::Local::new(&mut scope, &entry.resolver);
    settle(&mut scope, resolver);
}

/// Resolve with `undefined`, or drop on the liveness guard. Timers and
/// threadpool jobs use this.
pub(crate) fn resolve_undefined(
    isolate: &mut v8::OwnedIsolate,
    host_ctx: &v8::Global<v8::Context>,
    entry: &ResolverEntry,
) {
    settle_if_live(isolate, host_ctx, entry, |scope, resolver| {
        let undef = v8::undefined(scope);
        resolver.resolve(scope, undef.into());
    });
}

/// Process-singleton reset: drop every resolver Global. MUST run while the
/// isolate is still alive. Does **not** reset [`NEXT_ASYNC_ID`].
pub(crate) fn reset_resolvers() {
    let entries = RESOLVERS.with(|m| std::mem::take(&mut *m.borrow_mut()));
    for (_, e) in entries {
        if let Some(c) = e.cancel {
            c.cancel();
        }
    }
}

/// Process-singleton reset: pending-job count back to zero. Does **not** reset
/// [`NEXT_ASYNC_ID`].
pub(crate) fn reset_pending() {
    PENDING_JOBS.with(|c| c.set(0));
}

fn insert(id: u64, owner: Option<(String, u64)>, resolver: v8::Global<v8::PromiseResolver>) {
    RESOLVERS.with(|m| {
        m.borrow_mut().insert(
            id,
            ResolverEntry {
                owner,
                resolver,
                cancel: None,
            },
        );
    });
}

#[cfg(test)]
pub(crate) fn resolver_ids() -> Vec<u64> {
    RESOLVERS.with(|m| m.borrow().keys().copied().collect())
}

#[cfg(test)]
mod tests {
    #[test]
    fn request_header_growth_is_amortized_and_fits_admitted_metadata() {
        let mut headers = Vec::new();
        let mut growths = 0;
        for count in 1..=16385 {
            let old_capacity = headers.capacity();
            super::push_request_header(&mut headers, String::new(), String::new());
            growths += usize::from(headers.capacity() != old_capacity);
            assert!(headers.capacity() * std::mem::size_of::<(String, String)>() <= count * 64);
        }
        eprintln!("{growths} capacity growths for 16385 headers");
        assert!(growths < 64, "{growths} capacity growths for 16385 headers");
    }

    use super::*;

    /// The process-singleton reset clears the map and the pending count. The
    /// id allocator is process-global and must keep climbing — a resettable
    /// counter is what made a stale threadpool completion resolve a later
    /// test's unrelated Promise.
    #[test]
    fn async_ids_remain_monotonic_across_singleton_reset() {
        assert!(
            resolver_is_empty(),
            "jobs unit test expects a clean resolver map (prior isolate test must shutdown)"
        );
        let a = next_id();
        reset_resolvers();
        reset_pending();
        let b = next_id();
        assert!(
            b > a,
            "reset must not rewind the async id allocator ({a} then {b})"
        );
        assert_eq!(pending(), 0, "reset_pending must leave the count at zero");
        assert!(
            resolver_is_empty(),
            "reset_resolvers must leave the map empty"
        );
    }

    /// Missing and double completion must not undercount. Without a V8 Global
    /// we can still drive the count protocol: `complete_job` / `drop_if_present`
    /// decrement only when a resolver was actually removed.
    #[test]
    fn missing_and_double_completion_do_not_undercount_pending() {
        assert_eq!(pending(), 0);
        assert!(
            complete_job(9_999_998).is_none(),
            "missing complete is a no-op"
        );
        assert_eq!(pending(), 0, "missing complete must not decrement");
        assert!(!drop_if_present(9_999_997), "missing drop is a no-op");
        assert_eq!(pending(), 0, "missing drop must not decrement");
        // A second complete of the same missing id is still a no-op (double).
        assert!(complete_job(9_999_998).is_none());
        assert_eq!(pending(), 0);
    }
}

/// Admission precedes native input copies, ledger insertion and resolver retention.
pub(crate) fn reserve(
    scope: &mut v8::PinScope,
    bytes: usize,
) -> Result<crate::async_limits::JobLease, crate::async_limits::AdmissionError> {
    crate::async_limits::domain().job(crate::v8host::jobs_owner_tag(scope), bytes)
}
pub(crate) fn check_live(lease: &crate::async_limits::JobLease) -> Result<(), String> {
    if lease
        .cancel
        .owner
        .as_ref()
        .is_some_and(|(owner, generation)| !crate::v8host::jobs_owner_is_live(owner, *generation))
    {
        lease.cancel.cancel();
        Err("AsyncCancelled".into())
    } else {
        Ok(())
    }
}
pub(crate) fn commit_reserved(
    scope: &mut v8::PinScope,
    id: u64,
    resolver: v8::Local<v8::PromiseResolver>,
    cancel: crate::async_limits::CancelHandle,
) {
    let owner = cancel.owner.clone();
    if let Some((ref oid, generation)) = owner {
        crate::v8host::jobs_record_job(oid, generation, id);
    }
    insert(id, owner, v8::Global::new(scope.as_ref(), resolver));
    RESOLVERS.with(|m| {
        if let Some(e) = m.borrow_mut().get_mut(&id) {
            e.cancel = Some(cancel);
        }
    });
    PENDING_JOBS.with(|c| c.set(c.get() + 1));
    crate::v8host::refresh_detour();
}

pub(crate) fn reject(
    scope: &mut v8::PinScope,
    resolver: v8::Local<v8::PromiseResolver>,
    message: &str,
) {
    let msg = v8::String::new(scope, message).unwrap();
    let ex = v8::Exception::error(scope, msg);
    if let Ok(obj) = v8::Local::<v8::Object>::try_from(ex) {
        let name = message.split(':').next().unwrap_or("Error");
        if matches!(
            name,
            "AsyncQueueFull"
                | "AsyncPayloadTooLarge"
                | "AsyncCancelled"
                | "HttpResponseTooLarge"
                | "DatabaseResultTooLarge"
        ) {
            let key = v8::String::new(scope, "name").unwrap();
            let val = v8::String::new(scope, name).unwrap();
            obj.create_data_property(scope, key.into(), val.into());
        }
    }
    resolver.reject(scope, ex);
    crate::v8host::request_microtask_drain();
}

const STRING_METADATA_BYTES: usize = 32;

pub(crate) fn copy_string(
    scope: &mut v8::PinScope,
    value: v8::Local<v8::Value>,
    lease: &mut crate::async_limits::JobLease,
) -> Result<String, crate::async_limits::AdmissionError> {
    let s = value
        .to_string(scope)
        .ok_or(crate::async_limits::AdmissionError::PayloadTooLarge)?;
    lease.input_grow(s.utf8_length(scope).saturating_add(STRING_METADATA_BYTES))?;
    Ok(s.to_rust_string_lossy(scope))
}

pub(crate) fn has_resolver(id: u64) -> bool {
    RESOLVERS.with(|m| m.borrow().contains_key(&id))
}

/// Append strings already admitted by copy_string, using their metadata allowance for slots.
pub(crate) fn push_request_header(headers: &mut Vec<(String, String)>, key: String, value: String) {
    if headers.len() == headers.capacity() {
        const HEADER_METADATA_BYTES: usize = 2 * STRING_METADATA_BYTES;
        const SLOT_BYTES: usize = std::mem::size_of::<(String, String)>();
        const { assert!(SLOT_BYTES <= HEADER_METADATA_BYTES); }
        // Include the already-admitted pair on the stack. On 64-bit targets this
        // grows by roughly 4/3, amortizing reallocations without charging twice.
        let credit = (headers.len() + 1)
            .checked_mul(HEADER_METADATA_BYTES)
            .expect("admitted header metadata fits usize");
        let capacity = credit / SLOT_BYTES;
        headers.reserve_exact(capacity - headers.len());
    }
    headers.push((key, value));
}
