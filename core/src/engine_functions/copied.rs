//! One bounded host allocation ledger for copied values, spans and conversion scratch.
//! Slots are fixed bookkeeping; clones share the allocation and its original producer.
use super::contract::{OwnerKey, OwnerKind};
use crate::v8host::{
    S2FunctionCopyInput, S2FunctionCopyOutput, S2FunctionCopyProducer, S2FunctionValue,
};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};
const PROCESS: usize = 32 * 1024 * 1024;
const OWNER: usize = 8 * 1024 * 1024;
const OPERATION: usize = 8 * 1024 * 1024;
const OVERHEAD: usize = 256;
pub(crate) const MAX_STRING: usize = 65535;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Producer {
    domain: u32,
    digest: [u8; 32],
    generation: u64,
}
impl Producer {
    pub(crate) fn engine() -> Self {
        Self {
            domain: 0,
            digest: [0; 32],
            generation: 0,
        }
    }
    /// Only call with an already verified host identity; this is billing, not authority.
    pub(crate) fn owner(owner: &OwnerKey) -> Self {
        let domain = match owner.kind {
            OwnerKind::Plugin => 1u32,
            OwnerKind::GamePackage => 2,
        };
        let mut hash = Sha256::new();
        hash.update(b"s2script.function-copy.producer.v1\0");
        hash.update(domain.to_le_bytes());
        hash.update((owner.id.len() as u64).to_le_bytes());
        hash.update(owner.id.as_bytes());
        Self {
            domain,
            digest: hash.finalize().into(),
            generation: owner.generation,
        }
    }
    pub(crate) fn wire(self) -> S2FunctionCopyProducer {
        S2FunctionCopyProducer {
            version: 1,
            struct_size: 56,
            domain: self.domain,
            reserved: 0,
            digest: self.digest,
            generation: self.generation,
        }
    }
}
#[derive(Clone, Copy)]
struct Slot {
    bytes: usize,
    operation: u64,
    producer: Producer,
}
static SLOTS: Mutex<[Option<Slot>; 1024]> = Mutex::new([None; 1024]);
static NEXT: AtomicU64 = AtomicU64::new(1);
thread_local! {static CURRENT:RefCell<([u64;128],usize)>=const{RefCell::new(([0;128],0))};}
pub(crate) struct Scope;
impl Scope {
    pub(crate) fn enter() -> Result<Self, String> {
        let id = NEXT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |i| i.checked_add(1))
            .map_err(|_| "FunctionCopyBudgetExceeded: operation ids")?;
        CURRENT.with(|s| {
            let mut s = s.borrow_mut();
            let i = s.1;
            if i == 128 {
                return Err("FunctionCopyBudgetExceeded: nested operations".into());
            }
            s.0[i] = id;
            s.1 += 1;
            Ok(Self)
        })
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        CURRENT.with(|s| s.borrow_mut().1 -= 1);
    }
}
struct Charge(usize);
impl Charge {
    fn acquire(bytes: usize, producer: Producer) -> Result<Self, String> {
        let operation = match CURRENT.with(|s| {
            let s = s.borrow();
            (s.1 > 0).then(|| s.0[s.1 - 1])
        }) {
            Some(id) => id,
            None => NEXT
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |i| i.checked_add(1))
                .map_err(|_| "FunctionCopyBudgetExceeded: operation ids")?,
        };
        let mut slots = SLOTS.lock().unwrap();
        let (mut total, mut owner, mut op, mut count) = (0, 0, 0, 0);
        for s in slots.iter().flatten() {
            total += s.bytes;
            if s.producer == producer {
                owner += s.bytes
            }
            if s.operation == operation {
                op += s.bytes;
                count += 1
            }
        }
        let bytes = bytes
            .checked_add(OVERHEAD)
            .ok_or("FunctionCopyBudgetExceeded")?;
        if bytes > PROCESS - std::mem::size_of::<[Option<Slot>; 1024]>() - total
            || bytes > OWNER - owner
            || bytes > OPERATION - op
            || count >= 128
        {
            return Err("FunctionCopyBudgetExceeded: Rust transient allocation".into());
        }
        let i = slots
            .iter()
            .position(Option::is_none)
            .ok_or("FunctionCopyBudgetExceeded: Rust transient items")?;
        slots[i] = Some(Slot {
            bytes,
            operation,
            producer,
        });
        Ok(Self(i))
    }
}
impl Drop for Charge {
    fn drop(&mut self) {
        SLOTS.lock().unwrap()[self.0] = None;
    }
}
#[derive(Clone)]
pub(crate) struct Bookkeeping {
    _charge: Rc<Charge>,
}
impl Bookkeeping {
    pub(crate) fn reserve(bytes: usize, producer: Producer) -> Result<Self, String> {
        Ok(Self {
            _charge: Rc::new(Charge::acquire(bytes, producer)?),
        })
    }
}
struct Allocation {
    bytes: Vec<u8>,
    producer: Producer,
    _charge: Charge,
}
pub(crate) struct Buffer {
    allocation: Rc<Allocation>,
}
impl Buffer {
    pub(crate) fn new(size: usize, producer: Producer) -> Result<Self, String> {
        let charge = Charge::acquire(size, producer)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(size)
            .map_err(|_| "FunctionCopyBudgetExceeded: Rust allocation")?;
        // Vec reports the actual allocation capacity. Never retain extra capacity uncharged.
        if bytes.capacity() != size {
            return Err("FunctionCopyBudgetExceeded: allocator capacity".into());
        }
        bytes.resize(size, 0);
        Ok(Self {
            allocation: Rc::new(Allocation {
                bytes,
                producer,
                _charge: charge,
            }),
        })
    }
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.allocation.bytes
    }
    pub(crate) fn bytes_mut(&mut self) -> &mut [u8] {
        &mut Rc::get_mut(&mut self.allocation)
            .expect("unpublished buffer")
            .bytes
    }
    pub(crate) fn input(&self) -> S2FunctionCopyInput {
        S2FunctionCopyInput {
            version: 1,
            struct_size: 24,
            data: self.allocation.bytes.as_ptr(),
            size: self.allocation.bytes.len() as u64,
        }
    }
    pub(crate) fn output(&mut self) -> S2FunctionCopyOutput {
        let bytes = self.bytes_mut();
        S2FunctionCopyOutput {
            version: 1,
            struct_size: 32,
            data: bytes.as_mut_ptr(),
            capacity: bytes.len() as u64,
            size: 0,
        }
    }
    pub(crate) fn finish(
        mut self,
        flags: u8,
        value: S2FunctionValue,
        output: &S2FunctionCopyOutput,
    ) -> Result<Owned, String> {
        let bytes = &mut Rc::get_mut(&mut self.allocation)
            .expect("unpublished buffer")
            .bytes;
        if output.version != 1
            || output.struct_size != 32
            || output.data != bytes.as_mut_ptr()
            || output.capacity != bytes.len() as u64
            || output.size > output.capacity
            || value.kind != 8
            || value.flags != flags
            || value.reserved != 0
            || value.bits != 0
            || value.aux as u64 != output.size
        {
            return Err("FunctionCopyInvalidTransport: output span".into());
        }
        bytes.truncate(output.size as usize);
        validate(flags, bytes)?;
        Ok(Owned {
            allocation: self.allocation,
            flags,
        })
    }
    pub(crate) fn own(self, flags: u8) -> Result<Owned, String> {
        validate(flags, self.bytes())?;
        Ok(Owned {
            allocation: self.allocation,
            flags,
        })
    }
}
#[derive(Clone)]
pub(crate) struct Owned {
    allocation: Rc<Allocation>,
    pub(crate) flags: u8,
}
impl Owned {
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.allocation.bytes
    }
    pub(crate) fn producer(&self) -> Producer {
        self.allocation.producer
    }
    pub(crate) fn input(&self) -> S2FunctionCopyInput {
        S2FunctionCopyInput {
            version: 1,
            struct_size: 24,
            data: self.bytes().as_ptr(),
            size: self.bytes().len() as u64,
        }
    }
    pub(crate) fn wire(&self) -> S2FunctionValue {
        S2FunctionValue {
            kind: 8,
            flags: self.flags,
            reserved: 0,
            aux: self.bytes().len() as u32,
            bits: 0,
        }
    }
}
pub(crate) fn flag(projection: &str) -> Option<u8> {
    match projection {
        "string" => Some(4),
        "vector" => Some(5),
        _ => None,
    }
}
pub(crate) fn max_size(flag: u8) -> usize {
    if flag == 4 {
        MAX_STRING
    } else {
        12
    }
}
pub(crate) fn validate(flag: u8, bytes: &[u8]) -> Result<(), String> {
    let valid = match flag {
        4 => bytes.len() <= MAX_STRING && !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok(),
        5 => {
            bytes.len() == 12
                && bytes
                    .chunks_exact(4)
                    .all(|b| f32::from_le_bytes(b.try_into().unwrap()).is_finite())
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err("FunctionCopyInvalidValue: string/vector bytes".into())
    }
}
pub(crate) fn empty_input() -> S2FunctionCopyInput {
    S2FunctionCopyInput {
        version: 1,
        struct_size: 24,
        data: std::ptr::null(),
        size: 0,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_values_hold_one_charge_until_last_clone_and_quota_rolls_back() {
        let _scope = Scope::enter().unwrap();
        let producer = Producer::owner(&OwnerKey::plugin("quota", 99));
        let value = Buffer::new(65535, producer).unwrap();
        let mut value = value;
        value.bytes_mut().fill(b'a');
        let value = value.own(4).unwrap();
        let clone = value.clone();
        let count = || {
            SLOTS
                .lock()
                .unwrap()
                .iter()
                .flatten()
                .filter(|s| s.producer == producer)
                .count()
        };
        assert_eq!(count(), 1);
        drop(value);
        assert_eq!(count(), 1);
        drop(clone);
        assert_eq!(count(), 0);
        let mut held = Vec::new();
        for _ in 0..128 {
            held.push(Buffer::new(0, producer).unwrap())
        }
        assert!(Buffer::new(0, producer).is_err());
        drop(held);
        assert_eq!(count(), 0);
        assert!(Buffer::new(0, producer).is_ok());
    }
    #[test]
    fn span_outputs_reject_malformed_metadata_and_unicode_without_leaking_charges() {
        for case in 0..6 {
            let mut b = Buffer::new(12, Producer::engine()).unwrap();
            let mut out = b.output();
            let mut value = S2FunctionValue {
                kind: 8,
                flags: 5,
                reserved: 0,
                aux: 12,
                bits: 0,
            };
            out.size = 12;
            match case {
                0 => out.version = 2,
                1 => out.struct_size = 0,
                2 => out.size = 13,
                3 => value.bits = 1,
                4 => value.flags = 4,
                5 => b.bytes_mut()[0..4].copy_from_slice(&f32::INFINITY.to_le_bytes()),
                _ => unreachable!(),
            }
            assert!(b.finish(5, value, &out).is_err());
        }
        assert!(validate(4, &[0xc0, 0x80]).is_err());
        assert!(validate(4, b"a\0b").is_err());
        assert!(validate(4, b"").is_ok());
    }
    #[test]
    fn owner_generation_operation_and_process_caps_are_independent() {
        let a = Producer::owner(&OwnerKey::plugin("bounded", 1));
        let b = Producer::owner(&OwnerKey::plugin("bounded", 2));
        let _scope = Scope::enter().unwrap();
        let held = Buffer::new(7 * 1024 * 1024, a).unwrap();
        assert!(Buffer::new(2 * 1024 * 1024, b).is_err());
        drop(_scope);
        {
            let _scope = Scope::enter().unwrap();
            assert!(Buffer::new(2 * 1024 * 1024, a).is_err());
            assert!(Buffer::new(2 * 1024 * 1024, b).is_ok());
        }
        drop(held);
        let mut held = Vec::new();
        for i in 0..4 {
            let _scope = Scope::enter().unwrap();
            held.push(
                Buffer::new(
                    7 * 1024 * 1024,
                    Producer::owner(&OwnerKey::plugin("process", i)),
                )
                .unwrap(),
            );
        }
        {
            let _scope = Scope::enter().unwrap();
            assert!(Buffer::new(5 * 1024 * 1024, Producer::engine()).is_err());
        }
        drop(held);
        assert!(Buffer::new(5 * 1024 * 1024, Producer::engine()).is_ok());
    }
    #[test]
    fn stable_identity_is_domain_separated_and_generation_only_changes_transient_key() {
        let a = Producer::owner(&OwnerKey::plugin("@same/id", 1));
        let mut key = OwnerKey::plugin("@same/id", 2);
        let b = Producer::owner(&key);
        assert_eq!(a.digest, b.digest);
        assert_ne!(a, b);
        key.kind = OwnerKind::GamePackage;
        let c = Producer::owner(&key);
        assert_ne!(a.digest, c.digest);
        assert_ne!(b, c);
    }
}
