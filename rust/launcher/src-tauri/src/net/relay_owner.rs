//! Whether a connection to the relay comes from the game (RD21; DR rows L22, L31, `relay-owner.ts`). On
//! Windows the TCP table (`netstat -ano -p tcp`) names the process that owns the client end of a loopback
//! connection; only the process id of the game the launcher started passes. Requests waiting together
//! share one fresh snapshot; nothing is cached, a missing row gets one retry 25 ms later, and everything
//! else fails closed, as do more than 64 waiting checks.
//!
//! Elsewhere (Linux, the game under Proton) there is no owner check yet: the relay serves any loopback
//! connection, as Reforged's Electron launcher does on every platform. That is
//! [`loopback_only_without_owner_check`], which RA10 replaces if it finds the game's process.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use super::relay::Authorize;

mod table;

use table::{endpoint, peer_pid};

const MAX_PENDING: usize = 64;

/// The process id of the game the launcher started, while it runs.
pub type GamePid = Arc<dyn Fn() -> Option<u32> + Send + Sync>;

#[cfg_attr(not(test), expect(dead_code, reason = "the tests use it"))]
pub fn tcp_table_owns_peer(table: &str, local: SocketAddr, remote: SocketAddr, pid: u32) -> bool {
    pid > 0 && peer_pid(table, local, remote) == Some(i64::from(pid))
}

pub type ReadTable = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Option<String>> + Send>> + Send + Sync>;

struct Pending {
    local: SocketAddr,
    remote: SocketAddr,
    pid: u32,
    answer: oneshot::Sender<bool>,
}

/// `netstat -ano -p tcp` from the system folder, within 2 s.
pub fn read_netstat() -> ReadTable {
    Arc::new(|| {
        Box::pin(async {
            let netstat = crate::platform::launch::system_root()
                .join("System32")
                .join("netstat.exe");
            crate::platform::launch::run_hidden(
                &netstat,
                &["-ano", "-p", "tcp"],
                Duration::from_secs(2),
                4 * 1024 * 1024,
            )
            .await
        })
    })
}

async fn serve(mut requests: mpsc::UnboundedReceiver<Pending>, read: ReadTable, pending: Arc<AtomicUsize>) {
    while let Some(first) = requests.recv().await {
        // Everything waiting now shares one snapshot; later arrivals wait for the next
        let mut batch = vec![first];
        while let Ok(more) = requests.try_recv() {
            batch.push(more);
        }
        let table = read().await;
        let mut owners: Vec<Option<i64>> = batch
            .iter()
            .map(|item| {
                table
                    .as_deref()
                    .map_or(Some(-1), |table| peer_pid(table, item.local, item.remote))
            })
            .collect();
        if owners.contains(&None) {
            // An accepted socket can come before its row in the table
            tokio::time::sleep(Duration::from_millis(25)).await;
            if let Some(retry) = read().await {
                for (item, owner) in batch.iter().zip(owners.iter_mut()) {
                    if owner.is_none() {
                        *owner = peer_pid(&retry, item.local, item.remote);
                    }
                }
            }
        }
        for (item, owner) in batch.into_iter().zip(owners) {
            let _ = item.answer.send(owner == Some(i64::from(item.pid)));
            pending.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// The relay's owner check for the game process `pid()` (the current game, when there is one).
pub fn owner_check(read: ReadTable, windows: bool, pid: GamePid) -> Authorize {
    let (tx, rx) = mpsc::unbounded_channel();
    let pending = Arc::new(AtomicUsize::new(0));
    if windows {
        tokio::spawn(serve(rx, read, pending.clone()));
    }
    Arc::new(move |local, remote| {
        let (tx, pending, pid) = (tx.clone(), pending.clone(), pid());
        Box::pin(async move {
            let Some(pid) = pid.filter(|pid| *pid > 0) else {
                return false;
            };
            if !windows || endpoint(local).is_none() || endpoint(remote).is_none() {
                return false;
            }
            if pending.fetch_add(1, Ordering::SeqCst) >= MAX_PENDING {
                pending.fetch_sub(1, Ordering::SeqCst);
                return false;
            }
            let (answer, verdict) = oneshot::channel();
            if tx
                .send(Pending {
                    local,
                    remote,
                    pid,
                    answer,
                })
                .is_err()
            {
                pending.fetch_sub(1, Ordering::SeqCst);
                return false;
            }
            verdict.await.unwrap_or(false)
        })
    })
}

/// The relay's owner check on this platform (`RelayOptions::authorize`): on Windows only the game's own
/// connections are served; elsewhere see [`loopback_only_without_owner_check`].
pub fn game_owner_check(pid: GamePid) -> Option<Authorize> {
    if cfg!(windows) {
        Some(owner_check(read_netstat(), true, pid))
    } else {
        loopback_only_without_owner_check(pid)
    }
}

/// Linux and every platform but Windows: no owner check, so any process on this PC that connects to
/// 127.0.0.1 is served, within the relay's local-request rules (loopback peer, loopback Host, no browser
/// headers or foreign Origin). This is what Reforged's Electron relay does today. RA10 replaces this
/// function with a real check if it can find the game's process under Proton (RD21).
pub fn loopback_only_without_owner_check(_pid: GamePid) -> Option<Authorize> {
    None
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    const ROW: &str = " TCP 127.0.0.1:52123 127.0.0.1:61100 ESTABLISHED 123\r\n";

    #[test]
    fn the_client_row_and_the_exact_pid_never_the_server_row() {
        let (local, remote) = (addr("127.0.0.1:61100"), addr("127.0.0.1:52123"));
        assert!(tcp_table_owns_peer(ROW, local, remote, 123));
        for table in [
            ROW.replace("123\r", "456\r"),
            format!("{ROW}{ROW}"),
            "TCP 127.0.0.1:61100 127.0.0.1:52123 ESTABLISHED 123".into(),
            ROW.replace("52123", "52124"),
            ROW.replace("127.0.0.1:52123", "127.0.0.2:52123"),
        ] {
            assert!(!tcp_table_owns_peer(&table, local, remote, 123), "{table:?}");
        }
        assert!(tcp_table_owns_peer(ROW, local, addr("[::ffff:127.0.0.1]:52123"), 123));
        assert!(!tcp_table_owns_peer(ROW, local, remote, 0));
        assert!(tcp_table_owns_peer(
            &ROW.replace("ESTABLISHED", "HERGESTELLT"),
            local,
            remote,
            123
        ));
    }

    fn counting(tables: Arc<Mutex<Vec<String>>>, reads: Arc<AtomicUsize>) -> ReadTable {
        Arc::new(move || {
            let (tables, reads) = (tables.clone(), reads.clone());
            Box::pin(async move {
                reads.fetch_add(1, Ordering::SeqCst);
                let mut tables = tables.lock().unwrap();
                Some(if tables.len() > 1 {
                    tables.remove(0)
                } else {
                    tables[0].clone()
                })
            })
        })
    }

    #[tokio::test]
    async fn checks_share_fresh_snapshots_and_fail_closed() {
        let (local, remote) = (addr("127.0.0.1:61100"), addr("127.0.0.1:52123"));
        let reads = Arc::new(AtomicUsize::new(0));
        let check = owner_check(
            counting(Arc::new(Mutex::new(vec![ROW.into()])), reads.clone()),
            true,
            Arc::new(|| Some(123)),
        );
        assert!(check(local, remote).await);
        assert!(check(local, remote).await);
        assert_eq!(reads.load(Ordering::SeqCst), 2, "no snapshot is reused");

        let other_platform = owner_check(
            counting(Arc::new(Mutex::new(vec![ROW.into()])), Arc::default()),
            false,
            Arc::new(|| Some(123)),
        );
        assert!(!other_platform(local, remote).await);
        let no_game = owner_check(
            counting(Arc::new(Mutex::new(vec![ROW.into()])), Arc::default()),
            true,
            Arc::new(|| None),
        );
        assert!(!no_game(local, remote).await);
        let failing: ReadTable = Arc::new(|| Box::pin(async { None }));
        assert!(!owner_check(failing, true, Arc::new(|| Some(123)))(local, remote).await);

        // A row that appears late gets one retry; one that never does is refused after it
        let reads = Arc::new(AtomicUsize::new(0));
        let appears = owner_check(
            counting(Arc::new(Mutex::new(vec![String::new(), ROW.into()])), reads.clone()),
            true,
            Arc::new(|| Some(123)),
        );
        assert!(appears(local, remote).await);
        assert_eq!(reads.load(Ordering::SeqCst), 2);
        let reads = Arc::new(AtomicUsize::new(0));
        let wrong = owner_check(
            counting(Arc::new(Mutex::new(vec![ROW.into()])), reads.clone()),
            true,
            Arc::new(|| Some(456)),
        );
        assert!(!wrong(local, remote).await);
        assert_eq!(reads.load(Ordering::SeqCst), 1, "a wrong owner is not retried");
    }

    #[tokio::test]
    async fn simultaneous_sockets_share_one_snapshot() {
        let table: String = (0..32)
            .map(|i| ROW.replace("52123", &(52123 + i).to_string()))
            .collect();
        let reads = Arc::new(AtomicUsize::new(0));
        let slow: ReadTable = {
            let reads = reads.clone();
            Arc::new(move || {
                let (reads, table) = (reads.clone(), table.clone());
                Box::pin(async move {
                    reads.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Some(table)
                })
            })
        };
        let check = owner_check(slow, true, Arc::new(|| Some(123)));
        let local = addr("127.0.0.1:61100");
        let first = check(local, addr("127.0.0.1:52123"));
        let first = tokio::spawn(first);
        tokio::time::sleep(Duration::from_millis(10)).await;
        let rest =
            futures_util::future::join_all((1..32).map(|i| check(local, addr(&format!("127.0.0.1:{}", 52123 + i)))))
                .await;
        assert!(first.await.unwrap() && rest.iter().all(|ok| *ok));
        assert!(reads.load(Ordering::SeqCst) <= 2, "{}", reads.load(Ordering::SeqCst));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_real_loopback_socket_resolves_this_process() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (accepted, remote) = listener.accept().await.unwrap();
        let local = accepted.local_addr().unwrap();
        let me = std::process::id();
        assert!(owner_check(read_netstat(), true, Arc::new(move || Some(me)))(local, remote).await);
        assert!(!owner_check(read_netstat(), true, Arc::new(move || Some(me + 1)))(local, remote).await);
        drop(client);
    }
}
