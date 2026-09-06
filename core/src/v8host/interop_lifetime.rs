//! Owned forward registrations and optional availability attachments.
//! Registry identities and the consumer's immutable context origin authorize every operation.
use super::*;
use std::collections::HashMap;

struct Attachment {
    watch_id: u64,
    consumer: String,
    generation: u64,
    name: String,
    provider: (String, u64),
    subscriptions: Vec<u64>,
    disposers: Vec<v8::Global<v8::Function>>,
}

thread_local! {
    static WATCH_CALLBACKS: std::cell::RefCell<HashMap<u64, v8::Global<v8::Function>>> = Default::default();
    static ATTACHMENTS: std::cell::RefCell<HashMap<u64, Attachment>> = Default::default();
    // A token is usable only on the synchronous host-to-attach stack, never nested interop.
    static ATTACH_AUTH: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

fn next_id() -> u64 {
    NEXT_SUB_ID.with(|c| {
        let id = c.get();
        c.set(id + 1);
        id
    })
}
fn origin(scope: &mut v8::PinScope) -> Option<(String, u64)> {
    let id = current_plugin(scope)?;
    let generation = scope
        .get_current_context()
        .get_slot::<InteropGeneration>()?
        .0;
    REGISTRY
        .with(|r| r.borrow().is_live(&id, generation))
        .then_some((id, generation))
}
fn active(id: &str, generation: u64) -> bool {
    REGISTRY.with(|r| r.borrow().is_live(id, generation))
        && plugin_phase(id) == Some(plugin::Phase::Active)
}
fn attachment_live(scope: &mut v8::PinScope, token: u64) -> bool {
    let Some((consumer, generation)) = origin(scope) else {
        return false;
    };
    let snapshot = ATTACHMENTS.with(|a| {
        a.borrow().get(&token).map(|a| {
            (
                a.consumer.clone(),
                a.generation,
                a.name.clone(),
                a.provider.clone(),
            )
        })
    });
    snapshot.is_some_and(|(owner, owner_gen, name, provider)| {
        owner == consumer
            && owner_gen == generation
            && active(&consumer, generation)
            && active(&provider.0, provider.1)
            && IFACES.with(|r| r.borrow().producer_of(&name)) == Some(provider)
    })
}
fn authorized(scope: &mut v8::PinScope, token: u64) -> bool {
    ATTACH_AUTH.with(|a| a.get() == Some(token))
        && interop_wire::at_interop_boundary()
        && attachment_live(scope, token)
}

/// Protocol 2 raw natives obey the same registration boundary as the public API.
/// The capability names the exact attachment, consumer origin, and interface.
pub(super) fn subscription_authorized(scope: &mut v8::PinScope, name: &str, token: u64) -> bool {
    if token != 0 {
        return authorized(scope, token)
            && ATTACHMENTS.with(|a| a.borrow().get(&token).is_some_and(|a| a.name == name));
    }
    origin(scope)
        .is_some_and(|(consumer, _)| plugin_phase(&consumer) == Some(plugin::Phase::Loading))
}
pub(super) fn track_subscription(token: u64, id: u64) {
    ATTACHMENTS.with(|a| {
        if let Some(a) = a.borrow_mut().get_mut(&token) {
            a.subscriptions.push(id);
        }
    });
}
fn dispose_subscription(consumer: &str, generation: u64, id: u64) {
    if !IFACES.with(|r| r.borrow_mut().remove_subscriber(id, consumer, generation)) {
        return; // A foreign or stale caller must not change the attachment's removal index.
    }
    IFACE_SUBS.with(|s| s.borrow_mut().remove(&id));
    release_resource(consumer, generation, &plugin::Resource::EventSub(id));
    ATTACHMENTS.with(|a| {
        for a in a.borrow_mut().values_mut() {
            a.subscriptions.retain(|sid| *sid != id);
        }
    });
}

pub(super) fn s2_iface_dispose(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let Some((consumer, generation)) = origin(scope) else {
        return;
    };
    let id = args.get(0).integer_value(scope).unwrap_or(0) as u64;
    dispose_subscription(&consumer, generation, id);
}

pub(super) fn s2_iface_watch(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let Some((consumer, generation)) = origin(scope) else {
        throw_named(scope, "InterfaceUnavailable", "watch origin");
        return;
    };
    if plugin_phase(&consumer) != Some(plugin::Phase::Loading) {
        throw_named(
            scope,
            "InterfaceRegistrationClosed",
            "watchOptional requires the load window",
        );
        return;
    }
    let name = args.get(0).to_rust_string_lossy(scope);
    if IFACES.with(|r| r.borrow().dep_kind(&consumer, &name))
        != Some(crate::interfaces::Kind::Optional)
    {
        throw_named(
            scope,
            "InterfaceDependencyError",
            "watchOptional requires optionalPluginDependencies",
        );
        return;
    }
    if !PLUGIN_INTEROP.with(|p| {
        p.borrow()
            .get(&consumer)
            .is_some_and(|m| m.contains_key(&name))
    }) {
        throw_named(
            scope,
            "InterfaceTypesMismatch",
            "watchOptional requires a verified protocol 2 contract",
        );
        return;
    }
    let Ok(callback) = v8::Local::<v8::Function>::try_from(args.get(1)) else {
        throw_named(
            scope,
            "InterfaceAttachmentError",
            "attach must be a function",
        );
        return;
    };
    let id = next_id();
    IFACES.with(|r| {
        r.borrow_mut()
            .add_watch(crate::interfaces::AvailabilityWatch {
                id,
                name,
                consumer_id: consumer.clone(),
                consumer_gen: generation,
                attempted: None,
            })
    });
    WATCH_CALLBACKS.with(|c| {
        c.borrow_mut()
            .insert(id, v8::Global::new(scope.as_ref(), callback))
    });
    record_resource(&consumer, generation, plugin::Resource::InterfaceWatch(id));
    rv.set_double(id as f64);
}

pub(super) fn s2_iface_watch_dispose(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let Some((consumer, generation)) = origin(scope) else {
        return;
    };
    let id = args.get(0).integer_value(scope).unwrap_or(0) as u64;
    let owned = IFACES.with(|r| {
        r.borrow()
            .watch(id)
            .is_some_and(|w| w.consumer_id == consumer && w.consumer_gen == generation)
    });
    if owned {
        dispose_watch(scope, id);
    }
}

pub(super) fn s2_iface_attachment_live(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let token = args.get(0).integer_value(scope).unwrap_or(0) as u64;
    if !attachment_live(scope, token) {
        throw_named(scope, "InterfaceUnavailable", "expired attachment");
    }
}
pub(super) fn s2_iface_attachment_own(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let token = args.get(0).integer_value(scope).unwrap_or(0) as u64;
    if !authorized(scope, token) {
        throw_named(
            scope,
            "InterfaceRegistrationClosed",
            "attachment ownership is synchronous",
        );
        return;
    }
    let Ok(dispose) = v8::Local::<v8::Function>::try_from(args.get(1)) else {
        throw_named(
            scope,
            "InterfaceAttachmentError",
            "resource must have dispose()",
        );
        return;
    };
    let dispose = v8::Global::new(scope.as_ref(), dispose);
    ATTACHMENTS.with(|a| {
        if let Some(a) = a.borrow_mut().get_mut(&token) {
            a.disposers.push(dispose);
        }
    });
}

fn dispose_attachment(scope: &mut v8::PinScope, token: u64) {
    // Remove authority FIRST. Owned dispose functions can throw, dispose their watch, or reenter.
    let Some(attachment) = ATTACHMENTS.with(|a| a.borrow_mut().remove(&token)) else {
        return;
    };
    release_resource(
        &attachment.consumer,
        attachment.generation,
        &plugin::Resource::InterfaceAttachment(token),
    );
    for id in attachment.subscriptions {
        dispose_subscription(&attachment.consumer, attachment.generation, id);
    }
    // Do not enter a replacement context with callbacks belonging to the old generation.
    if !REGISTRY.with(|r| {
        r.borrow()
            .is_live(&attachment.consumer, attachment.generation)
    }) {
        return;
    }
    let Some(context) = PLUGINS.with(|p| {
        p.borrow()
            .get(&attachment.consumer)
            .map(|p| p.context.clone())
    }) else {
        return;
    };
    let context = v8::Local::new(scope, context);
    let scope = &mut v8::ContextScope::new(scope, context);
    for disposer in attachment.disposers.into_iter().rev() {
        let mut tc_storage = v8::TryCatch::new(scope);
        let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
        let tc = &mut tc;
        let disposer = v8::Local::new(tc, &disposer);
        let recv = v8::undefined(tc).into();
        if disposer.call(tc, recv, &[]).is_none() {
            log_warn(&format!(
                "InterfaceAttachmentError: consumer '{}' interface '{}' disposer threw",
                attachment.consumer, attachment.name
            ));
        }
    }
}
fn dispose_watch(scope: &mut v8::PinScope, id: u64) {
    let watch = IFACES.with(|r| r.borrow_mut().remove_watch(id));
    WATCH_CALLBACKS.with(|c| c.borrow_mut().remove(&id));
    if let Some(watch) = watch {
        release_resource(
            &watch.consumer_id,
            watch.consumer_gen,
            &plugin::Resource::InterfaceWatch(id),
        );
    }
    let tokens: Vec<_> = ATTACHMENTS.with(|a| {
        a.borrow()
            .iter()
            .filter(|(_, a)| a.watch_id == id)
            .map(|(id, _)| *id)
            .collect()
    });
    for token in tokens {
        dispose_attachment(scope, token);
    }
}

/// Safe HOST entry: called from lifecycle only, never from a publish or dispatch native.
/// All registry/callback-map borrows end before invoking JS; native reentry uses this PinScope.
fn with_scope(f: impl FnOnce(&mut v8::PinScope)) {
    HOST.with(|h| {
        let mut host = h.borrow_mut();
        let Some(host) = host.as_mut() else { return };
        let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
        let context = v8::Local::new(&hs, &host.context);
        let scope = &mut v8::ContextScope::new(&mut hs, context);
        f(scope);
    });
}

/// Retire watches before dropping their consumer context, and attachments before a provider's
/// removal can leave a retained proxy usable. The provider is already Unloading (or Loading).
pub(super) fn teardown_plugin(id: &str) {
    let watches = IFACES.with(|r| {
        r.borrow()
            .watches()
            .into_iter()
            .filter(|w| w.consumer_id == id)
            .map(|w| w.id)
            .collect::<Vec<_>>()
    });
    let tokens = ATTACHMENTS.with(|a| {
        a.borrow()
            .iter()
            .filter(|(_, a)| a.provider.0 == id)
            .map(|(id, _)| *id)
            .collect::<Vec<_>>()
    });
    with_scope(|scope| {
        for watch in watches {
            dispose_watch(scope, watch);
        }
        for token in tokens {
            dispose_attachment(scope, token);
        }
    });
}

/// Availability is scanned once at the lifecycle boundary. Publication only changes bookkeeping.
/// Snapshotting attempts means an attachment cannot recursively cause another attachment.
pub(super) fn drain_attachments() {
    let pending = IFACES
        .with(|r| r.borrow().watches())
        .into_iter()
        .filter_map(|w| {
            if !active(&w.consumer_id, w.consumer_gen) {
                return None;
            }
            let provider = IFACES.with(|r| r.borrow().producer_of(&w.name))?;
            (active(&provider.0, provider.1) && w.attempted.as_ref() != Some(&provider))
                .then_some((w, provider))
        })
        .collect::<Vec<_>>();
    if pending.is_empty() {
        return;
    }
    with_scope(|scope| {
        for (watch, provider) in pending {
            if !active(&watch.consumer_id, watch.consumer_gen)
                || !active(&provider.0, provider.1)
                || IFACES.with(|r| {
                    r.borrow().watch(watch.id).is_none()
                        || r.borrow().producer_of(&watch.name) != Some(provider.clone())
                })
            {
                continue;
            }
            IFACES.with(|r| r.borrow_mut().attempt_watch(watch.id, provider.clone()));
            if !IFACES.with(|r| r.borrow().is_available(&watch.consumer_id, &watch.name))
                || !matches!(
                    checked_contract(&watch.consumer_id, &watch.name),
                    Ok(Some(_))
                )
            {
                log_warn(&format!("InterfaceTypesMismatch: optional consumer '{}' interface '{}' provider '{}' generation {} incompatible", watch.consumer_id, watch.name, provider.0, provider.1));
                continue;
            }
            let callback = WATCH_CALLBACKS.with(|c| c.borrow().get(&watch.id).cloned());
            let context = PLUGINS.with(|p| {
                p.borrow()
                    .get(&watch.consumer_id)
                    .map(|p| p.context.clone())
            });
            let (Some(callback), Some(context)) = (callback, context) else {
                continue;
            };
            let token = next_id();
            ATTACHMENTS.with(|a| {
                a.borrow_mut().insert(
                    token,
                    Attachment {
                        watch_id: watch.id,
                        consumer: watch.consumer_id.clone(),
                        generation: watch.consumer_gen,
                        name: watch.name.clone(),
                        provider,
                        subscriptions: Vec::new(),
                        disposers: Vec::new(),
                    },
                )
            });
            record_resource(
                &watch.consumer_id,
                watch.consumer_gen,
                plugin::Resource::InterfaceAttachment(token),
            );
            let context = v8::Local::new(scope, context);
            let scope = &mut v8::ContextScope::new(scope, context);
            let ok = {
                let mut tc_storage = v8::TryCatch::new(scope);
                let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
                let tc = &mut tc;
                let callback = v8::Local::new(tc, &callback);
                let recv = v8::undefined(tc).into();
                let arg = v8::Number::new(tc, token as f64).into();
                ATTACH_AUTH.with(|a| a.set(Some(token)));
                let result = callback.call(tc, recv, &[arg]);
                // Close authorization before thenable inspection/observation, including getters.
                ATTACH_AUTH.with(|a| a.set(None));
                result.is_some_and(|value| !interop_wire::observe_thenable(tc, value))
            };
            if !ok {
                log_warn(&format!("InterfaceAttachmentError: optional consumer '{}' interface '{}' attach threw or returned a thenable", watch.consumer_id, watch.name));
                dispose_attachment(scope, token);
            }
        }
    });
}

pub(super) fn register_resets() {
    use crate::process_singletons::{register, ResetPhase::BeforeIsolateDrop};
    register(
        "IFACE_WATCH_CALLBACKS",
        BeforeIsolateDrop,
        Box::new(|| WATCH_CALLBACKS.with(|c| c.borrow_mut().clear())),
    );
    register(
        "IFACE_ATTACHMENTS",
        BeforeIsolateDrop,
        Box::new(|| ATTACHMENTS.with(|a| a.borrow_mut().clear())),
    );
    register(
        "IFACE_ATTACH_AUTH",
        BeforeIsolateDrop,
        Box::new(|| ATTACH_AUTH.with(|a| a.set(None))),
    );
}

pub(super) fn counts() -> (usize, usize, usize, usize, usize) {
    let watches = IFACES.with(|r| r.borrow().watches());
    let pending = watches
        .iter()
        .filter(|w| {
            IFACES
                .with(|r| r.borrow().producer_of(&w.name))
                .is_some_and(|p| {
                    active(&w.consumer_id, w.consumer_gen)
                        && active(&p.0, p.1)
                        && w.attempted.as_ref() != Some(&p)
                })
        })
        .count();
    (
        watches.len(),
        WATCH_CALLBACKS.with(|c| c.borrow().len()),
        ATTACHMENTS.with(|a| a.borrow().len()),
        ATTACHMENTS.with(|a| a.borrow().values().map(|a| a.disposers.len()).sum()),
        pending,
    )
}
