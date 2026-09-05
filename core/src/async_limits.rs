//! Process-stable application admission accounting. No V8 or engine state.
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub(crate) type Owner = Option<(String, u64)>;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdmissionError {
    QueueFull,
    PayloadTooLarge,
    Cancelled,
}
impl AdmissionError {
    pub fn name(self) -> &'static str {
        match self {
            Self::QueueFull => "AsyncQueueFull",
            Self::PayloadTooLarge => "AsyncPayloadTooLarge",
            Self::Cancelled => "AsyncCancelled",
        }
    }
}
impl std::fmt::Display for AdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub(crate) struct Gauge {
    pub items: usize,
    pub bytes: usize,
    pub rejected: u64,
}
#[derive(Default)]
struct State {
    total: Gauge,
    owners: HashMap<Owner, Gauge>,
}
/// Every lease retains this domain. Removing a resolver or reinitializing an isolate never resets it.
pub(crate) struct Budget {
    limits: [usize; 4],
    state: Mutex<State>,
}
pub(crate) struct Lease {
    budget: Arc<Budget>,
    owner: Owner,
    items: usize,
    bytes: usize,
}
impl Budget {
    pub fn new(items: usize, bytes: usize, owner_items: usize, owner_bytes: usize) -> Arc<Self> {
        Arc::new(Self {
            limits: [items, bytes, owner_items, owner_bytes],
            state: Mutex::new(State::default()),
        })
    }
    pub fn acquire(
        self: &Arc<Self>,
        owner: Owner,
        items: usize,
        bytes: usize,
    ) -> Result<Lease, AdmissionError> {
        let mut s = self.state.lock().unwrap();
        let o = s.owners.get(&owner).copied().unwrap_or_default();
        let [ni, nb, oi, ob] = self.limits;
        let impossible = items > ni || bytes > nb || items > oi || bytes > ob;
        if impossible
            || items > ni.saturating_sub(s.total.items)
            || bytes > nb.saturating_sub(s.total.bytes)
            || items > oi.saturating_sub(o.items)
            || bytes > ob.saturating_sub(o.bytes)
        {
            s.total.rejected = s.total.rejected.saturating_add(1);
            return Err(if impossible {
                AdmissionError::PayloadTooLarge
            } else {
                AdmissionError::QueueFull
            });
        }
        s.total.items += items;
        s.total.bytes += bytes;
        let o = s.owners.entry(owner.clone()).or_default();
        o.items += items;
        o.bytes += bytes;
        Ok(Lease {
            budget: self.clone(),
            owner,
            items,
            bytes,
        })
    }
    pub fn snapshot(&self) -> Gauge {
        self.state.lock().unwrap().total
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut s = self.budget.state.lock().unwrap();
        s.total.items -= self.items;
        s.total.bytes -= self.bytes;
        let o = s.owners.get_mut(&self.owner).unwrap();
        o.items -= self.items;
        o.bytes -= self.bytes;
        if o.items == 0 && o.bytes == 0 {
            s.owners.remove(&self.owner);
        }
        drop(s);
        CAPACITY.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn admission_is_atomic_at_count_and_byte_edges() {
        let b = Budget::new(2, 6, 1, 4);
        let a = Some(("a".into(), 1));
        let l = b.acquire(a.clone(), 1, 4).unwrap();
        assert!(matches!(b.acquire(a, 1, 0), Err(AdmissionError::QueueFull)));
        assert!(matches!(
            b.acquire(None, 1, 3),
            Err(AdmissionError::QueueFull)
        ));
        assert_eq!((b.snapshot().items, b.snapshot().bytes), (1, 4));
        drop(l);
        assert_eq!((b.snapshot().items, b.snapshot().bytes), (0, 0));
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct AsyncPolicy {
    pub jobs_global: usize,
    pub jobs_per_owner: usize,
    pub input_bytes: usize,
    pub owner_input_bytes: usize,
    pub completion_bytes: usize,
    pub failure_bytes: usize,
    pub sockets_global: usize,
    pub sockets_per_owner: usize,
    pub sqlite_global: usize,
    pub sqlite_per_owner: usize,
    pub pools_global: usize,
    pub pools_per_owner: usize,
    pub socket_out_items: usize,
    pub socket_out_bytes: usize,
    pub outbound_bytes: usize,
    pub inbound_items: usize,
    pub inbound_bytes: usize,
    pub timers_global: usize,
    pub timers_per_owner: usize,
    pub worker_queue_items: usize,
    pub sqlite_queue_items: usize,
    pub http_body_bytes: usize,
    pub input_item_bytes: usize,
    pub db_result_rows: usize,
    pub db_result_bytes: usize,
    pub cookie_versions: usize,
    pub cookie_bytes: usize,
    pub cookie_write_bytes: usize,
    pub frame_items: usize,
    pub frame_bytes: usize,
    pub frame_poll_items: usize,
    pub frame_soft_us: u64,
}
impl Default for AsyncPolicy {
    fn default() -> Self {
        Self {
            jobs_global: 512,
            jobs_per_owner: 64,
            input_bytes: 32 << 20,
            owner_input_bytes: 8 << 20,
            completion_bytes: 16 << 20,
            failure_bytes: 4096,
            sockets_global: 128,
            sockets_per_owner: 16,
            sqlite_global: 32,
            sqlite_per_owner: 4,
            pools_global: 16,
            pools_per_owner: 4,
            socket_out_items: 256,
            socket_out_bytes: 1 << 20,
            outbound_bytes: 16 << 20,
            inbound_items: 2048,
            inbound_bytes: 16 << 20,
            timers_global: 4096,
            timers_per_owner: 512,
            worker_queue_items: 256,
            sqlite_queue_items: 64,
            http_body_bytes: 10 << 20,
            input_item_bytes: 8 << 20,
            db_result_rows: 10_000,
            db_result_bytes: 8 << 20,
            cookie_versions: 4096,
            cookie_bytes: 4 << 20,
            cookie_write_bytes: 64 << 10,
            frame_items: 256,
            frame_bytes: 2 << 20,
            frame_poll_items: 256,
            frame_soft_us: 2000,
        }
    }
}
impl AsyncPolicy {
    pub fn validate(&self) -> bool {
        let fields = [
            self.jobs_global,
            self.jobs_per_owner,
            self.input_bytes,
            self.owner_input_bytes,
            self.completion_bytes,
            self.failure_bytes,
            self.sockets_global,
            self.sockets_per_owner,
            self.sqlite_global,
            self.sqlite_per_owner,
            self.pools_global,
            self.pools_per_owner,
            self.socket_out_items,
            self.socket_out_bytes,
            self.outbound_bytes,
            self.inbound_items,
            self.inbound_bytes,
            self.timers_global,
            self.timers_per_owner,
            self.worker_queue_items,
            self.sqlite_queue_items,
            self.http_body_bytes,
            self.input_item_bytes,
            self.db_result_rows,
            self.db_result_bytes,
            self.cookie_versions,
            self.cookie_bytes,
            self.cookie_write_bytes,
            self.frame_items,
            self.frame_bytes,
            self.frame_poll_items,
        ];
        self.failure_bytes >= 256
            && fields
                .iter()
                .all(|n| *n > 0 && *n <= isize::MAX as usize / 16)
            && self.frame_soft_us > 0
            && self.jobs_per_owner <= self.jobs_global
            && self.owner_input_bytes <= self.input_bytes
            && self.sockets_per_owner <= self.sockets_global
            && self.sqlite_per_owner <= self.sqlite_global
            && self.pools_per_owner <= self.pools_global
            && self.timers_per_owner <= self.timers_global
            && self.input_item_bytes <= self.owner_input_bytes
            && self.cookie_write_bytes <= self.cookie_bytes
            && self
                .jobs_global
                .checked_mul(self.failure_bytes)
                .is_some_and(|n| n <= self.completion_bytes)
            && self.socket_out_bytes <= self.outbound_bytes
    }
}
static POLICY: OnceLock<AsyncPolicy> = OnceLock::new();
pub(crate) fn policy() -> &'static AsyncPolicy {
    POLICY.get_or_init(|| match std::env::var("S2SCRIPT_ASYNC_LIMITS_JSON") {
        Ok(json) => match serde_json::from_str::<AsyncPolicy>(&json) {
            Ok(p) if p.validate() => p,
            _ => {
                eprintln!("s2script: invalid S2SCRIPT_ASYNC_LIMITS_JSON; using complete defaults");
                AsyncPolicy::default()
            }
        },
        _ => AsyncPolicy::default(),
    })
}

pub(crate) struct Domain {
    pub jobs: Arc<Budget>,
    pub results: Arc<Budget>,
    pub sockets: Arc<Budget>,
    pub sqlite: Arc<Budget>,
    pub pools: Arc<Budget>,
    pub inbound: Arc<Budget>,
    pub outbound: Arc<Budget>,
    pub timers: Arc<Budget>,
    pub policy: AsyncPolicy,
}
impl Domain {
    pub fn new(p: AsyncPolicy) -> Arc<Self> {
        Arc::new(Self {
            jobs: Budget::new(
                p.jobs_global,
                p.input_bytes,
                p.jobs_per_owner,
                p.owner_input_bytes,
            ),
            results: Budget::new(
                p.jobs_global,
                p.completion_bytes,
                p.jobs_global,
                p.completion_bytes,
            ),
            sockets: Budget::new(
                p.sockets_global,
                usize::MAX,
                p.sockets_per_owner,
                usize::MAX,
            ),
            sqlite: Budget::new(p.sqlite_global, usize::MAX, p.sqlite_per_owner, usize::MAX),
            pools: Budget::new(
                p.pools_global,
                p.input_bytes,
                p.pools_per_owner,
                p.owner_input_bytes,
            ),
            inbound: Budget::new(
                p.inbound_items,
                p.inbound_bytes,
                p.inbound_items,
                p.inbound_bytes,
            ),
            outbound: Budget::new(
                p.sockets_global.saturating_mul(p.socket_out_items),
                p.outbound_bytes,
                p.sockets_global.saturating_mul(p.socket_out_items),
                p.outbound_bytes,
            ),
            timers: Budget::new(p.timers_global, 0, p.timers_per_owner, 0),
            policy: p,
        })
    }
    pub fn job(self: &Arc<Self>, owner: Owner, bytes: usize) -> Result<JobLease, AdmissionError> {
        if bytes > self.policy.input_item_bytes {
            return Err(AdmissionError::PayloadTooLarge);
        }
        let input = self.jobs.acquire(owner.clone(), 1, bytes)?;
        let result = self.results.acquire(None, 1, self.policy.failure_bytes)?;
        Ok(JobLease {
            input,
            result,
            cancel: CancelHandle {
                owner,
                ..Default::default()
            },
            policy: self.policy.clone(),
            domain: self.clone(),
        })
    }
}
static DOMAIN: OnceLock<Arc<Domain>> = OnceLock::new();
pub(crate) fn domain() -> &'static Arc<Domain> {
    DOMAIN.get_or_init(|| Domain::new(policy().clone()))
}
#[derive(Clone, Default)]
pub(crate) struct CancelHandle {
    state: Arc<CancelState>,
    pub owner: Owner,
}
#[derive(Default)]
struct CancelState {
    cancelled: AtomicBool,
    notify: tokio::sync::Notify,
}
impl CancelHandle {
    pub fn cancel(&self) {
        self.state.cancelled.store(true, Ordering::Release);
        self.state.notify.notify_waiters();
    }
    pub fn cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::Acquire)
    }
    pub async fn wait(&self) {
        loop {
            let notified = self.state.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.cancelled() {
                return;
            }
            notified.await;
        }
    }
}

pub(crate) struct JobLease {
    input: Lease,
    result: Lease,
    pub cancel: CancelHandle,
    pub policy: AsyncPolicy,
    domain: Arc<Domain>,
}
impl JobLease {
    /// Nonblocking growth: a builder never waits while retaining partial output.
    pub fn grow(&mut self, bytes: usize) -> Result<(), AdmissionError> {
        if self.cancel.cancelled() {
            return Err(AdmissionError::Cancelled);
        }
        self.result.grow(bytes)
    }
    pub fn bytes(&self) -> usize {
        self.result.bytes
    }
    pub fn input_grow(&mut self, bytes: usize) -> Result<(), AdmissionError> {
        if bytes
            > self
                .policy
                .input_item_bytes
                .saturating_sub(self.input.bytes)
        {
            return Err(AdmissionError::PayloadTooLarge);
        }
        self.input.grow(bytes)
    }
    pub fn discard_result(&mut self) {
        self.result
            .shrink(self.result.bytes.saturating_sub(self.policy.failure_bytes));
    }
}
impl Lease {
    pub fn grow(&mut self, bytes: usize) -> Result<(), AdmissionError> {
        let mut extra = self.budget.acquire(self.owner.clone(), 0, bytes)?;
        self.bytes += extra.bytes;
        extra.bytes = 0;
        Ok(())
    }
    pub fn shrink(&mut self, bytes: usize) {
        let bytes = bytes.min(self.bytes);
        self.bytes -= bytes;
        let mut s = self.budget.state.lock().unwrap();
        s.total.bytes -= bytes;
        s.owners.get_mut(&self.owner).unwrap().bytes -= bytes;
    }
}
/// Includes work whose resolver was dropped; transfer into completion retains the same lease.
pub(crate) fn obligations() -> usize {
    let d = domain();
    d.jobs.snapshot().items + d.sockets.snapshot().items + d.inbound.snapshot().items
}
/// Owned lossy conversion; callers reserve final bytes before entering this allocation.
pub(crate) fn utf8_lossy_owned(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(utf8_lossy_len(bytes));
    for chunk in bytes.utf8_chunks() {
        output.push_str(chunk.valid());
        if !chunk.invalid().is_empty() {
            output.push('\u{fffd}');
        }
    }
    output
}

pub(crate) fn diagnostic(s: String) -> String {
    diagnostic_limit(s, policy().failure_bytes / 2)
}
pub(crate) fn diagnostic_limit(mut s: String, cap: usize) -> String {
    if s.len() > cap {
        let mut n = cap;
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        s.truncate(n);
        s.shrink_to_fit();
    }
    s
}
/// Exact final UTF-8 length without allocating the lossy output.
pub(crate) fn utf8_lossy_len(mut bytes: &[u8]) -> usize {
    let mut n = 0usize;
    loop {
        match std::str::from_utf8(bytes) {
            Ok(s) => return n.saturating_add(s.len()),
            Err(e) => {
                n = n.saturating_add(e.valid_up_to()).saturating_add(3);
                let Some(bad) = e.error_len() else { return n };
                bytes = &bytes[e.valid_up_to() + bad..];
            }
        }
    }
}

/// Lifetime + pre-reserved control storage. A worker can publish connect plus terminal without
/// competing with inbound data. Clones travel with every staged callback until materialization ends.
pub(crate) struct SocketResources {
    _lifetime: Lease,
    pub job: JobLease,
    pub domain: Arc<Domain>,
}
impl SocketResources {
    pub fn new(owner: Owner, mut job: JobLease) -> Result<Arc<Self>, AdmissionError> {
        let domain = job.domain.clone();
        let lifetime = domain.sockets.acquire(owner, 1, 0)?;
        job.grow(domain.policy.failure_bytes.saturating_mul(2))?;
        Ok(Arc::new(Self {
            _lifetime: lifetime,
            job,
            domain,
        }))
    }
}
pub(crate) struct Retention {
    _data: Option<Lease>,
    _socket: Option<Arc<SocketResources>>,
}
pub(crate) trait Signal: Send {
    fn data_bytes(&self) -> Option<usize>;
    fn retain(&mut self, r: Arc<Retention>);
    fn bound_diagnostics(&mut self, cap: usize);
}
pub(crate) struct SocketSender<T> {
    tx: std::sync::mpsc::Sender<T>,
    socket: Option<Arc<SocketResources>>,
}
impl<T> Clone for SocketSender<T> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            socket: self.socket.clone(),
        }
    }
}
pub(crate) fn signal_channel<T>() -> (SocketSender<T>, std::sync::mpsc::Receiver<T>) {
    let (tx, rx) = std::sync::mpsc::channel();
    (SocketSender { tx, socket: None }, rx)
}
impl<T: Signal> SocketSender<T> {
    pub async fn inbound(&self, bytes: usize) -> Result<Lease, AdmissionError> {
        let d = self
            .socket
            .as_ref()
            .map(|r| &r.domain)
            .unwrap_or_else(|| domain());
        inbound_in(d, bytes).await
    }
    pub fn owned(&self, socket: Arc<SocketResources>) -> Self {
        Self {
            tx: self.tx.clone(),
            socket: Some(socket),
        }
    }
    /// Control publication has its own admitted connection reservation; data can never consume it.
    pub fn send(&self, mut value: T) -> Result<(), ()> {
        value.bound_diagnostics(
            self.socket
                .as_ref()
                .map_or(policy().failure_bytes, |r| r.domain.policy.failure_bytes)
                / 2,
        );
        let data = match value.data_bytes() {
            Some(n) => Some(
                self.socket
                    .as_ref()
                    .map(|r| &r.domain)
                    .unwrap_or_else(|| domain())
                    .inbound
                    .acquire(None, 1, n)
                    .map_err(|_| ())?,
            ),
            None => None,
        };
        self.send_reserved(value, data)
    }
    pub fn send_reserved(&self, mut value: T, data: Option<Lease>) -> Result<(), ()> {
        value.bound_diagnostics(
            self.socket
                .as_ref()
                .map_or(policy().failure_bytes, |r| r.domain.policy.failure_bytes)
                / 2,
        );
        value.retain(Arc::new(Retention {
            _data: data,
            _socket: self.socket.clone(),
        }));
        if let Some(socket) = &self.socket {
            let _guard = delivery_guard(&socket.job.cancel).ok_or(())?;
            self.tx.send(value).map_err(|_| ())
        } else {
            self.tx.send(value).map_err(|_| ())
        }
    }
}
/// Wait only with no partially built result. Callers select this socket capacity wait against
/// their independent control receiver, so close/shutdown is never blocked by inbound pressure.
async fn inbound_in(d: &Arc<Domain>, bytes: usize) -> Result<Lease, AdmissionError> {
    loop {
        let notified = CAPACITY.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        match d.inbound.acquire(None, 1, bytes) {
            Ok(l) => return Ok(l),
            Err(AdmissionError::QueueFull) => notified.await,
            Err(e) => return Err(e),
        }
    }
}
static CAPACITY: tokio::sync::Notify = tokio::sync::Notify::const_new();
pub(crate) trait PayloadSize {
    fn bytes(&self) -> usize;
}
struct Outbound<T> {
    value: T,
    _global: Lease,
    _local: Lease,
}
pub(crate) struct OutSender<T> {
    tx: tokio::sync::mpsc::UnboundedSender<Outbound<T>>,
    budget: Arc<Budget>,
    global: Arc<Budget>,
}
pub(crate) struct OutReceiver<T> {
    rx: tokio::sync::mpsc::UnboundedReceiver<Outbound<T>>,
    held: Option<(Lease, Lease)>,
}
pub(crate) struct OutReservation {
    global: Lease,
    local: Lease,
}
pub(crate) fn out_channel<T>() -> (OutSender<T>, OutReceiver<T>) {
    out_channel_in(domain())
}
pub(crate) fn out_channel_in<T>(d: &Arc<Domain>) -> (OutSender<T>, OutReceiver<T>) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let p = &d.policy;
    (
        OutSender {
            tx,
            budget: Budget::new(
                p.socket_out_items,
                p.socket_out_bytes,
                p.socket_out_items,
                p.socket_out_bytes,
            ),
            global: d.outbound.clone(),
        },
        OutReceiver { rx, held: None },
    )
}
impl<T: PayloadSize> OutSender<T> {
    pub fn reserve(&self, bytes: usize) -> Result<OutReservation, AdmissionError> {
        let local = self.budget.acquire(None, 1, bytes)?;
        let global = self.global.acquire(None, 1, bytes)?;
        Ok(OutReservation { global, local })
    }
    pub fn send(&self, value: T) -> Result<(), ()> {
        let r = self.reserve(value.bytes()).map_err(|_| ())?;
        self.send_reserved(value, r)
    }
    pub fn send_reserved(&self, value: T, r: OutReservation) -> Result<(), ()> {
        self.tx
            .send(Outbound {
                value,
                _global: r.global,
                _local: r.local,
            })
            .map_err(|_| ())
    }
}
impl<T> OutReceiver<T> {
    pub async fn recv(&mut self) -> Option<T> {
        self.held = None;
        let Outbound {
            value,
            _global,
            _local,
        } = self.rx.recv().await?;
        self.held = Some((_global, _local));
        Some(value)
    }
    pub fn try_recv(&mut self) -> Result<T, tokio::sync::mpsc::error::TryRecvError> {
        self.held = None;
        let Outbound {
            value,
            _global,
            _local,
        } = self.rx.try_recv()?;
        self.held = Some((_global, _local));
        Ok(value)
    }
}

impl<T> Clone for OutSender<T> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            budget: self.budget.clone(),
            global: self.global.clone(),
        }
    }
}

/// Polling and logical delivery have distinct finite limits. A socket signal staged for later
/// callbacks is a poll only; one promise settlement, timer, or callback event is a delivery.
pub(crate) struct FrameBudget {
    pub items: usize,
    pub bytes: usize,
    pub polls: usize,
    max_items: usize,
    max_bytes: usize,
    poll_limit: usize,
    start: Instant,
    soft: Duration,
}
impl FrameBudget {
    pub fn new(p: &AsyncPolicy) -> Self {
        Self {
            items: 0,
            bytes: 0,
            polls: 0,
            max_items: p.frame_items,
            max_bytes: p.frame_bytes,
            poll_limit: p.frame_poll_items,
            start: Instant::now(),
            soft: Duration::from_micros(p.frame_soft_us),
        }
    }
    pub fn can(&self, bytes: usize) -> bool {
        self.items < self.max_items
            && (self.items == 0
                || (self.start.elapsed() < self.soft
                    && bytes <= self.max_bytes.saturating_sub(self.bytes)))
            && (self.items == 0 || self.bytes < self.max_bytes)
    }
    pub fn spend(&mut self, bytes: usize) {
        self.items += 1;
        self.bytes = self.bytes.saturating_add(bytes);
    }
    pub fn poll(&mut self) -> bool {
        if self.polls >= self.poll_limit || (self.polls > 0 && self.start.elapsed() >= self.soft) {
            false
        } else {
            self.polls += 1;
            true
        }
    }
}
thread_local! {
    static FRAME:std::cell::RefCell<Option<FrameBudget>>=const {std::cell::RefCell::new(None)};
    static PRE_RESERVE:std::cell::Cell<usize>=const {std::cell::Cell::new(0)};
    static PRE_TURN:std::cell::Cell<bool>=const {std::cell::Cell::new(false)};
}
static LAST_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static MAX_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(crate) fn begin_frame(callbacks: bool) {
    begin_frame_with(policy(), callbacks);
}
pub(crate) fn begin_frame_with(p: &AsyncPolicy, callbacks: bool) {
    let reserve = if callbacks {
        PRE_TURN.with(|v| {
            let old = v.get();
            v.set(!old);
            if old {
                0
            } else {
                p.frame_items
            }
        })
    } else {
        0
    };
    PRE_RESERVE.with(|v| v.set(reserve));
    FRAME.with(|f| *f.borrow_mut() = Some(FrameBudget::new(p)));
}
pub(crate) fn poll_frame() -> bool {
    FRAME.with(|f| f.borrow_mut().as_mut().is_some_and(|b| b.poll()))
}
pub(crate) fn can_deliver(bytes: usize, pre: bool) -> bool {
    FRAME.with(|f| {
        f.borrow().as_ref().is_none_or(|b| {
            b.can(bytes) && (!pre || b.items + PRE_RESERVE.with(|v| v.get()) < b.max_items)
        })
    })
}
pub(crate) fn deliver(bytes: usize) {
    FRAME.with(|f| {
        if let Some(b) = f.borrow_mut().as_mut() {
            b.spend(bytes);
        }
    });
}
pub(crate) fn finish_frame() {
    FRAME.with(|f| {
        if let Some(b) = f.borrow().as_ref() {
            let ns = b.start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
            LAST_NS.store(ns, Ordering::Relaxed);
            MAX_NS.fetch_max(ns, Ordering::Relaxed);
        }
    });
}
pub(crate) fn metrics() -> serde_json::Value {
    let d = domain();
    let frame = FRAME
        .with(|f| {
            f.borrow()
                .as_ref()
                .map(|b| serde_json::json!({"items":b.items,"bytes":b.bytes,"polls":b.polls}))
        })
        .unwrap_or_default();
    serde_json::json!({"jobs":d.jobs.snapshot(),"completion":d.results.snapshot(),"sockets":d.sockets.snapshot(),"sqlite":d.sqlite.snapshot(),"pools":d.pools.snapshot(),"inbound":d.inbound.snapshot(),"outbound":d.outbound.snapshot(),"timers":d.timers.snapshot(),"frame":frame,"queued":queued_metrics(),"lastNs":LAST_NS.load(Ordering::Relaxed),"maxNs":MAX_NS.load(Ordering::Relaxed)})
}

static DELIVERY: Mutex<bool> = Mutex::new(true);
pub(crate) fn resume_delivery() {
    *DELIVERY.lock().unwrap() = true;
}
pub(crate) fn delivery_guard(
    cancel: &CancelHandle,
) -> Option<std::sync::MutexGuard<'static, bool>> {
    let guard = DELIVERY.lock().unwrap();
    if *guard && !cancel.cancelled() {
        Some(guard)
    } else {
        None
    }
}
pub(crate) fn stop_delivery(cleanup: impl FnOnce()) {
    let mut guard = DELIVERY.lock().unwrap();
    *guard = false;
    cleanup();
}

static QUEUES: [std::sync::atomic::AtomicUsize; 5] =
    [const { std::sync::atomic::AtomicUsize::new(0) }; 5];
pub(crate) struct QueueTicket(Option<usize>);
impl QueueTicket {
    pub fn new(source: usize) -> Self {
        QUEUES[source].fetch_add(1, Ordering::Relaxed);
        Self(Some(source))
    }
    pub fn dequeue(&mut self) {
        if let Some(source) = self.0.take() {
            QUEUES[source].fetch_sub(1, Ordering::Relaxed);
        }
    }
}
impl Drop for QueueTicket {
    fn drop(&mut self) {
        self.dequeue();
    }
}
pub(crate) fn queued_metrics() -> serde_json::Value {
    serde_json::json!({"worker":QUEUES[0].load(Ordering::Relaxed),"http":QUEUES[1].load(Ordering::Relaxed),"db":QUEUES[2].load(Ordering::Relaxed),"ws":QUEUES[3].load(Ordering::Relaxed),"net":QUEUES[4].load(Ordering::Relaxed)})
}

#[cfg(test)]
mod pressure_tests {
    use super::*;
    fn tiny() -> AsyncPolicy {
        AsyncPolicy {
            jobs_global: 2,
            jobs_per_owner: 2,
            input_bytes: 8,
            owner_input_bytes: 8,
            input_item_bytes: 8,
            completion_bytes: 16,
            failure_bytes: 2,
            ..Default::default()
        }
    }
    #[test]
    fn partial_result_builders_never_wait_and_failure_capacity_survives() {
        let d = Domain::new(tiny());
        let mut a = d.job(None, 4).unwrap();
        let mut b = d.job(None, 4).unwrap();
        a.grow(6).unwrap();
        b.grow(6).unwrap();
        assert_eq!(a.grow(1).err(), Some(AdmissionError::QueueFull));
        a.discard_result();
        assert_eq!(d.results.snapshot().bytes, 10);
        b.grow(1).unwrap();
        drop(b);
        drop(a);
        assert_eq!(
            (d.jobs.snapshot().items, d.results.snapshot().bytes),
            (0, 0)
        );
        let mut small = d.job(None, 1).unwrap();
        small.grow(1).unwrap();
    }
    #[test]
    fn old_generation_keeps_global_bytes_until_producer_exit() {
        let d = Domain::new(tiny());
        let lease = d.job(Some(("p".into(), 1)), 8).unwrap();
        let cancel = lease.cancel.clone();
        let (entered, wait) = std::sync::mpsc::channel();
        let (release, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            entered.send(()).unwrap();
            rx.recv().unwrap();
            drop(lease);
        });
        wait.recv().unwrap();
        cancel.cancel();
        assert!(d.job(Some(("p".into(), 2)), 1).is_err());
        assert_eq!(d.jobs.snapshot().bytes, 8);
        release.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(d.jobs.snapshot().bytes, 0);
        assert!(d.job(Some(("p".into(), 2)), 1).is_ok());
    }
    #[test]
    fn impossible_items_reject_without_consuming_any_capacity() {
        let d = Domain::new(tiny());
        assert_eq!(
            d.job(None, 9).err().unwrap(),
            AdmissionError::PayloadTooLarge
        );
        assert_eq!(d.jobs.snapshot().items, 0);
        assert_eq!(d.results.snapshot().items, 0);
        let mut l = d.job(None, 0).unwrap();
        assert_eq!(l.grow(17).err(), Some(AdmissionError::PayloadTooLarge));
        l.discard_result();
        assert_eq!(d.results.snapshot().bytes, 2);
    }
    #[test]
    fn utf8_final_size_counts_replacement_expansion() {
        for bytes in [
            &b"abc"[..],
            &[0xff],
            &[0xe2, 0x82],
            &[0xff, b'a', 0xff],
            &[0xf0, 0x9f, 0x98, 0x80],
        ] {
            assert_eq!(utf8_lossy_len(bytes), String::from_utf8_lossy(bytes).len());
        }
        assert_eq!(utf8_lossy_len(&[0xff; 10]), 30);
    }
    #[test]
    fn lossy_conversion_peak_fits_the_preallocated_reservation() {
        let d = Domain::new(AsyncPolicy::default());
        let raw = vec![0xff; 1 << 20];
        let mut lease = d.job(None, 0).unwrap();
        lease.grow(raw.capacity()).unwrap();
        lease.grow(utf8_lossy_len(&raw)).unwrap();
        let output = utf8_lossy_owned(&raw);
        assert_eq!(output.len(), 3 << 20);
        assert!(
            raw.capacity() + output.capacity() <= lease.bytes(),
            "conversion capacity exceeded its reservation"
        );
        assert_eq!(output.capacity(), output.len());
    }

    #[test]
    fn oversized_delivery_is_alone_even_when_more_rounds_are_available() {
        let p = AsyncPolicy {
            frame_items: 3,
            frame_bytes: 2,
            ..Default::default()
        };
        let mut b = FrameBudget::new(&p);
        assert!(b.can(3));
        b.spend(3);
        assert!(!b.can(0));
        assert!(!b.can(3));
        let mut b = FrameBudget::new(&p);
        b.spend(1);
        assert!(!b.can(3));
        assert!(b.can(1));
    }
    #[test]
    fn one_item_frames_alternate_pre_and_callback_progress() {
        let p = AsyncPolicy {
            frame_items: 1,
            frame_bytes: 1,
            ..Default::default()
        };
        let mut pre = 0;
        let mut post = 0;
        for _ in 0..12 {
            begin_frame_with(&p, true);
            if can_deliver(1, true) {
                pre += 1;
                deliver(1);
            }
            if can_deliver(1, false) {
                post += 1;
                deliver(1);
            }
            assert!(!can_deliver(0, false));
        }
        assert_eq!((pre, post), (6, 6));
    }
    #[test]
    fn malformed_policy_is_all_or_nothing_and_checks_reservations() {
        assert!(AsyncPolicy::default().validate());
        assert!(serde_json::from_str::<AsyncPolicy>(r#"{"unknown":1}"#).is_err());
        let mut p = AsyncPolicy::default();
        p.completion_bytes = p.jobs_global * p.failure_bytes - 1;
        assert!(!p.validate());
        p = AsyncPolicy::default();
        p.frame_items = 0;
        assert!(!p.validate());
    }
}
