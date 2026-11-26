//! The objects a hook callback is handed, stamped out of templates built once.
//!
//! A hooked engine function calls into JS for every invocation, on some functions for every player
//! on every tick. Each callback is handed a view of the native frame: one accessor pair per
//! parameter, bound to the callback's lease. Building that view from fresh functions costs two
//! `v8::Function`s per parameter per call, and V8 allocates a function template for each in the
//! old generation, so the garbage is only ever reclaimed by a full collection. A server hooking a
//! per-tick function paid tens of microseconds a call and a full collection every second or two.
//!
//! Here each distinct shape is one `FunctionTemplate`, built the first time it is needed and kept
//! for the isolate's life. A view is an instance of it, which V8 clones from a cached boilerplate,
//! and the lease it is bound to is the instance's one internal field. The accessors carry a
//! signature, so V8 itself refuses to run one against any object that is not an instance of the
//! same shape; the lease is still validated on every access exactly as before.
use super::*;
use std::collections::HashMap;

/// What distinguishes one shape from another. Everything a builder reads must be in the key.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Shape {
    /// A callback's view of the native frame.
    Frame { binding: u64, post: bool, skipped: bool, authority: bool },
    /// A borrowed record position's fields.
    Record { binding: u64, selector: i32 },
    /// The package adapter's subscriber cursor.
    Cursor,
    /// What a package adapter's callback is handed: the frame view, the phase and the cursor.
    Facade { post: bool },
}

impl Shape {
    fn binding(&self) -> Option<u64> {
        match self {
            Shape::Frame { binding, .. } | Shape::Record { binding, .. } => Some(*binding),
            Shape::Cursor | Shape::Facade { .. } => None,
        }
    }
    /// Internal fields of an instance: the lease, then whatever the shape's slot getters return.
    fn fields(&self) -> usize {
        match self {
            Shape::Facade { .. } => 3,
            _ => 1,
        }
    }
}

type Words = crate::fasthash::FastState;

thread_local! {
    static SHAPES: RefCell<HashMap<Shape, v8::Global<v8::FunctionTemplate>, Words>> = RefCell::new(HashMap::default());
    /// Property names and the delivery tag's private key, made once.
    static NAMES: RefCell<HashMap<usize, v8::Global<v8::String>, Words>> = RefCell::new(HashMap::default());
    static PRIVATES: RefCell<HashMap<usize, v8::Global<v8::Private>, Words>> = RefCell::new(HashMap::default());
}

/// `name` as an internalized string, made once per isolate. Keyed by the literal's address.
pub(super) fn name<'s>(scope: &mut v8::PinScope<'s, '_>, name: &'static str) -> Result<v8::Local<'s, v8::String>, String> {
    let key = name.as_ptr() as usize;
    if let Some(found) = NAMES.with(|n| n.borrow().get(&key).map(|g| v8::Local::new(scope, g))) {
        return Ok(found);
    }
    let made = v8::String::new_from_utf8(scope, name.as_bytes(), v8::NewStringType::Internalized).ok_or("string allocation")?;
    NAMES.with(|n| n.borrow_mut().insert(key, v8::Global::new(scope, made)));
    Ok(made)
}

/// The API private symbol `name`, looked up once per isolate.
pub(super) fn private<'s>(scope: &mut v8::PinScope<'s, '_>, name: &'static str) -> Result<v8::Local<'s, v8::Private>, String> {
    let key = name.as_ptr() as usize;
    if let Some(found) = PRIVATES.with(|n| n.borrow().get(&key).map(|g| v8::Local::new(scope, g))) {
        return Ok(found);
    }
    let text = v8::String::new(scope, name).ok_or("private key allocation")?;
    let made = v8::Private::for_api(scope, Some(text));
    PRIVATES.with(|n| n.borrow_mut().insert(key, v8::Global::new(scope, made)));
    Ok(made)
}

/// Drop every cached shape. Runs before the isolate drops.
pub(crate) fn reset() {
    SHAPES.with(|s| s.borrow_mut().clear());
    NAMES.with(|n| n.borrow_mut().clear());
    PRIVATES.with(|n| n.borrow_mut().clear());
}

/// Drop the shapes of a binding that is going away.
pub(super) fn forget_binding(id: u64) {
    SHAPES.with(|s| s.borrow_mut().retain(|shape, _| shape.binding() != Some(id)));
}

#[cfg(test)]
pub(super) fn cached() -> usize {
    SHAPES.with(|s| s.borrow().len())
}

/// A shape under construction.
pub(super) struct Builder<'a, 's, 'i> {
    scope: &'a mut v8::PinScope<'s, 'i>,
    instance: v8::Local<'s, v8::ObjectTemplate>,
    signature: v8::Local<'s, v8::Signature>,
}

const FIXED: v8::PropertyAttribute = v8::PropertyAttribute::DONT_DELETE;

impl<'s> Builder<'_, 's, '_> {
    fn key(&mut self, name: &str) -> Result<v8::Local<'s, v8::Name>, String> {
        Ok(v8::String::new(self.scope, name).ok_or("shape name allocation")?.into())
    }
    fn function(
        &mut self,
        callback: impl v8::MapFnTo<v8::FunctionCallback>,
        data: Option<v8::Local<'s, v8::Value>>,
    ) -> v8::Local<'s, v8::FunctionTemplate> {
        let mut builder = v8::FunctionTemplate::builder(callback).signature(self.signature);
        if let Some(data) = data {
            builder = builder.data(data);
        }
        let template = builder.build(self.scope);
        template.remove_prototype();
        template
    }
    /// A read-write accessor property. `data` reaches both callbacks as `args.data()`.
    pub(super) fn accessor(
        &mut self,
        name: &str,
        getter: impl v8::MapFnTo<v8::FunctionCallback>,
        setter: impl v8::MapFnTo<v8::FunctionCallback>,
        data: v8::Local<'s, v8::Value>,
    ) -> Result<(), String> {
        let key = self.key(name)?;
        let getter = self.function(getter, Some(data));
        let setter = self.function(setter, Some(data));
        self.instance.set_accessor_property(key, Some(getter), Some(setter), FIXED | v8::PropertyAttribute::DONT_ENUM);
        Ok(())
    }
    /// A read-only accessor property.
    pub(super) fn getter(&mut self, name: &str, getter: impl v8::MapFnTo<v8::FunctionCallback>) -> Result<(), String> {
        let key = self.key(name)?;
        let getter = self.function(getter, None);
        self.instance.set_accessor_property(key, Some(getter), None, FIXED | v8::PropertyAttribute::DONT_ENUM);
        Ok(())
    }
    /// A method, called as `view.name(...)`.
    pub(super) fn method(&mut self, name: &str, callback: impl v8::MapFnTo<v8::FunctionCallback>) -> Result<(), String> {
        let key = self.key(name)?;
        let function = self.function(callback, None);
        self.instance.set_with_attr(key, function.into(), FIXED | v8::PropertyAttribute::READ_ONLY);
        Ok(())
    }
    /// A constant every instance carries.
    pub(super) fn constant(&mut self, name: &str, value: v8::Local<'s, v8::Primitive>) -> Result<(), String> {
        let key = self.key(name)?;
        self.instance.set_with_attr(key, value.into(), FIXED | v8::PropertyAttribute::READ_ONLY);
        Ok(())
    }
    /// A read-only property answering with the instance's internal field `slot` (see `set_slot`).
    pub(super) fn slot(&mut self, name: &str, slot: i32) -> Result<(), String> {
        let key = self.key(name)?;
        let data = self.integer(slot);
        let getter = self.function(js_slot, Some(data));
        self.instance.set_accessor_property(key, Some(getter), None, FIXED);
        Ok(())
    }
    pub(super) fn string(&mut self, value: &str) -> Result<v8::Local<'s, v8::Primitive>, String> {
        Ok(v8::String::new(self.scope, value).ok_or("shape string allocation")?.into())
    }
    /// Callback data must be a primitive: a shape outlives every context, and an object (an array
    /// of selectors, say) would keep the context it was made in alive after its plugin unloads.
    pub(super) fn integer(&mut self, value: i32) -> v8::Local<'s, v8::Value> {
        v8::Integer::new(self.scope, value).into()
    }
    pub(super) fn boolean(&mut self, value: bool) -> v8::Local<'s, v8::Primitive> {
        v8::Boolean::new(self.scope, value).into()
    }
}

/// A new instance of `shape` in the current context, bound to `lease`. `build` describes the shape
/// and runs only the first time the shape is asked for.
pub(super) fn instance<'s, 'i>(
    scope: &mut v8::PinScope<'s, 'i>,
    shape: Shape,
    lease: u64,
    build: impl FnOnce(&mut Builder<'_, 's, 'i>) -> Result<(), String>,
) -> Result<v8::Local<'s, v8::Object>, String> {
    let cached = SHAPES.with(|s| s.borrow().get(&shape).map(|template| v8::Local::new(scope, template)));
    let template = match cached {
        Some(template) => template,
        None => {
            let template = v8::FunctionTemplate::builder(illegal_constructor).build(scope);
            let instance = template.instance_template(scope);
            instance.set_internal_field_count(shape.fields());
            let signature = v8::Signature::new(scope, template);
            build(&mut Builder { scope, instance, signature })?;
            SHAPES.with(|s| s.borrow_mut().insert(shape, v8::Global::new(scope, template)));
            template
        }
    };
    let object = template.instance_template(scope).new_instance(scope).ok_or("view allocation")?;
    // Lease ids come from one process-wide counter and stay far below 2^53.
    let id = v8::Number::new(scope, lease as f64);
    if !object.set_internal_field(0, id.into()) {
        return Err("view lease slot".into());
    }
    Ok(object)
}

/// Fill one of a `Facade`'s slots.
pub(super) fn set_slot(object: v8::Local<v8::Object>, slot: usize, value: v8::Local<v8::Value>) -> Result<(), String> {
    if object.set_internal_field(slot, value.into()) { Ok(()) } else { Err("view slot".into()) }
}

/// A slot getter. No lease check: a slot holds another view, and every view checks its own lease.
fn js_slot(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let value = args.data().int32_value(scope)
        .and_then(|slot| args.this().get_internal_field(scope, slot as usize))
        .and_then(|field| v8::Local::<v8::Value>::try_from(field).ok());
    match value {
        Some(value) => rv.set(value),
        None => throw(scope, "invalid receiver"),
    }
}

fn illegal_constructor(scope: &mut v8::PinScope, _: v8::FunctionCallbackArguments, _: v8::ReturnValue) {
    throw(scope, "Illegal constructor");
}

/// The lease the receiver was stamped with. The signature guarantees the receiver is an instance of
/// the shape the callback belongs to; anything else never reaches the callback.
pub(super) fn lease_id(scope: &mut v8::PinScope, args: &v8::FunctionCallbackArguments) -> Result<u64, String> {
    let this = args.this();
    let field = this.get_internal_field(scope, 0).ok_or("invalid receiver")?;
    let value = v8::Local::<v8::Value>::try_from(field).map_err(|_| "invalid receiver")?;
    let id = value.number_value(scope).ok_or("invalid receiver")?;
    if !(id >= 1.0 && id <= 9007199254740991.0 && id.fract() == 0.0) {
        return Err("invalid receiver".into());
    }
    Ok(id as u64)
}
