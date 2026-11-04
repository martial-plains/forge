// SPDX-License-Identifier: MIT OR Apache-2.0
//! HTTP requests Forge makes for a plugin (`forge:editor/http`).
//!
//! A plugin never touches the network itself: it names the `http`
//! permission and Forge sends the request. The synchronous form
//! (`http.get`, `http.request`) runs the request and waits, up to
//! [`TIMEOUT`]. The asynchronous form (`http.get-async`,
//! `http.request-async`) returns a request id at once; the answer is
//! collected by [`HttpRequests::poll`] and delivered to the plugin's
//! `http-handler.on-response` when the editor is idle, like a process's
//! output. Requests belong to the plugin that made them and are dropped
//! when it unloads.

use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

/// How long a single request may take before it's given up as failed.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// A request's id, handed to the plugin by `get-async` / `request-async`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RequestId(pub u32);

/// An answer, as the plugin's `http-handler.on-response` receives it:
/// the response, or the reason it failed.
pub type Answer = Result<Response, String>;

/// A server's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// One finished request, waiting to be delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpEvent {
    pub id: RequestId,
    pub answer: Answer,
}

/// Send `method` to `url` and read the answer. Blocking: the caller is
/// responsible for running it off the thread that must not wait (a plain
/// OS thread, in both the synchronous and asynchronous paths).
pub fn run(method: &str, url: &str, headers: &[(String, String)], body: Option<Vec<u8>>) -> Answer {
    let method = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|_| format!("not an HTTP method: {method}"))?;
    let client = reqwest::blocking::Client::builder()
        .user_agent(format!("Forge/{}", env!("CARGO_PKG_VERSION")))
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| format!("can't set up the HTTP client: {e}"))?;
    let mut req = client.request(method, url);
    for (name, value) in headers {
        req = req.header(name, value);
    }
    if let Some(b) = body {
        req = req.body(b);
    }
    let resp = req.send().map_err(|e| format!("{url}: {e}"))?;
    let status = resp.status();
    // A 4xx/5xx is an error, like curl --fail: the caller usually wants
    // to know the fetch failed, not to read an error page.
    if status.is_client_error() || status.is_server_error() {
        return Err(format!("{url}: HTTP {}", status.as_u16()));
    }
    let headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_string(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect();
    let body = resp
        .bytes()
        .map_err(|e| format!("{url}: reading the body: {e}"))?
        .to_vec();
    Ok(Response {
        status: status.as_u16(),
        headers,
        body,
    })
}

/// A plugin's in-flight and finished asynchronous requests.
pub struct HttpRequests {
    /// Ids that are still running: kept so `is_running`/`cancel` know them.
    open: BTreeMap<RequestId, ()>,
    next: u32,
    tx: Sender<HttpEvent>,
    rx: Receiver<HttpEvent>,
}

impl Default for HttpRequests {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            open: BTreeMap::new(),
            next: 1,
            tx,
            rx,
        }
    }
}

impl std::fmt::Debug for HttpRequests {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpRequests")
            .field("running", &self.open.len())
            .finish()
    }
}

impl HttpRequests {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start `method url` in the background; the answer comes later
    /// through [`poll`](Self::poll).
    pub fn start(
        &mut self,
        method: &str,
        url: &str,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
    ) -> RequestId {
        let id = RequestId(self.next);
        self.next += 1;
        self.open.insert(id, ());
        let tx = self.tx.clone();
        let method = method.to_string();
        let url = url.to_string();
        std::thread::Builder::new()
            .name("forge-plugin-http".into())
            .spawn(move || {
                let answer = run(&method, &url, &headers, body);
                // If the plugin unloaded (or cancelled), the receiver is
                // gone: the send just fails, and the thread ends.
                let _ = tx.send(HttpEvent { id, answer });
            })
            .ok();
        id
    }

    pub fn is_running(&self, id: RequestId) -> bool {
        self.open.contains_key(&id)
    }

    pub fn any_running(&self) -> bool {
        !self.open.is_empty()
    }

    /// Give up on `id`: its answer is dropped and never delivered.
    pub fn cancel(&mut self, id: RequestId) -> bool {
        self.open.remove(&id).is_some()
    }

    /// Answers ready since the last call, in the order they finished.
    /// A cancelled request's answer is dropped.
    pub fn poll(&mut self) -> Vec<HttpEvent> {
        let mut events = Vec::new();
        while let Ok(ev) = self.rx.try_recv() {
            if self.open.remove(&ev.id).is_some() {
                events.push(ev);
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_arrives_after_polling() {
        let mut r = HttpRequests::new();
        // No network in tests: drive the channel directly through a
        // request-shaped send by starting one to a file URL, which fails
        // fast, then poll until it lands.
        let id = r.start("GET", "file:///nonexistent-forge-test", Vec::new(), None);
        assert!(r.is_running(id));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut got = Vec::new();
        while got.is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
            got = r.poll();
        }
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, id);
        assert!(got[0].answer.is_err(), "a missing file is an error");
        assert!(!r.is_running(id), "finished requests leave the running set");
        assert!(!r.any_running());
    }

    #[test]
    fn a_cancelled_request_is_dropped() {
        let mut r = HttpRequests::new();
        let id = r.start("GET", "file:///nonexistent-forge-test", Vec::new(), None);
        assert!(r.cancel(id));
        // Even after the thread finishes, poll yields nothing for it.
        std::thread::sleep(Duration::from_millis(100));
        assert!(r.poll().is_empty());
        assert!(!r.any_running());
        assert!(!r.cancel(id), "cancelling twice is a no-op");
    }
}
