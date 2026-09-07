//! Engine-generic cancellable raw TCP + UDP socket engine. V8 routing stays in the adapter below.
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;
use crate::async_limits::{signal_channel as channel, SocketSender as Sender};
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
const READ_CAP: usize = 64 * 1024;
#[derive(Clone, Debug)]
pub struct NetTerminal {
    pub error: Option<String>,
}
pub enum NetSignalKind {
    Connected,
    Bound,
    ConnectFailed(String),
    Data(Vec<u8>),
    Datagram { from: String, data: Vec<u8> },
    Terminal(NetTerminal),
}
pub struct NetSignal {
    pub conn_id: u64,
    pub kind: NetSignalKind,
    retention: Option<Arc<crate::async_limits::Retention>>,
    queue:Option<crate::async_limits::QueueTicket>,
}
enum NetCommand {
    Send(Vec<u8>),
    SendTo(String, u16, Vec<u8>),
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Control {
    Open,
    CloseRequested,
    Shutdown,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum ConnPhase {
    Connecting,
    Open,
    Closing,
    TerminalPending,
}
#[derive(Clone, Copy)]
struct SocketDeadlines {
    connect: Duration,
    close_grace: Duration,
}
impl Default for SocketDeadlines {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(10),
            close_grace: Duration::from_secs(1),
        }
    }
}
struct Conn {
    resources: Option<Arc<crate::async_limits::SocketResources>>,
    data_tx: crate::async_limits::OutSender<NetCommand>,
    control_tx: tokio::sync::watch::Sender<Control>,
    phase: ConnPhase,
    owner: String,
    owner_generation: u64,
}
struct Engine {
    sig_tx: Sender<NetSignal>,
    sig_rx: Mutex<Receiver<NetSignal>>,
    conns: Mutex<HashMap<u64, Conn>>,
}
static ENGINE: OnceLock<Engine> = OnceLock::new();
static ACTIVE_WORKERS: AtomicUsize = AtomicUsize::new(0);
struct WorkerGuard;
impl WorkerGuard {
    fn new() -> Self {
        ACTIVE_WORKERS.fetch_add(1, Ordering::SeqCst);
        Self
    }
}
impl Drop for WorkerGuard {
    fn drop(&mut self) {
        ACTIVE_WORKERS.fetch_sub(1, Ordering::SeqCst);
    }
}
fn engine() -> &'static Engine {
    ENGINE.get_or_init(|| {
        let (tx, rx) = channel();
        Engine {
            sig_tx: tx,
            sig_rx: Mutex::new(rx),
            conns: Mutex::new(HashMap::new()),
        }
    })
}
fn publish_control(tx: &tokio::sync::watch::Sender<Control>, next: Control) {
    tx.send_if_modified(|v| {
        if *v < next {
            *v = next;
            true
        } else {
            false
        }
    });
}
fn control(rx: &tokio::sync::watch::Receiver<Control>) -> Control {
    *rx.borrow()
}

enum Fair<L, R> {
    Left(L),
    Right(R),
}

async fn fair_pair<A, B>(left: A, right: B) -> Fair<A::Output, B::Output>
where
    A: std::future::Future,
    B: std::future::Future,
{
    tokio::pin!(left);
    tokio::pin!(right);
    tokio::select! {
        value = &mut left => Fair::Left(value),
        value = &mut right => Fair::Right(value),
    }
}

async fn await_connect<F, T, E>(
    attempt: F,
    control_rx: &mut tokio::sync::watch::Receiver<Control>,
    deadline: Duration,
) -> Option<Result<T, String>>
where
    F: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    tokio::pin!(attempt);
    tokio::select! { biased;
        _ = control_rx.changed() => None,
        result = tokio::time::timeout(deadline, &mut attempt) => Some(match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(error.to_string()),
            Err(_) => Err(format!("connect did not complete within {}ms", deadline.as_millis())),
        }),
    }
}

async fn run_tcp_connected<R, W>(
    id: u64,
    mut rd: R,
    mut wr: W,
    mut data_rx: crate::async_limits::OutReceiver<NetCommand>,
    control_rx: &mut tokio::sync::watch::Receiver<Control>,
    sig_tx: Sender<NetSignal>,
    deadlines: SocketDeadlines,
) -> Option<NetTerminal>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    async fn graceful<W: AsyncWrite + Unpin>(
        wr: &mut W,
        data_rx: &mut crate::async_limits::OutReceiver<NetCommand>,
        control_rx: &mut tokio::sync::watch::Receiver<Control>,
        deadline: tokio::time::Instant,
    ) -> Option<Result<(), String>> {
        let operation = tokio::time::timeout_at(deadline, async {
            while let Ok(NetCommand::Send(bytes)) = data_rx.try_recv() {
                wr.write_all(&bytes).await?;
            }
            wr.shutdown().await
        });
        tokio::pin!(operation);
        tokio::select! { biased;
            _ = control_rx.changed() => None,
            result = &mut operation => Some(match result {
                Ok(result) => result.map_err(|error| error.to_string()),
                Err(_) => Err("graceful close deadline exceeded".into()),
            }),
        }
    }

    let mut buf = vec![0; READ_CAP];
    loop {
        enum Ready {
            Control(Result<(), tokio::sync::watch::error::RecvError>),
            Read(std::io::Result<usize>),
            Command(Option<NetCommand>),
        }
        let ready = if control(control_rx) >= Control::CloseRequested {
            let _ = control_rx.borrow_and_update();
            Ready::Control(Ok(()))
        } else {
            let io = async {
                match fair_pair(rd.read(&mut buf), data_rx.recv()).await {
                    Fair::Left(result) => Ready::Read(result),
                    Fair::Right(command) => Ready::Command(command),
                }
            };
            tokio::pin!(io);
            tokio::select! { biased;
                changed = control_rx.changed() => Ready::Control(changed),
                ready = &mut io => ready,
            }
        };
        match ready {
            Ready::Control(changed) => {
                if changed.is_err() || control(&control_rx) == Control::Shutdown {
                    return None;
                }
                let deadline = tokio::time::Instant::now() + deadlines.close_grace;
                return graceful(&mut wr, &mut data_rx, control_rx, deadline)
                    .await
                    .map(|result| NetTerminal {
                        error: result.err(),
                    });
            }
            Ready::Read(result) => match result {
                Ok(0) => return Some(NetTerminal { error: None }),
                Ok(n) => {
                    let permit = tokio::select! { biased;
                        _=control_rx.changed()=>{if control(control_rx)==Control::Shutdown {return None;}continue;},
                        p=sig_tx.inbound(n.saturating_add(64))=>match p {Ok(p)=>p,Err(e)=>return Some(NetTerminal {error:Some(e.to_string())})},
                    };
                    let _ = sig_tx.send_reserved(
                        NetSignal {
                            retention: None,
                            queue: None,
                            conn_id: id,
                            kind: NetSignalKind::Data(buf[..n].to_vec()),
                        },
                        Some(permit),
                    );
                }
                Err(e) => {
                    return Some(NetTerminal {
                        error: Some(e.to_string()),
                    })
                }
            },
            Ready::Command(command) => match command {
                Some(NetCommand::Send(bytes)) => {
                    let write = wr.write_all(&bytes);
                    tokio::pin!(write);
                    tokio::select! { biased;
                        changed = control_rx.changed() => {
                            if changed.is_err() || control(&control_rx) == Control::Shutdown {
                                return None;
                            }
                            let deadline = tokio::time::Instant::now() + deadlines.close_grace;
                            let first = tokio::time::timeout_at(deadline, &mut write);
                            tokio::pin!(first);
                            let first = tokio::select! { biased;
                                _ = control_rx.changed() => return None,
                                result = &mut first => result,
                            };
                            match first {
                                Ok(Ok(())) => {}
                                Ok(Err(e)) => return Some(NetTerminal { error: Some(e.to_string()) }),
                                Err(_) => return Some(NetTerminal { error: Some("graceful close deadline exceeded".into()) }),
                            }
                            drop(write);
                            return graceful(&mut wr, &mut data_rx, control_rx, deadline)
                                .await
                                .map(|result| NetTerminal { error: result.err() });
                        }
                        result = &mut write => if let Err(e) = result {
                            return Some(NetTerminal { error: Some(e.to_string()) });
                        }
                    }
                }
                Some(NetCommand::SendTo(..)) => {}
                None => return None,
            },
        }
    }
}
#[cfg(test)]
fn insert_conn(
    id: u64,
    owner: String,
    generation: u64,
) -> (
    crate::async_limits::OutReceiver<NetCommand>,
    tokio::sync::watch::Receiver<Control>,
) {
    let job = crate::async_limits::domain()
        .job(Some((owner.clone(), generation)), 0)
        .unwrap();
    let resources =
        crate::async_limits::SocketResources::new(Some((owner.clone(), generation)), job).unwrap();
    insert_conn_reserved(id, owner, generation, resources)
}
fn insert_conn_reserved(
    id: u64,
    owner: String,
    generation: u64,
    resources: Arc<crate::async_limits::SocketResources>,
) -> (
    crate::async_limits::OutReceiver<NetCommand>,
    tokio::sync::watch::Receiver<Control>,
) {
    let (tx, rx) = crate::async_limits::out_channel_in(&resources.domain);
    let (ctx, crx) = tokio::sync::watch::channel(Control::Open);
    engine().conns.lock().unwrap().insert(
        id,
        Conn {
            resources: Some(resources),
            data_tx: tx,
            control_tx: ctx,
            phase: ConnPhase::Connecting,
            owner,
            owner_generation: generation,
        },
    );
    (rx, crx)
}
#[cfg(test)]
pub fn connect_tcp(id: u64, host: String, port: u16, owner: String) {
    let lease = crate::async_limits::domain()
        .job(Some((owner.clone(), 0)), 0)
        .unwrap();
    connect_tcp_owned(id, host, port, owner, 0, lease).unwrap()
}
pub(crate) fn connect_tcp_owned(
    id: u64,
    host: String,
    port: u16,
    owner: String,
    generation: u64,
    lease: crate::async_limits::JobLease,
) -> Result<(), String> {
    connect_tcp_with_deadlines(
        id,
        host,
        port,
        owner,
        generation,
        SocketDeadlines::default(),
        lease,
    )
}
fn connect_tcp_with_deadlines(
    id: u64,
    host: String,
    port: u16,
    owner: String,
    generation: u64,
    d: SocketDeadlines,
    lease: crate::async_limits::JobLease,
) -> Result<(), String> {
    spawn_tcp_reserved(
        id,
        owner,
        generation,
        d,
        tokio::net::TcpStream::connect((host, port)),
        lease,
    )
}

#[cfg(test)]
fn spawn_tcp_attempt<F, S, E>(
    id: u64,
    owner: String,
    generation: u64,
    d: SocketDeadlines,
    attempt: F,
) where
    F: std::future::Future<Output = Result<S, E>> + Send + 'static,
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    let lease = crate::async_limits::domain()
        .job(Some((owner.clone(), generation)), 0)
        .unwrap();
    spawn_tcp_reserved(id, owner, generation, d, attempt, lease).unwrap();
}
fn spawn_tcp_reserved<F, S, E>(
    id: u64,
    owner: String,
    generation: u64,
    d: SocketDeadlines,
    attempt: F,
    lease: crate::async_limits::JobLease,
) -> Result<(), String>
where
    F: std::future::Future<Output = Result<S, E>> + Send + 'static,
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    let resources =
        crate::async_limits::SocketResources::new(Some((owner.clone(), generation)), lease)
            .map_err(|e| e.to_string())?;
    let (rx, mut crx) = insert_conn_reserved(id, owner, generation, resources.clone());
    let tx = engine().sig_tx.owned(resources);
    crate::http::spawn(async move {
        let _g = WorkerGuard::new();
        let stream = match await_connect(attempt, &mut crx, d.connect).await {
            Some(Ok(stream)) => stream,
            Some(Err(error)) => {
                let _ = tx.send(NetSignal {
                    retention: None,
                    queue: None,
                    conn_id: id,
                    kind: NetSignalKind::ConnectFailed(error),
                });
                return;
            }
            None => return,
        };
        if control(&crx) == Control::Shutdown {
            return;
        }
        let _ = tx.send(NetSignal {
            retention: None,
            queue: None,
            conn_id: id,
            kind: NetSignalKind::Connected,
        });
        let (rd, wr) = tokio::io::split(stream);
        let terminal = run_tcp_connected(id, rd, wr, rx, &mut crx, tx.clone(), d).await;
        if let Some(terminal) = terminal.filter(|_| control(&crx) != Control::Shutdown) {
            let _ = tx.send(NetSignal {
                retention: None,
                queue: None,
                conn_id: id,
                kind: NetSignalKind::Terminal(terminal),
            });
        }
    });
    Ok(())
}
#[cfg(test)]
pub fn bind_udp(id: u64, owner: String) {
    let lease = crate::async_limits::domain()
        .job(Some((owner.clone(), 0)), 0)
        .unwrap();
    bind_udp_owned(id, owner, 0, lease).unwrap()
}
pub(crate) fn bind_udp_owned(
    id: u64,
    owner: String,
    generation: u64,
    lease: crate::async_limits::JobLease,
) -> Result<(), String> {
    bind_udp_with_deadlines(id, owner, generation, SocketDeadlines::default(), lease)
}
fn bind_udp_with_deadlines(
    id: u64,
    owner: String,
    generation: u64,
    d: SocketDeadlines,
    lease: crate::async_limits::JobLease,
) -> Result<(), String> {
    let resources =
        crate::async_limits::SocketResources::new(Some((owner.clone(), generation)), lease)
            .map_err(|e| e.to_string())?;
    let (mut rx, mut crx) = insert_conn_reserved(id, owner, generation, resources.clone());
    let tx = engine().sig_tx.owned(resources);
    crate::http::spawn(async move {
        let _g = WorkerGuard::new();
        let bind = tokio::net::UdpSocket::bind("0.0.0.0:0");
        tokio::pin!(bind);
        let sock = tokio::select! {biased;_=crx.changed()=>return,r=&mut bind=>match r{Ok(s)=>s,Err(e)=>{let _=tx.send(NetSignal { retention: None, queue:None,conn_id:id,kind:NetSignalKind::ConnectFailed(e.to_string())});return}}};
        if control(&crx) == Control::Shutdown {
            return;
        }
        let _ = tx.send(NetSignal {
            retention: None,
            queue: None,
            conn_id: id,
            kind: NetSignalKind::Bound,
        });
        let mut buf = vec![0; READ_CAP];
        let terminal = loop {
            enum Ready {
                Control(Result<(), tokio::sync::watch::error::RecvError>),
                Receive(std::io::Result<(usize, std::net::SocketAddr)>),
                Command(Option<NetCommand>),
            }
            let ready = if control(&crx) >= Control::CloseRequested {
                let _ = crx.borrow_and_update();
                Ready::Control(Ok(()))
            } else {
                let io = async {
                    match fair_pair(sock.recv_from(&mut buf), rx.recv()).await {
                        Fair::Left(result) => Ready::Receive(result),
                        Fair::Right(command) => Ready::Command(command),
                    }
                };
                tokio::pin!(io);
                tokio::select! { biased;
                    changed = crx.changed() => Ready::Control(changed),
                    ready = &mut io => ready,
                }
            };
            match ready {
                Ready::Control(changed) => {
                    if changed.is_err() || control(&crx) == Control::Shutdown {
                        return;
                    }
                    let deadline = tokio::time::Instant::now() + d.close_grace;
                    let drain = tokio::time::timeout_at(deadline, async {
                        while let Ok(NetCommand::SendTo(host, port, bytes)) = rx.try_recv() {
                            sock.send_to(&bytes, (host.as_str(), port)).await?;
                        }
                        Ok::<(), std::io::Error>(())
                    });
                    tokio::pin!(drain);
                    let result = tokio::select! { biased;
                        _ = crx.changed() => return,
                        result = &mut drain => result,
                    };
                    break NetTerminal {
                        error: match result {
                            Ok(Ok(())) => None,
                            Ok(Err(error)) => Some(error.to_string()),
                            Err(_) => Some("graceful close deadline exceeded".into()),
                        },
                    };
                }
                Ready::Receive(result) => match result {
                    Ok((n, from)) => {
                        let permit = tokio::select! { biased;
                            _=crx.changed()=> {if control(&crx)==Control::Shutdown {return;}continue;},
                            p=tx.inbound(n.saturating_add(128))=>match p {Ok(p)=>p,Err(e)=>break NetTerminal {error:Some(e.to_string())}},
                        };
                        let _ = tx.send_reserved(
                            NetSignal {
                                retention: None,
                                queue: None,
                                conn_id: id,
                                kind: NetSignalKind::Datagram {
                                    from: from.to_string(),
                                    data: buf[..n].to_vec(),
                                },
                            },
                            Some(permit),
                        );
                    }
                    Err(error) => {
                        break NetTerminal {
                            error: Some(error.to_string()),
                        }
                    }
                },
                Ready::Command(command) => match command {
                    Some(NetCommand::SendTo(host, port, bytes)) => {
                        let send = sock.send_to(&bytes, (host.as_str(), port));
                        tokio::pin!(send);
                        tokio::select! { biased;
                            changed = crx.changed() => {
                                if changed.is_err() || control(&crx) == Control::Shutdown { return; }
                                let deadline = tokio::time::Instant::now() + d.close_grace;
                                let first = tokio::time::timeout_at(deadline, &mut send);
                                tokio::pin!(first);
                                let first = tokio::select! { biased;
                                    _ = crx.changed() => return,
                                    result = &mut first => result,
                                };
                                match first {
                                    Ok(Ok(_)) => {}
                                    Ok(Err(error)) => break NetTerminal { error: Some(error.to_string()) },
                                    Err(_) => break NetTerminal { error: Some("graceful close deadline exceeded".into()) },
                                }
                                drop(send);
                                let drain = tokio::time::timeout_at(deadline, async {
                                    while let Ok(NetCommand::SendTo(host, port, bytes)) = rx.try_recv() {
                                        sock.send_to(&bytes, (host.as_str(), port)).await?;
                                    }
                                    Ok::<(), std::io::Error>(())
                                });
                                tokio::pin!(drain);
                                let result = tokio::select! { biased;
                                    _ = crx.changed() => return,
                                    result = &mut drain => result,
                                };
                                break NetTerminal { error: match result {
                                    Ok(Ok(())) => None,
                                    Ok(Err(error)) => Some(error.to_string()),
                                    Err(_) => Some("graceful close deadline exceeded".into()),
                                }};
                            }
                            result = &mut send => if let Err(error) = result {
                                break NetTerminal { error: Some(error.to_string()) };
                            }
                        }
                    }
                    Some(NetCommand::Send(_)) => {}
                    None => return,
                },
            }
        };
        if control(&crx) != Control::Shutdown {
            let _ = tx.send(NetSignal {
                retention: None,
                queue: None,
                conn_id: id,
                kind: NetSignalKind::Terminal(terminal),
            });
        }
    });
    Ok(())
}
pub fn send(id: u64, owner: &str, b: Vec<u8>) -> bool {
    let map = engine().conns.lock().unwrap();
    matches!(map.get(&id),Some(c)if c.owner==owner&&c.phase==ConnPhase::Open&&c.data_tx.send(NetCommand::Send(b)).is_ok())
}
pub fn send_to(id: u64, owner: &str, h: String, p: u16, b: Vec<u8>) -> bool {
    if b.len() > 65507 {
        return false;
    }
    let map = engine().conns.lock().unwrap();
    matches!(map.get(&id),Some(c)if c.owner==owner&&c.phase==ConnPhase::Open&&c.data_tx.send(NetCommand::SendTo(h,p,b)).is_ok())
}
pub fn close(id: u64, owner: &str) -> bool {
    let mut map = engine().conns.lock().unwrap();
    let Some(c) = map.get_mut(&id) else {
        return false;
    };
    if c.owner != owner || c.phase != ConnPhase::Open {
        return false;
    }
    c.phase = ConnPhase::Closing;
    publish_control(&c.control_tx, Control::CloseRequested);
    true
}
pub fn is_owner(id: u64, owner: &str) -> bool {
    matches!(engine().conns.lock().unwrap().get(&id),Some(c)if c.owner==owner)
}
pub fn shutdown_conn(id: u64) {
    if let Some(c) = engine().conns.lock().unwrap().remove(&id) {
        if let Some(r) = &c.resources {
            r.job.cancel.cancel();
        }
        publish_control(&c.control_tx, Control::Shutdown);
        purge_conn(id);
        crate::v8host::release_resource(
            &c.owner,
            c.owner_generation,
            &crate::plugin::Resource::NetConn(id),
        );
    }
}
pub fn retire_conn(id: u64) {
    if let Some(c) = engine().conns.lock().unwrap().remove(&id) {
        crate::v8host::release_resource(
            &c.owner,
            c.owner_generation,
            &crate::plugin::Resource::NetConn(id),
        );
    }
}
pub fn drop_conn(id: u64) {
    shutdown_conn(id)
}
pub fn try_recv_signal() -> Option<NetSignal> {
    let mut s = engine().sig_rx.lock().ok()?.try_recv().ok()?;
    s.queue.take();
    Some(s)
}
#[cfg(test)]
pub(crate) fn test_insert_conn(conn_id: u64, owner: String, generation: u64) {
    let _ = insert_conn(conn_id, owner, generation);
}
#[cfg(test)]
pub(crate) fn test_inject_batch(conn_id: u64, data: &[u8], error: &str) {
    let tx = &engine().sig_tx;
    let _ = tx.send(NetSignal {
        retention: None,
        queue: None,
        conn_id,
        kind: NetSignalKind::Connected,
    });
    let _ = tx.send(NetSignal {
        retention: None,
        queue: None,
        conn_id,
        kind: NetSignalKind::Data(data.to_vec()),
    });
    let terminal = NetTerminal {
        error: Some(error.into()),
    };
    let _ = tx.send(NetSignal {
        retention: None,
        queue: None,
        conn_id,
        kind: NetSignalKind::Terminal(terminal.clone()),
    });
    let _ = tx.send(NetSignal {
        retention: None,
        queue: None,
        conn_id,
        kind: NetSignalKind::Terminal(terminal),
    });
}
#[cfg(test)]
pub(crate) fn test_spawn_terminal_worker(conn_id: u64, owner: String, generation: u64) {
    let (stream, peer) = tokio::io::duplex(8);
    drop(peer);
    spawn_tcp_attempt(
        conn_id,
        owner,
        generation,
        SocketDeadlines::default(),
        std::future::ready(Ok::<_, std::io::Error>(stream)),
    );
}
#[cfg(test)]
pub(crate) fn test_spawn_failed_worker(conn_id: u64, owner: String, generation: u64) {
    let (stream, peer) = tokio::io::duplex(8);
    drop(stream);
    drop(peer);
    spawn_tcp_attempt(
        conn_id,
        owner,
        generation,
        SocketDeadlines::default(),
        std::future::ready(Err::<tokio::io::DuplexStream, _>(std::io::Error::other(
            "stress connect failure",
        ))),
    );
}
#[cfg(test)]
pub(crate) fn test_pending_count() -> usize {
    NET_EVENT_PENDING.with(|q| q.borrow().len())
}
#[cfg(test)]
pub(crate) fn test_mux_count(conn_id: u64) -> usize {
    NET_EVENT_MUX.with(|m| {
        ["data", "message", "error", "close"]
            .iter()
            .map(|event| m.borrow().snapshot(&format!("{conn_id}:{event}")).len())
            .sum()
    })
}
#[cfg(test)]
pub(crate) fn active_conn_count() -> usize {
    engine().conns.lock().unwrap().len()
}
#[cfg(test)]
pub(crate) fn active_worker_count() -> usize {
    ACTIVE_WORKERS.load(Ordering::SeqCst)
}

// ---------------------------------------------------------------------------
// V8 adapter — natives, mux, pending queue, post-drain dispatch, teardown.
// The tokio engine above holds no V8 handles. This half owns the isolate-facing
// surface and routes through `fan_out` so the host isolate stays in `v8host`.
// ---------------------------------------------------------------------------

use crate::dispatch::{fan_out, Instrument};
use crate::v8host::{current_plugin, log_warn, set_native, subscribe_into};

enum PendingNetEvent {
    Data(Vec<u8>),
    Datagram { from: String, data: Vec<u8> },
    Closed,
    Errored(String),
}
fn purge_conn(conn_id: u64) {
    NET_EVENT_PENDING.with(|q| q.borrow_mut().retain(|e| e.0 != conn_id));
    NET_EVENT_MUX.with(|m| {
        let mut m = m.borrow_mut();
        for ev in ["data", "message", "error", "close"] {
            m.remove_by_name(&format!("{conn_id}:{ev}"));
        }
    });
}

thread_local! {
    static NET_EVENT_MUX: std::cell::RefCell<crate::channels::Channels<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::channels::Channels::new());
    static NET_EVENT_PENDING: std::cell::RefCell<Vec<(u64, PendingNetEvent, Option<Arc<crate::async_limits::Retention>>)>>
        = std::cell::RefCell::new(Vec::new());
}

fn queue_data(conn_id: u64, b: Vec<u8>, retention: Option<Arc<crate::async_limits::Retention>>) {
    NET_EVENT_PENDING.with(|q| {
        q.borrow_mut()
            .push((conn_id, PendingNetEvent::Data(b), retention))
    });
}
fn queue_datagram(
    conn_id: u64,
    from: String,
    data: Vec<u8>,
    retention: Option<Arc<crate::async_limits::Retention>>,
) {
    NET_EVENT_PENDING.with(|q| {
        q.borrow_mut()
            .push((conn_id, PendingNetEvent::Datagram { from, data }, retention))
    });
}
fn queue_error(conn_id: u64, e: String, retention: Option<Arc<crate::async_limits::Retention>>) {
    NET_EVENT_PENDING.with(|q| {
        q.borrow_mut()
            .push((conn_id, PendingNetEvent::Errored(e), retention))
    });
}
fn queue_close(conn_id: u64, retention: Option<Arc<crate::async_limits::Retention>>) {
    NET_EVENT_PENDING.with(|q| {
        q.borrow_mut()
            .push((conn_id, PendingNetEvent::Closed, retention))
    });
}

/// One drain's worth of net signals. Connect/bind results go back to the host so it can resolve
/// the Promise (needs the Jobs resolver map). Events are queued here. Only failed connects are
/// returned for retirement after the microtask checkpoint.
pub(crate) struct SignalPoll {
    pub connects: Vec<(u64, Result<(), String>)>,
    pub polled: usize,
    pub drops: Vec<u64>,
}

/// Drain the engine channel. The tick only polls — it does not match Data/Datagram/Errored/Closed.
pub(crate) fn poll_signals() -> SignalPoll {
    poll_signals_limited(crate::async_limits::policy().frame_poll_items)
}
pub(crate) fn poll_signals_limited(limit: usize) -> SignalPoll {
    let mut connects = Vec::new();
    let mut polled = 0;
    let mut drops = Vec::new();
    for _ in 0..limit {
        let Some(sig) = try_recv_signal() else { break };
        polled += 1;
        if !engine().conns.lock().unwrap().contains_key(&sig.conn_id) {
            continue;
        }
        match sig.kind {
            NetSignalKind::Connected | NetSignalKind::Bound => {
                if let Some(c) = engine().conns.lock().unwrap().get_mut(&sig.conn_id) {
                    if c.phase == ConnPhase::Connecting {
                        c.phase = ConnPhase::Open;
                        connects.push((sig.conn_id, Ok(())));
                    }
                }
            }
            NetSignalKind::ConnectFailed(e) => {
                let accepted = engine()
                    .conns
                    .lock()
                    .unwrap()
                    .get_mut(&sig.conn_id)
                    .is_some_and(|c| {
                        if c.phase == ConnPhase::Connecting {
                            c.phase = ConnPhase::TerminalPending;
                            true
                        } else {
                            false
                        }
                    });
                if accepted {
                    connects.push((sig.conn_id, Err(e)));
                    drops.push(sig.conn_id);
                }
            }
            NetSignalKind::Data(b) => {
                if matches!(
                    engine()
                        .conns
                        .lock()
                        .unwrap()
                        .get(&sig.conn_id)
                        .map(|c| c.phase),
                    Some(ConnPhase::Open | ConnPhase::Closing)
                ) {
                    queue_data(sig.conn_id, b, sig.retention);
                }
            }
            NetSignalKind::Datagram { from, data } => {
                if matches!(
                    engine()
                        .conns
                        .lock()
                        .unwrap()
                        .get(&sig.conn_id)
                        .map(|c| c.phase),
                    Some(ConnPhase::Open | ConnPhase::Closing)
                ) {
                    queue_datagram(sig.conn_id, from, data, sig.retention);
                }
            }
            NetSignalKind::Terminal(t) => {
                let accepted = engine()
                    .conns
                    .lock()
                    .unwrap()
                    .get_mut(&sig.conn_id)
                    .is_some_and(|c| {
                        if matches!(c.phase, ConnPhase::Open | ConnPhase::Closing) {
                            c.phase = ConnPhase::TerminalPending;
                            true
                        } else {
                            false
                        }
                    });
                if accepted {
                    if let Some(e) = t.error {
                        queue_error(sig.conn_id, e, sig.retention.clone());
                    }
                    queue_close(sig.conn_id, sig.retention);
                }
            }
        }
    }
    SignalPoll {
        connects,
        drops,
        polled,
    }
}

fn net_owner(scope: &mut v8::PinScope) -> String {
    current_plugin(scope).unwrap_or_default()
}

/// Read a native arg as bytes: a TypedArray/DataView (copied) or a string (UTF-8). Never hands a
/// raw backing store to Rust.
fn js_bytes_arg(scope: &mut v8::PinScope, val: v8::Local<v8::Value>) -> Vec<u8> {
    if val.is_string() {
        return val.to_rust_string_lossy(scope).into_bytes();
    }
    if let Ok(view) = v8::Local::<v8::ArrayBufferView>::try_from(val) {
        let len = view.byte_length();
        let mut buf = vec![0u8; len];
        let n = view.copy_contents(&mut buf);
        buf.truncate(n);
        return buf;
    }
    Vec::new()
}

/// Build a JS `Uint8Array` from bytes — a fresh copy into a V8-owned ArrayBuffer.
fn bytes_to_uint8array<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    bytes: &[u8],
) -> v8::Local<'s, v8::Value> {
    if bytes.is_empty() {
        let ab = v8::ArrayBuffer::new(scope, 0);
        return match v8::Uint8Array::new(scope, ab, 0, 0) {
            Some(u) => u.into(),
            None => v8::null(scope).into(),
        };
    }
    let store = v8::ArrayBuffer::new_backing_store_from_bytes(bytes.to_vec()).make_shared();
    let ab = v8::ArrayBuffer::with_backing_store(scope, &store);
    let len = bytes.len();
    match v8::Uint8Array::new(scope, ab, 0, len) {
        Some(u) => u.into(),
        None => v8::null(scope).into(),
    }
}

fn js_bytes_len(scope: &mut v8::PinScope, val: v8::Local<v8::Value>) -> usize {
    if let Ok(s) = v8::Local::<v8::String>::try_from(val) {
        s.utf8_length(scope)
    } else {
        v8::Local::<v8::ArrayBufferView>::try_from(val)
            .map(|v| v.byte_length())
            .unwrap_or(0)
    }
}
fn s2_net_send(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    rv.set_bool(false);
    let id = args.get(0).number_value(scope).unwrap_or(0.0) as u64;
    let owner = net_owner(scope);
    let sender = engine()
        .conns
        .lock()
        .unwrap()
        .get(&id)
        .filter(|c| c.owner == owner && c.phase == ConnPhase::Open)
        .map(|c| c.data_tx.clone());
    let Some(sender) = sender else { return };
    let Ok(r) = sender.reserve(js_bytes_len(scope, args.get(1)).saturating_add(64)) else {
        return;
    };
    let bytes = js_bytes_arg(scope, args.get(1));
    rv.set_bool(sender.send_reserved(NetCommand::Send(bytes), r).is_ok());
}
fn s2_net_send_to(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    rv.set_bool(false);
    let id = args.get(0).number_value(scope).unwrap_or(0.0) as u64;
    let owner = net_owner(scope);
    let Some(host) = args.get(1).to_string(scope) else {
        return;
    };
    let port = args.get(2).number_value(scope).unwrap_or(0.0) as u16;
    let bytes = js_bytes_len(scope, args.get(3));
    if bytes > 65507 {
        return;
    }
    let sender = engine()
        .conns
        .lock()
        .unwrap()
        .get(&id)
        .filter(|c| c.owner == owner && c.phase == ConnPhase::Open)
        .map(|c| c.data_tx.clone());
    let Some(sender) = sender else { return };
    let Ok(r) = sender.reserve(
        bytes
            .saturating_add(host.utf8_length(scope))
            .saturating_add(64),
    ) else {
        return;
    };
    let host = host.to_rust_string_lossy(scope);
    let bytes = js_bytes_arg(scope, args.get(3));
    rv.set_bool(
        sender
            .send_reserved(NetCommand::SendTo(host, port, bytes), r)
            .is_ok(),
    );
}

fn s2_net_close(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 {
            return;
        }
        let id = args.get(0).number_value(scope).unwrap_or(0.0) as u64;
        let owner = net_owner(scope);
        close(id, &owner);
    }));
}

fn s2_net_on(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 3 {
            return;
        }
        let id = args.get(0).number_value(scope).unwrap_or(0.0) as u64;
        let event = args.get(1).to_rust_string_lossy(scope);
        let owner = net_owner(scope);
        if !is_owner(id, &owner) {
            log_warn(&format!(
                "WARN: __s2_net_on: '{owner}' does not own net conn {id} — '{event}' handler NOT \
                 subscribed and will never fire"
            ));
            return;
        }
        let key = format!("{id}:{event}");
        let _ = subscribe_into(scope, &args, &NET_EVENT_MUX, &key, 2);
    }));
}

thread_local! {static DELIVERY_CURSOR:std::cell::Cell<u64>=const {std::cell::Cell::new(0)};}
pub(crate) fn pending_events() -> bool {
    NET_EVENT_PENDING.with(|q| !q.borrow().is_empty())
}
pub(crate) fn dispatch_pending_events() {
    for _ in 0..crate::async_limits::policy().frame_items {
        if !dispatch_one() {
            break;
        }
    }
}
pub(crate) fn dispatch_one() -> bool {
    let last = DELIVERY_CURSOR.with(|v| v.get());
    // Pick a connection fairly, then its oldest event. Blocked connect settlements are skipped.
    let ready = NET_EVENT_PENDING.with(|q| {
        let q = q.borrow();
        let ids = q
            .iter()
            .filter(|e| !crate::jobs::has_resolver(e.0))
            .map(|e| e.0);
        let id = ids
            .clone()
            .filter(|id| *id > last)
            .min()
            .or_else(|| ids.min())?;
        let i = q.iter().position(|e| e.0 == id)?;
        let bytes = match &q[i].1 {
            PendingNetEvent::Data(b) => b.len() + 64,
            PendingNetEvent::Datagram { from, data } => from.len() + data.len() + 64,
            PendingNetEvent::Errored(e) => e.len() + 64,
            PendingNetEvent::Closed => 64,
        };
        Some((i, bytes))
    });
    let Some((i, bytes)) = ready else {
        return false;
    };
    if !crate::async_limits::can_deliver(bytes, false) {
        return false;
    }
    let (conn_id, ev, _retention) = NET_EVENT_PENDING.with(|q| q.borrow_mut().remove(i));
    DELIVERY_CURSOR.with(|v| v.set(conn_id));
    crate::async_limits::deliver(bytes);
    let event: &str = match &ev {
        PendingNetEvent::Data(_) => "data",
        PendingNetEvent::Datagram { .. } => "message",
        PendingNetEvent::Closed => "close",
        PendingNetEvent::Errored(_) => "error",
    };
    let key = format!("{conn_id}:{event}");
    let snap = NET_EVENT_MUX.with(|m| m.borrow().snapshot(&key));
    if !snap.is_empty() {
        let _ = fan_out(
            &snap,
            &format!("dispatch_pending_net_events('{key}')"),
            Instrument::none(),
            |tc| match &ev {
                PendingNetEvent::Data(b) => Some(vec![bytes_to_uint8array(tc, b)]),
                PendingNetEvent::Datagram { from, data } => {
                    let (fhost, fport): (&str, u16) = match from.rsplit_once(':') {
                        Some((h, p)) => (h, p.parse::<u16>().unwrap_or(0)),
                        None => (from.as_str(), 0),
                    };
                    let from_obj = v8::Object::new(tc);
                    if let Some(k) = v8::String::new(tc, "host") {
                        let v: v8::Local<v8::Value> = v8::String::new(tc, fhost)
                            .unwrap_or_else(|| v8::String::new(tc, "").unwrap())
                            .into();
                        from_obj.set(tc, k.into(), v);
                    }
                    if let Some(k) = v8::String::new(tc, "port") {
                        let v: v8::Local<v8::Value> = v8::Number::new(tc, fport as f64).into();
                        from_obj.set(tc, k.into(), v);
                    }
                    Some(vec![from_obj.into(), bytes_to_uint8array(tc, data)])
                }
                PendingNetEvent::Errored(e) => {
                    let s_val: v8::Local<v8::Value> = v8::String::new(tc, e)
                        .unwrap_or_else(|| v8::String::new(tc, "").unwrap())
                        .into();
                    Some(vec![s_val])
                }
                PendingNetEvent::Closed => Some(vec![]),
            },
        );
    }
    if matches!(ev, PendingNetEvent::Closed) {
        NET_EVENT_MUX.with(|m| {
            let mut mux = m.borrow_mut();
            for evn in ["data", "message", "error", "close"] {
                mux.remove_by_name(&format!("{conn_id}:{evn}"));
            }
        });
        retire_conn(conn_id);
    }
    true
}

pub(crate) fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    set_native(scope, global_obj, "__s2_net_send", s2_net_send);
    set_native(scope, global_obj, "__s2_net_send_to", s2_net_send_to);
    set_native(scope, global_obj, "__s2_net_close", s2_net_close);
    set_native(scope, global_obj, "__s2_net_on", s2_net_on);
}

pub(crate) fn register_store() {
    crate::owner_stores::register(
        "NET_EVENT_MUX",
        Box::new(|owner| {
            NET_EVENT_MUX.with(|m| m.borrow_mut().remove_by_owner(owner));
        }),
        Box::new(|ids| {
            NET_EVENT_MUX.with(|m| {
                m.borrow_mut().remove_by_ids(ids);
            });
        }),
        Box::new(|| {
            NET_EVENT_MUX.with(|m| *m.borrow_mut() = crate::channels::Channels::new());
        }),
    );
}

pub(crate) fn register_singletons() {
    use crate::process_singletons::ResetPhase::AfterIsolateDrop;
    crate::process_singletons::register(
        "NET_EVENT_PENDING",
        AfterIsolateDrop,
        Box::new(|| NET_EVENT_PENDING.with(|q| q.borrow_mut().clear())),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll};

    struct PendingReader;
    impl AsyncRead for PendingReader {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut tokio::io::ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Pending
        }
    }

    struct FloodReader(Arc<AtomicUsize>);
    impl AsyncRead for FloodReader {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            buf.put_slice(&[b'x']);
            Poll::Ready(Ok(()))
        }
    }

    struct RecordingWriter(Arc<AtomicUsize>);
    impl AsyncWrite for RecordingWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            self.0.fetch_add(buf.len(), Ordering::SeqCst);
            Poll::Ready(Ok(buf.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    struct FailingWriter;
    impl AsyncWrite for FailingWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            Poll::Ready(Err(std::io::Error::other("injected write failure")))
        }
        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    struct PendingWriter(Arc<AtomicUsize>);
    impl AsyncWrite for PendingWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Poll::Pending
        }
        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Pending
        }
        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Pending
        }
    }
    // A local TCP echo listener on the shared runtime (mirrors ws.rs's echo_server_port).
    fn tcp_echo_port() -> u16 {
        crate::http::init();
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut buf = [0u8; 64];
                if let Ok(n) = s.read(&mut buf) {
                    let _ = s.write_all(&buf[..n]);
                }
            }
        });
        port
    }
    fn drain_until<F: Fn(&NetSignal) -> bool>(f: F) -> NetSignal {
        for _ in 0..500 {
            while let Some(s) = try_recv_signal() {
                if f(&s) {
                    return s;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("no matching signal");
    }
    fn wait_workers(n: usize) {
        for _ in 0..200 {
            if active_worker_count() == n {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("worker count did not reach {n}: {}", active_worker_count());
    }
    #[test]
    fn owner_shutdown_cancels_a_pending_connector_silently() {
        crate::http::init();
        let base = active_worker_count();
        let (mut _rx, mut crx) = insert_conn(8799, "cancel".into(), 0);
        crate::http::spawn(async move {
            let _g = WorkerGuard::new();
            tokio::select! {biased;_=crx.changed()=>{},_=std::future::pending::<()>()=>{}}
        });
        wait_workers(base + 1);
        shutdown_conn(8799);
        wait_workers(base);
        assert!(!is_owner(8799, "cancel"));
        while let Some(s) = try_recv_signal() {
            assert_ne!(s.conn_id, 8799, "shutdown must not signal");
        }
    }
    #[test]
    fn tcp_connect_timeout_is_injected_and_bounded() {
        crate::http::init();
        let (_control_tx, mut control_rx) = tokio::sync::watch::channel(Control::Open);
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        crate::http::spawn(async move {
            let result = await_connect(
                std::future::pending::<std::io::Result<()>>(),
                &mut control_rx,
                Duration::from_millis(20),
            )
            .await;
            done_tx.send(result).unwrap();
        });
        let result = done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("connect deadline");
        assert!(result
            .unwrap()
            .unwrap_err()
            .contains("did not complete within 20ms"));
    }
    #[test]
    fn injected_tcp_write_failure_returns_one_error_terminal() {
        crate::http::init();
        let (data_tx, data_rx) = crate::async_limits::out_channel();
        let (_control_tx, mut control_rx) = tokio::sync::watch::channel(Control::Open);
        let (signal_tx, signal_rx) = channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        data_tx.send(NetCommand::Send(b"fail".to_vec())).unwrap();
        crate::http::spawn(async move {
            let terminal = run_tcp_connected(
                8810,
                PendingReader,
                FailingWriter,
                data_rx,
                &mut control_rx,
                signal_tx,
                SocketDeadlines::default(),
            )
            .await;
            done_tx.send(terminal).unwrap();
        });
        let terminal = done_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert_eq!(terminal.error.as_deref(), Some("injected write failure"));
        assert!(
            signal_rx.try_recv().is_err(),
            "write failure must not emit a second envelope"
        );
    }
    #[test]
    fn owner_shutdown_interrupts_an_injected_pending_tcp_write() {
        crate::http::init();
        let polls = Arc::new(AtomicUsize::new(0));
        let (data_tx, data_rx) = crate::async_limits::out_channel();
        let (control_tx, mut control_rx) = tokio::sync::watch::channel(Control::Open);
        let (signal_tx, signal_rx) = channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        data_tx.send(NetCommand::Send(b"stall".to_vec())).unwrap();
        let worker_polls = polls.clone();
        crate::http::spawn(async move {
            let terminal = run_tcp_connected(
                8811,
                PendingReader,
                PendingWriter(worker_polls),
                data_rx,
                &mut control_rx,
                signal_tx,
                SocketDeadlines::default(),
            )
            .await;
            done_tx.send(terminal).unwrap();
        });
        for _ in 0..200 {
            if polls.load(Ordering::SeqCst) > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(polls.load(Ordering::SeqCst) > 0, "writer was never polled");
        publish_control(&control_tx, Control::Shutdown);
        assert!(done_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .is_none());
        assert!(signal_rx.try_recv().is_err(), "owner shutdown is silent");
    }
    #[test]
    fn continuous_tcp_reads_do_not_starve_a_queued_write_or_shutdown() {
        crate::http::init();
        let reads = Arc::new(AtomicUsize::new(0));
        let bytes = Arc::new(AtomicUsize::new(0));
        let (data_tx, data_rx) = crate::async_limits::out_channel();
        let (control_tx, mut control_rx) = tokio::sync::watch::channel(Control::Open);
        let (signal_tx, _) = channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        data_tx
            .send(NetCommand::Send(b"progress".to_vec()))
            .unwrap();
        let worker_reads = reads.clone();
        let worker_bytes = bytes.clone();
        crate::http::spawn(async move {
            done_tx
                .send(
                    run_tcp_connected(
                        8814,
                        FloodReader(worker_reads),
                        RecordingWriter(worker_bytes),
                        data_rx,
                        &mut control_rx,
                        signal_tx,
                        SocketDeadlines::default(),
                    )
                    .await,
                )
                .unwrap();
        });
        for _ in 0..200 {
            if bytes.load(Ordering::SeqCst) == 8 {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            bytes.load(Ordering::SeqCst),
            8,
            "ready reads starved the write"
        );
        assert!(
            reads.load(Ordering::SeqCst) > 0,
            "probe did not keep reads ready"
        );
        publish_control(&control_tx, Control::Shutdown);
        assert!(done_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .is_none());
    }
    #[test]
    fn fair_io_primitive_used_by_tcp_and_udp_selects_both_ready_sides() {
        crate::http::init();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        crate::http::spawn(async move {
            let mut left = 0;
            let mut right = 0;
            for _ in 0..10_000 {
                match fair_pair(std::future::ready(()), std::future::ready(())).await {
                    Fair::Left(()) => left += 1,
                    Fair::Right(()) => right += 1,
                }
            }
            done_tx.send((left, right)).unwrap();
        });
        let (left, right) = done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(
            left > 0 && right > 0,
            "fair selector starved one side: {left}/{right}"
        );
    }
    #[test]
    fn pending_tcp_write_obeys_one_grace_deadline() {
        crate::http::init();
        let polls = Arc::new(AtomicUsize::new(0));
        let (data_tx, data_rx) = crate::async_limits::out_channel();
        let (control_tx, mut control_rx) = tokio::sync::watch::channel(Control::Open);
        let (signal_tx, _) = channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        data_tx.send(NetCommand::Send(b"stall".to_vec())).unwrap();
        let worker_polls = polls.clone();
        crate::http::spawn(async move {
            done_tx
                .send(
                    run_tcp_connected(
                        8812,
                        PendingReader,
                        PendingWriter(worker_polls),
                        data_rx,
                        &mut control_rx,
                        signal_tx,
                        SocketDeadlines {
                            connect: Duration::from_secs(10),
                            close_grace: Duration::from_millis(20),
                        },
                    )
                    .await,
                )
                .unwrap();
        });
        for _ in 0..200 {
            if polls.load(Ordering::SeqCst) > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        publish_control(&control_tx, Control::CloseRequested);
        let terminal = done_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert_eq!(
            terminal.error.as_deref(),
            Some("graceful close deadline exceeded")
        );
    }
    #[test]
    fn simultaneous_peer_and_local_tcp_close_produces_one_terminal() {
        crate::http::init();
        let (_data_tx, data_rx) = crate::async_limits::out_channel();
        let (control_tx, mut control_rx) = tokio::sync::watch::channel(Control::Open);
        publish_control(&control_tx, Control::CloseRequested);
        let (signal_tx, signal_rx) = channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        crate::http::spawn(async move {
            done_tx
                .send(
                    run_tcp_connected(
                        8813,
                        tokio::io::empty(),
                        tokio::io::sink(),
                        data_rx,
                        &mut control_rx,
                        signal_tx,
                        SocketDeadlines::default(),
                    )
                    .await,
                )
                .unwrap();
        });
        let terminal = done_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert!(
            terminal.error.is_none(),
            "simultaneous normal closes stay normal"
        );
        assert!(
            signal_rx.try_recv().is_err(),
            "the connected loop returns one terminal only"
        );
    }
    #[test]
    fn tcp_connect_send_echo() {
        let port = tcp_echo_port();
        connect_tcp(1, "127.0.0.1".into(), port, "p".into());
        drain_until(|s| matches!(s.kind, NetSignalKind::Connected));
        engine().conns.lock().unwrap().get_mut(&1).unwrap().phase = ConnPhase::Open;
        assert!(send(1, "p", b"hello".to_vec()));
        let sig = drain_until(|s| matches!(s.kind, NetSignalKind::Data(_)));
        match sig.kind {
            NetSignalKind::Data(b) => assert_eq!(b, b"hello"),
            _ => unreachable!(),
        }
        assert!(!send(1, "pB", b"x".to_vec())); // wrong owner denied
        close(1, "p");
        drain_until(|s| matches!(s.kind, NetSignalKind::Terminal(_)));
        retire_conn(1);
        assert!(!is_owner(1, "p"));
    }
    #[test]
    fn tcp_bad_port_fails() {
        crate::http::init();
        connect_tcp(2, "127.0.0.1".into(), 1, "p".into());
        drain_until(|s| matches!(s.kind, NetSignalKind::ConnectFailed(_)));
    }
    #[test]
    fn udp_bind_send_recv() {
        crate::http::init();
        // A local UDP echo on a std socket + thread.
        let echo = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let echo_port = echo.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let mut buf = [0u8; 64];
            if let Ok((n, from)) = echo.recv_from(&mut buf) {
                let _ = echo.send_to(&buf[..n], from);
            }
        });
        bind_udp(3, "p".into());
        drain_until(|s| matches!(s.kind, NetSignalKind::Bound));
        engine().conns.lock().unwrap().get_mut(&3).unwrap().phase = ConnPhase::Open;
        assert!(send_to(
            3,
            "p",
            "127.0.0.1".into(),
            echo_port,
            b"ping".to_vec()
        ));
        let sig = drain_until(|s| matches!(s.kind, NetSignalKind::Datagram { .. }));
        match sig.kind {
            NetSignalKind::Datagram { data, .. } => assert_eq!(data, b"ping"),
            _ => unreachable!(),
        }
        drop_conn(3);
    }

    /// The tick only polls. Data and the single terminal envelope are queued for dispatch.
    #[test]
    fn poll_signals_returns_connects_and_queues_events() {
        while try_recv_signal().is_some() {}
        NET_EVENT_PENDING.with(|q| q.borrow_mut().retain(|e| e.0 != 8801));
        let tx = &engine().sig_tx;
        let (data_tx, _data_rx) = crate::async_limits::out_channel();
        let (control_tx, _) = tokio::sync::watch::channel(Control::Open);
        engine().conns.lock().unwrap().insert(
            8801,
            Conn {
                resources: None,
                data_tx,
                control_tx,
                phase: ConnPhase::Connecting,
                owner: "test".into(),
                owner_generation: 0,
            },
        );
        let _ = tx.send(NetSignal {
            retention: None,
            queue: None,
            conn_id: 8801,
            kind: NetSignalKind::Connected,
        });
        let _ = tx.send(NetSignal {
            retention: None,
            queue: None,
            conn_id: 8801,
            kind: NetSignalKind::Data(b"hi".to_vec()),
        });
        let _ = tx.send(NetSignal {
            retention: None,
            queue: None,
            conn_id: 8801,
            kind: NetSignalKind::Terminal(NetTerminal {
                error: Some("boom".into()),
            }),
        });
        let _ = tx.send(NetSignal {
            retention: None,
            queue: None,
            conn_id: 8801,
            kind: NetSignalKind::Terminal(NetTerminal {
                error: Some("duplicate".into()),
            }),
        });
        let p = poll_signals();
        assert_eq!(p.connects, vec![(8801, Ok(()))]);
        assert!(
            p.drops.is_empty(),
            "public terminal retirement belongs after callback dispatch"
        );
        let pending = NET_EVENT_PENDING.with(|q| q.borrow().len());
        let kinds = NET_EVENT_PENDING.with(|q| {
            q.borrow()
                .iter()
                .filter(|e| e.0 == 8801)
                .map(|(_, event, _)| match event {
                    PendingNetEvent::Data(_) => "data",
                    PendingNetEvent::Errored(_) => "error",
                    PendingNetEvent::Closed => "close",
                    PendingNetEvent::Datagram { .. } => "message",
                })
                .collect::<Vec<_>>()
        });
        assert_eq!(
            kinds,
            ["data", "error", "close"],
            "duplicate terminal must be discarded; pending={pending}"
        );
        NET_EVENT_PENDING.with(|q| q.borrow_mut().retain(|e| e.0 != 8801));
        shutdown_conn(8801);
    }

    #[test]
    fn poll_signals_failed_connect_is_a_drop_without_an_event() {
        while try_recv_signal().is_some() {}
        NET_EVENT_PENDING.with(|q| q.borrow_mut().retain(|e| e.0 != 8802));
        let (data_tx, _data_rx) = crate::async_limits::out_channel();
        let (control_tx, _) = tokio::sync::watch::channel(Control::Open);
        engine().conns.lock().unwrap().insert(
            8802,
            Conn {
                resources: None,
                data_tx,
                control_tx,
                phase: ConnPhase::Connecting,
                owner: "test".into(),
                owner_generation: 0,
            },
        );
        let _ = engine().sig_tx.send(NetSignal {
            retention: None,
            queue: None,
            conn_id: 8802,
            kind: NetSignalKind::ConnectFailed("nope".into()),
        });
        let p = poll_signals();
        assert_eq!(p.connects, vec![(8802, Err("nope".into()))]);
        assert_eq!(p.drops, vec![8802]);
        assert!(NET_EVENT_PENDING.with(|q| q.borrow().iter().all(|e| e.0 != 8802)));
    }
    #[test]
    fn a_thousand_adapter_transitions_leave_no_socket_state() {
        let workers = active_worker_count();
        for i in 0..1000u64 {
            let id = 90_000 + i;
            let (data_tx, _) = crate::async_limits::out_channel();
            let (control_tx, _) = tokio::sync::watch::channel(Control::Open);
            engine().conns.lock().unwrap().insert(
                id,
                Conn {
                    resources: None,
                    data_tx,
                    control_tx,
                    phase: ConnPhase::Connecting,
                    owner: "stress".into(),
                    owner_generation: 0,
                },
            );
            if i % 2 == 0 {
                let _ = engine().sig_tx.send(NetSignal {
                    retention: None,
                    queue: None,
                    conn_id: id,
                    kind: NetSignalKind::Connected,
                });
                let _ = engine().sig_tx.send(NetSignal {
                    retention: None,
                    queue: None,
                    conn_id: id,
                    kind: NetSignalKind::Terminal(NetTerminal { error: None }),
                });
                let p = poll_signals();
                assert!(p.drops.is_empty());
                dispatch_pending_events();
            } else {
                let _ = engine().sig_tx.send(NetSignal {
                    retention: None,
                    queue: None,
                    conn_id: id,
                    kind: NetSignalKind::ConnectFailed("x".into()),
                });
                let p = poll_signals();
                for id in p.drops {
                    retire_conn(id);
                }
            }
        }
        assert_eq!(active_conn_count(), 0);
        assert_eq!(active_worker_count(), workers);
        assert!(NET_EVENT_PENDING.with(|q| q.borrow().is_empty()));
    }
}

impl crate::async_limits::PayloadSize for NetCommand { fn bytes(&self)->usize { match self { Self::Send(b)=>b.len().saturating_add(64),Self::SendTo(h,_,b)=>h.len().saturating_add(b.len()).saturating_add(64) } } }
impl crate::async_limits::Signal for NetSignal {
    fn data_bytes(&self) -> Option<usize> {
        match &self.kind {
            NetSignalKind::Data(b) => Some(b.len().saturating_add(64)),
            NetSignalKind::Datagram { from, data } => {
                Some(from.len().saturating_add(data.len()).saturating_add(64))
            }
            _ => None,
        }
    }
    fn retain(&mut self, r: Arc<crate::async_limits::Retention>) {
        self.retention = Some(r);
        self.queue = Some(crate::async_limits::QueueTicket::new(4));
    }
    fn bound_diagnostics(&mut self,cap:usize) { match &mut self.kind {
        NetSignalKind::ConnectFailed(s)=>*s=crate::async_limits::diagnostic_limit(std::mem::take(s),cap),
        NetSignalKind::Terminal(t)=>t.error=t.error.take().map(|s|crate::async_limits::diagnostic_limit(s,cap)),_=>{}
    } }
}

pub(crate) fn pending_count() -> usize {
    NET_EVENT_PENDING.with(|q| q.borrow().len())
}

pub(crate) fn shutdown_all() {
    let ids = engine()
        .conns
        .lock()
        .unwrap()
        .keys()
        .copied()
        .collect::<Vec<_>>();
    for id in ids {
        shutdown_conn(id);
    }
}
