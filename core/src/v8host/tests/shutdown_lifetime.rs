use super::*;
use std::cell::RefCell;
use std::rc::Rc;

struct DropWatch {
    id: String,
    isolate: v8::IsolateHandle,
    events: Rc<RefCell<Vec<(String, bool)>>>,
}

impl Drop for DropWatch {
    fn drop(&mut self) {
        // Public API returns false after isolate disposal. At terminal teardown there is no
        // executing JS to resume; this observes liveness without retaining a raw isolate pointer.
        let live = self.isolate.cancel_terminate_execution();
        self.events.borrow_mut().push((self.id.clone(), live));
    }
}

fn watch(context: &v8::Global<v8::Context>, id: &str, events: &Rc<RefCell<Vec<(String, bool)>>>) {
    HOST.with(|h| {
        let mut host = h.borrow_mut();
        let host = host.as_mut().unwrap();
        let isolate = host.isolate.thread_safe_handle();
        let mut storage = v8::HandleScope::new(&mut host.isolate);
        let scope = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
        let local = v8::Local::new(&scope, context);
        local.set_slot(Rc::new(DropWatch { id: id.into(), isolate, events: events.clone() }));
    });
}

fn watch_host(events: &Rc<RefCell<Vec<(String, bool)>>>) {
    let context = HOST.with(|h| h.borrow().as_ref().unwrap().context.clone());
    watch(&context, "host", events);
}

#[test]
fn shutdown_context_slots_drop_while_isolate_live_even_with_retired_strong_root() {
    init(dummy_logger()).unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    watch_host(&events);
    create_plugin_context("retired-root");
    let retained = clone_plugin_context("retired-root").unwrap();
    watch(&retained, "retired", &events);
    unload_plugin("retired-root");
    assert!(events.borrow().is_empty(), "normal retirement must not clear live context slots");
    HOST.with(|h| {
        let mut host = h.borrow_mut();
        let mut storage = v8::HandleScope::new(&mut host.as_mut().unwrap().isolate);
        let scope = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
        let local = v8::Local::new(&scope, &retained);
        assert_eq!(local.get_slot::<PluginId>().unwrap().0, "retired-root");
        assert!(local.get_slot::<InteropGeneration>().is_some());
    });
    shutdown();
    let mut got = events.borrow().clone();
    got.sort();
    assert_eq!(got, [("host".into(), true), ("retired".into(), true)]);
    drop(retained);
}

#[test]
fn shutdown_retired_microtask_context_drops_slots_without_running_queued_js() {
    init(logger).unwrap();
    LOG.lock().unwrap().clear();
    let events = Rc::new(RefCell::new(Vec::new()));
    create_plugin_context("retired-microtask");
    let context = clone_plugin_context("retired-microtask").unwrap();
    watch(&context, "microtask", &events);
    eval_in_context("retired-microtask", "Promise.resolve().then(() => console.log('shutdown-microtask-ran'))").unwrap();
    drop(context);
    unload_plugin("retired-microtask");
    shutdown();
    assert_eq!(*events.borrow(), [("microtask".into(), true)]);
    assert!(!LOG.lock().unwrap().iter().any(|line| line.contains("shutdown-microtask-ran")));
}

#[test]
fn shutdown_registry_does_not_root_retired_contexts_and_prunes_dead_creation_batch() {
    init(dummy_logger()).unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    for slot in 0..64 {
        let id = format!("weak-registry-{slot}");
        create_plugin_context(&id);
        let context = clone_plugin_context(&id).unwrap();
        watch(&context, &id, &events);
        drop(context);
        unload_plugin(&id);
    }
    HOST.with(|h| {
        let mut host = h.borrow_mut();
        let host = host.as_mut().unwrap();
        assert_eq!(host.contexts.len(), 64);
        // Test-only collection proves the tracking registry keeps no strong context roots.
        host.isolate.low_memory_notification();
        assert!(host.contexts.iter().all(|context| context.is_empty()));
    });
    assert_eq!(events.borrow().len(), 64);
    assert!(events.borrow().iter().all(|(_, live)| *live));
    create_plugin_context("after-prune");
    assert_eq!(HOST.with(|h| h.borrow().as_ref().unwrap().contexts.len()), 1);
    shutdown();
}

#[test]
fn shutdown_context_slot_churn_and_reinit_retires_each_annex_once() {
    for cycle in 0..4 {
        init(dummy_logger()).unwrap();
        let events = Rc::new(RefCell::new(Vec::new()));
        watch_host(&events);
        for slot in 0..130 {
            let id = format!("shutdown-{cycle}-{slot}");
            create_plugin_context(&id);
            let context = clone_plugin_context(&id).unwrap();
            watch(&context, &id, &events);
            drop(context);
            unload_plugin(&id);
        }
        shutdown();
        let got = events.borrow();
        assert_eq!(got.len(), 131);
        assert!(got.iter().all(|(_, live)| *live));
        let unique: std::collections::HashSet<_> = got.iter().map(|(id, _)| id).collect();
        assert_eq!(unique.len(), got.len());
        assert!(HOST.with(|h| h.borrow().is_none()));
        assert!(PLUGINS.with(|p| p.borrow().is_empty()));
    }
}
