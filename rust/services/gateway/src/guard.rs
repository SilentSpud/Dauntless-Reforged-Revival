//! The timers of one client connection, as Node's HTTP server keeps them (`headersTimeout`,
//! `keepAliveTimeout`, `server.setTimeout`). hyper's own header timer also runs while a keep-alive
//! connection waits for its next request, which would close idle connections after 10 s instead of
//! 65 s, so the gateway keeps these itself:
//!
//! - waiting for a request: the connection may stay silent for the keep-alive time (the headers time
//!   on a new connection); once its first byte comes, the whole head must arrive within the headers
//!   time;
//! - always: a connection that moves no bytes for the idle time (the WebSocket idle time once
//!   upgraded) is cut, a stalled download included.

use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::{Instant, Sleep};

#[derive(Debug, Clone, Copy)]
enum Phase {
    /// Between requests: silent since `since` for at most `limit`; `started` is when the next head's
    /// first byte came
    AwaitingHead {
        since: Instant,
        limit: Duration,
        started: Option<Instant>,
    },
    /// A request is being answered (its own timers apply)
    InRequest,
    /// Joined to the WebSocket upstream
    Upgraded,
}

struct State {
    phase: Phase,
    last_activity: Instant,
    idle: Duration,
    /// A request was answered on this connection
    served: bool,
}

/// Node answers a request head that is too slow (or never comes on a new connection) like this.
const REQUEST_TIMEOUT: &[u8] = b"HTTP/1.1 408 Request Timeout\r\nContent-Type: application/json\r\nContent-Length: 27\r\nConnection: close\r\n\r\n{\"error\":\"request_timeout\"}";

/// The handle the request handler uses to move the connection between phases.
#[derive(Clone)]
pub struct Phases {
    state: Arc<Mutex<State>>,
    headers: Duration,
    keep_alive: Duration,
}

impl Phases {
    pub fn new(headers: Duration, keep_alive: Duration, idle: Duration) -> Self {
        let now = Instant::now();
        Self {
            state: Arc::new(Mutex::new(State {
                phase: Phase::AwaitingHead {
                    since: now,
                    limit: headers,
                    started: None,
                },
                last_activity: now,
                idle,
                served: false,
            })),
            headers,
            keep_alive,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A request head arrived.
    pub fn request_started(&self) {
        self.lock().phase = Phase::InRequest;
    }

    /// The answer is complete (or abandoned): the connection waits for the next request.
    pub fn request_done(&self) {
        let mut state = self.lock();
        if matches!(state.phase, Phase::InRequest) {
            state.served = true;
            state.phase = Phase::AwaitingHead {
                since: Instant::now(),
                limit: self.keep_alive,
                started: None,
            };
        }
    }

    /// The connection is a WebSocket now: no request timers, its own idle time.
    pub fn upgraded(&self, idle: Duration) {
        let mut state = self.lock();
        state.phase = Phase::Upgraded;
        state.idle = idle;
        state.last_activity = Instant::now();
    }

    fn progressed(&self, read: bool) {
        let mut state = self.lock();
        let now = Instant::now();
        state.last_activity = now;
        if read
            && let Phase::AwaitingHead {
                started: started @ None,
                ..
            } = &mut state.phase
        {
            *started = Some(now);
        }
    }

    /// Whether a timeout now ends a request head that was too slow (and earns a 408).
    fn head_overdue(&self) -> bool {
        let state = self.lock();
        match state.phase {
            Phase::AwaitingHead { started, since, limit } => {
                let now = Instant::now();
                match started {
                    Some(started) => now >= started + self.headers,
                    None => !state.served && now >= since + limit,
                }
            }
            Phase::InRequest | Phase::Upgraded => false,
        }
    }

    fn deadline(&self) -> Instant {
        let state = self.lock();
        let idle = state.last_activity + state.idle;
        match state.phase {
            Phase::AwaitingHead {
                started: Some(started), ..
            } => idle.min(started + self.headers),
            Phase::AwaitingHead { since, limit, .. } => idle.min(since + limit),
            Phase::InRequest | Phase::Upgraded => idle,
        }
    }
}

pub struct Guard<S> {
    inner: S,
    phases: Phases,
    sleep: Pin<Box<Sleep>>,
    /// The 408 being written (how much of it is out) before the connection ends
    goodbye: Option<usize>,
    /// The write side is shut: what the client still sends is read and dropped until it closes, for
    /// at most a second (`sleep`)
    closing: bool,
}

impl<S> Guard<S> {
    pub fn new(inner: S, phases: Phases) -> Self {
        let deadline = phases.deadline();
        Self {
            inner,
            phases,
            sleep: Box::pin(tokio::time::sleep_until(deadline)),
            goodbye: None,
            closing: false,
        }
    }

    /// Whether the connection has outlived its timers (and arms the timer for the next check).
    fn expired(&mut self, cx: &mut Context<'_>) -> bool {
        let deadline = self.phases.deadline();
        if Instant::now() >= deadline {
            return true;
        }
        if self.sleep.deadline() != deadline {
            self.sleep.as_mut().reset(deadline);
        }
        self.sleep.as_mut().poll(cx).is_ready() && Instant::now() >= deadline
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> Guard<S> {
    /// Reads and drops what the client still sends: ready once it closes (or the read fails). A
    /// socket closed with unread input is reset, and on Windows a reset drops the answer the client
    /// has not read yet.
    fn linger(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let mut scratch = [0u8; 1024];
        loop {
            let mut buf = ReadBuf::new(&mut scratch);
            match Pin::new(&mut self.inner).poll_read(cx, &mut buf) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Ok(())) if !buf.filled().is_empty() => {}
                Poll::Ready(_) => return Poll::Ready(()),
            }
        }
    }

    /// Writes the 408, shuts the write side and lingers (for at most a second more in all), then
    /// ends the connection.
    fn say_goodbye(&mut self, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let timed_out = || Poll::Ready(Err(std::io::ErrorKind::TimedOut.into()));
        if self.sleep.as_mut().poll(cx).is_ready() {
            return timed_out();
        }
        let mut written = self.goodbye.unwrap_or(0);
        while written < REQUEST_TIMEOUT.len() {
            match Pin::new(&mut self.inner).poll_write(cx, &REQUEST_TIMEOUT[written..]) {
                Poll::Ready(Ok(n)) if n > 0 => written += n,
                Poll::Ready(_) => return timed_out(),
                Poll::Pending => {
                    self.goodbye = Some(written);
                    return Poll::Pending;
                }
            }
        }
        self.goodbye = Some(written);
        if !self.closing {
            match Pin::new(&mut self.inner).poll_flush(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(_)) => return timed_out(),
                Poll::Ready(Ok(())) => {}
            }
            match Pin::new(&mut self.inner).poll_shutdown(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(_)) => return timed_out(),
                Poll::Ready(Ok(())) => self.closing = true,
            }
        }
        match self.linger(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(()) => timed_out(),
        }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncRead for Guard<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        if self.goodbye.is_some() {
            return self.say_goodbye(cx);
        }
        let before = buf.filled().len();
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(result) => {
                if buf.filled().len() > before {
                    self.phases.progressed(true);
                }
                Poll::Ready(result)
            }
            Poll::Pending if self.expired(cx) => {
                if self.phases.head_overdue() {
                    self.goodbye = Some(0);
                    let until = Instant::now() + Duration::from_secs(1);
                    self.sleep.as_mut().reset(until);
                    return self.say_goodbye(cx);
                }
                Poll::Ready(Err(std::io::ErrorKind::TimedOut.into()))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncWrite for Guard<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<std::io::Result<usize>> {
        match Pin::new(&mut self.inner).poll_write(cx, data) {
            Poll::Ready(result) => {
                self.phases.progressed(false);
                Poll::Ready(result)
            }
            Poll::Pending if self.expired(cx) => Poll::Ready(Err(std::io::ErrorKind::TimedOut.into())),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    /// Shuts the write side, then lingers: hyper ends a connection here after an answer that closes
    /// it, such as a 413 sent before the client finished its body.
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        if !self.closing {
            match Pin::new(&mut self.inner).poll_shutdown(cx) {
                Poll::Ready(Ok(())) => {}
                other => return other,
            }
            self.closing = true;
            let until = Instant::now() + Duration::from_secs(1);
            self.sleep.as_mut().reset(until);
        }
        if self.sleep.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Ok(()));
        }
        self.linger(cx).map(Ok)
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    const HEADERS: Duration = Duration::from_secs(10);
    const KEEP_ALIVE: Duration = Duration::from_secs(65);
    const IDLE: Duration = Duration::from_secs(120);

    async fn read_error(guard: &mut Guard<tokio::io::DuplexStream>) -> std::io::ErrorKind {
        let mut buf = [0u8; 16];
        loop {
            match guard.read(&mut buf).await {
                Ok(0) => panic!("closed"),
                Ok(_) => {}
                Err(error) => return error.kind(),
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_new_connection_gets_the_headers_time_and_a_408() {
        let (near, mut far) = tokio::io::duplex(256);
        let mut guard = Guard::new(near, Phases::new(HEADERS, KEEP_ALIVE, IDLE));
        // The client reads the 408 to the end, then closes
        let client = tokio::spawn(async move {
            let mut said = Vec::new();
            far.read_to_end(&mut said).await.unwrap();
            said
        });
        let started = Instant::now();
        assert_eq!(read_error(&mut guard).await, std::io::ErrorKind::TimedOut);
        assert_eq!(started.elapsed(), HEADERS);
        let said = client.await.unwrap();
        assert!(said.starts_with(b"HTTP/1.1 408 "));
        assert!(said.ends_with(b"{\"error\":\"request_timeout\"}"));
    }

    #[tokio::test(start_paused = true)]
    async fn after_the_408_the_rest_of_a_slow_head_is_read_for_at_most_a_second() {
        let (near, mut far) = tokio::io::duplex(256);
        let mut guard = Guard::new(near, Phases::new(HEADERS, KEEP_ALIVE, IDLE));
        far.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();
        let started = Instant::now();
        let reading = tokio::spawn(async move {
            let error = read_error(&mut guard).await;
            (error, guard)
        });
        // Still sending after the 408: read and dropped, so the close does not reset the connection
        tokio::time::sleep(HEADERS + Duration::from_millis(500)).await;
        far.write_all(b"X-Slow: 1\r\n").await.unwrap();
        let (error, _guard) = reading.await.unwrap();
        assert_eq!(error, std::io::ErrorKind::TimedOut);
        assert_eq!(started.elapsed(), HEADERS + Duration::from_secs(1));
        let mut said = Vec::new();
        far.read_to_end(&mut said).await.unwrap();
        assert!(said.starts_with(b"HTTP/1.1 408 "));
    }

    #[tokio::test(start_paused = true)]
    async fn an_idle_keep_alive_connection_waits_the_keep_alive_time() {
        // Room for the 408 the slow head earns
        let (near, mut far) = tokio::io::duplex(256);
        let phases = Phases::new(HEADERS, KEEP_ALIVE, IDLE);
        let mut guard = Guard::new(near, phases.clone());
        phases.request_started();
        phases.request_done();
        let started = Instant::now();
        // A slow head: its first byte after 60 s, then nothing; the headers time runs from that byte
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            far.write_all(b"G").await.unwrap();
            tokio::time::sleep(Duration::from_secs(3600)).await;
            drop(far);
        });
        assert_eq!(read_error(&mut guard).await, std::io::ErrorKind::TimedOut);
        // The 408 at 70 s, then a second for a client that never closes
        assert_eq!(started.elapsed(), Duration::from_secs(71));
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_in_progress_has_only_the_idle_time_and_a_websocket_its_own() {
        let (near, _far) = tokio::io::duplex(64);
        let phases = Phases::new(HEADERS, KEEP_ALIVE, IDLE);
        let mut guard = Guard::new(near, phases.clone());
        phases.request_started();
        let started = Instant::now();
        assert_eq!(read_error(&mut guard).await, std::io::ErrorKind::TimedOut);
        assert_eq!(started.elapsed(), IDLE);

        let (near, _far) = tokio::io::duplex(64);
        let phases = Phases::new(HEADERS, KEEP_ALIVE, IDLE);
        let mut guard = Guard::new(near, phases.clone());
        phases.upgraded(Duration::from_secs(300));
        let started = Instant::now();
        assert_eq!(read_error(&mut guard).await, std::io::ErrorKind::TimedOut);
        assert_eq!(started.elapsed(), Duration::from_secs(300));
    }
}
