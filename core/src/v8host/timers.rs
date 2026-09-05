use super::*;

/// Shared helper for the timer natives: create a `PromiseResolver`, stash its `Global` (tagged with
/// the owning plugin) under a fresh async id in the Jobs map (timers do not increment pending
/// jobs), push the timer, reconcile the detour, and return the pending promise.
fn make_timer_promise<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    kind: TimerKind,
) -> v8::Local<'s, v8::Value> {
    let resolver = v8::PromiseResolver::new(scope).unwrap();
    let promise = resolver.get_promise(scope);
    let id = crate::jobs::next_id();
    // Tag the resolver with the CALLING plugin's (id, current generation) — the async-liveness guard.
    let owner = resolver_owner_tag(scope);
    let lease = match crate::async_limits::domain()
        .timers
        .acquire(owner.clone(), 1, 0)
    {
        Ok(l) => l,
        Err(e) => {
            crate::jobs::reject(scope, resolver, &e.to_string());
            return promise.into();
        }
    };
    TIMER_LEASES.with(|m| m.borrow_mut().insert(id, lease));
    // Ledger this timer against the CALLING plugin (Task 6's teardown authority).  A non-plugin/
    // unknown owner is a safe no-op.  No thread-local borrow held across a JS call.
    if let Some((ref oid, generation)) = owner {
        record_resource(oid, generation, plugin::Resource::Timer(id));
    }
    crate::jobs::insert_timer_resolver(scope, id, resolver, owner);
    TIMERS.with(|t| t.borrow_mut().push(id, kind));
    refresh_detour();
    promise.into()
}

/// Native `__s2_delay(ms) -> Promise`.  Resolves after a wall-clock deadline.
pub(super) fn s2_delay(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let ms = args.get(0).integer_value(scope).unwrap_or(0);
        let ms = if ms > 0 { ms as u64 } else { 0 };
        let kind = TimerKind::Deadline(Instant::now() + Duration::from_millis(ms));
        let promise = make_timer_promise(scope, kind);
        rv.set(promise);
    }));
}

/// Native `__s2_timer_create(ms, fn, repeat) -> id`. A CALLBACK timer (SourceMod `CreateTimer`),
/// as opposed to `__s2_delay`'s one-shot Promise. Returns 0 when the arguments are unusable, so JS
/// can report failure instead of handing back a handle that will never fire.
pub(super) fn s2_timer_create(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_double(0.0);
        if args.length() < 2 {
            return;
        }
        let ms = args.get(0).integer_value(scope).unwrap_or(-1);
        if ms < 0 {
            return;
        }
        let ms = ms as u64;
        let Ok(f) = v8::Local::<v8::Function>::try_from(args.get(1)) else {
            return;
        };
        let repeat = args.get(2).boolean_value(scope);
        // A zero-interval REPEATING timer would re-arm itself every drain forever with no way for
        // the frame to make progress on anything else. Refuse it rather than ship a footgun.
        if repeat && ms == 0 {
            return;
        }

        let id = crate::jobs::next_id();
        let owner = resolver_owner_tag(scope);
        let lease = match crate::async_limits::domain()
            .timers
            .acquire(owner.clone(), 1, 0)
        {
            Ok(l) => l,
            Err(e) => {
                let msg = v8::String::new(scope, &e.to_string()).unwrap();
                let ex = v8::Exception::error(scope, msg);
                if let Ok(obj) = v8::Local::<v8::Object>::try_from(ex) {
                    let key = v8::String::new(scope, "name").unwrap();
                    let name = v8::String::new(scope, e.name()).unwrap();
                    obj.create_data_property(scope, key.into(), name.into());
                }
                scope.throw_exception(ex);
                return;
            }
        };
        TIMER_LEASES.with(|m| m.borrow_mut().insert(id, lease));
        if let Some((ref oid, generation)) = owner {
            record_resource(oid, generation, plugin::Resource::Timer(id));
        }
        TIMER_CBS.with(|m| {
            m.borrow_mut().insert(
                id,
                TimerCallback {
                    owner,
                    cb: v8::Global::new(scope.as_ref(), f),
                    interval_ms: if repeat { Some(ms) } else { None },
                },
            )
        });
        TIMERS.with(|t| {
            t.borrow_mut().push(
                id,
                TimerKind::Deadline(Instant::now() + Duration::from_millis(ms)),
            )
        });
        refresh_detour();
        rv.set_double(id as f64);
    }));
}

/// Native `__s2_timer_kill(id) -> bool`. Idempotent: killing an already-dead or never-existing
/// timer is `false`, not an error. Removes from BOTH the queue and the callback map — leaving the
/// callback behind would keep a Global<Function> alive for the isolate's lifetime.
pub(super) fn s2_timer_kill(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let id = args.get(0).integer_value(scope).unwrap_or(0) as u64;
        let owner = resolver_owner_tag(scope);
        let released = match &owner {
            Some((owner, generation)) => {
                release_resource(owner, *generation, &plugin::Resource::Timer(id))
            }
            None => true,
        };
        if !released {
            rv.set_bool(false);
            return;
        }
        let had_cb = TIMER_CBS.with(|m| m.borrow_mut().remove(&id)).is_some();
        let had_q = TIMERS.with(|t| t.borrow_mut().remove(id));
        DUE_TIMERS.with(|q| q.borrow_mut().retain(|n| *n != id));
        TIMER_LEASES.with(|m| m.borrow_mut().remove(&id));
        // Record ONLY the self-kill case, so this set stays bounded. If the timer was still in
        // TIMER_CBS or the queue we removed it above and the drain will never see it; the only way
        // both are absent for a live id is that the drain is holding it mid-fire — i.e. the
        // callback is killing itself. (An id that never existed also lands here; the drain removes
        // whatever it looks up, and shutdown clears the rest.)
        if !had_cb && !had_q {
            TIMER_KILLED.with(|k| {
                k.borrow_mut().insert(id);
            });
        }
        rv.set_bool(owner.is_some() || had_cb || had_q);
    }));
}

/// Native `__s2_timer_alive(id) -> bool`.
pub(super) fn s2_timer_alive(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let id = args.get(0).integer_value(scope).unwrap_or(0) as u64;
        // Killed-mid-fire counts as dead even though the drain is holding the entry.
        if TIMER_KILLED.with(|k| k.borrow().contains(&id)) { rv.set_bool(false); return; }
        rv.set_bool(TIMER_CBS.with(|m| m.borrow().contains_key(&id)));
    }));
}

/// Native `__s2_next_tick() -> Promise`.  Resolves on the very next frame drain
/// (`Frame(FRAME_COUNTER)` → the next drain reads that same count and fires it).
pub(super) fn s2_next_tick(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let target = FRAME_COUNTER.with(|c| c.get());
        let promise = make_timer_promise(scope, TimerKind::Frame(target));
        rv.set(promise);
    }));
}

/// Native `__s2_next_frame() -> Promise`.  Resolves exactly one frame later than `NextTick`
/// (`Frame(FRAME_COUNTER + 1)` → the drain after next).
pub(super) fn s2_next_frame(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let target = FRAME_COUNTER.with(|c| c.get().wrapping_add(1));
        let promise = make_timer_promise(scope, TimerKind::Frame(target));
        rv.set(promise);
    }));
}

/// Native `__s2_thread_sleep(ms) -> Promise`.  Submits a blocking sleep to the worker pool;
/// the Promise resolves the next drain after the worker finishes.
pub(super) fn s2_thread_sleep(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let ms = args.get(0).integer_value(scope).unwrap_or(0);
        let ms = if ms > 0 { ms as u64 } else { 0 };
        let resolver = v8::PromiseResolver::new(scope).unwrap();
        let promise = resolver.get_promise(scope);
        let result = crate::jobs::reserve(scope, 0)
            .map_err(|e| e.to_string())
            .and_then(|lease| {
                crate::jobs::check_live(&lease)?;
                let id = crate::jobs::next_id();
                let cancel = lease.cancel.clone();
                pool().try_submit(
                    id,
                    Box::new(move || {
                        std::thread::sleep(Duration::from_millis(ms));
                        Ok(())
                    }),
                    lease,
                )?;
                crate::jobs::commit_reserved(scope, id, resolver, cancel);
                Ok(())
            });
        if let Err(e) = result {
            crate::jobs::reject(scope, resolver, &e);
        }
        rv.set(promise.into());
    }));
}

