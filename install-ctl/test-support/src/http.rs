use crate::cgi::git_http_backend;
use serde_json::json;
use std::{
    collections::HashMap,
    fs,
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use tiny_http::{Header, Response, Server, StatusCode};

pub struct HttpFixture {
    pub url: String,
    stopped: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    workers: Arc<Mutex<Vec<JoinHandle<()>>>>,
    address: SocketAddr,
}

impl HttpFixture {
    pub fn new(root: &Path, log: PathBuf, mode: &str) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback fixture");
        let address = listener.local_addr().expect("fixture address");
        let server = Server::from_listener(listener, None).expect("start loopback fixture");
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&stopped);
        let root = root.to_owned();
        let mode = mode.to_owned();
        let workers = Arc::new(Mutex::new(Vec::new()));
        let server_workers = Arc::clone(&workers);
        let thread = thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                let Some(mut request) = server
                    .recv_timeout(Duration::from_millis(25))
                    .expect("receive fixture request")
                else {
                    continue;
                };
                let root = root.clone();
                let log = log.clone();
                let mode = mode.clone();
                let stop = Arc::clone(&stop);
                server_workers.lock().expect("lock fixture workers").push(thread::spawn(move || {
                    let method = request.method().as_str().to_owned();
                    let path = request.url().to_owned();
                    let headers: HashMap<_, _> = request.headers().iter().map(|header| {
                        (header.field.as_str().as_str().to_ascii_lowercase(), header.value.as_str().to_owned())
                    }).collect();
                    let mut body = Vec::new();
                    request.as_reader().read_to_end(&mut body).expect("read fixture body");
                    let authorized = headers.get("authorization").is_some_and(|value| {
                        value == "Basic Zml4dHVyZS11c2VyOmZpeHR1cmUtcGFzc3dvcmQ="
                    });
                    let record = json!({"method": method, "path": path, "authorized": authorized,
                        "fixture_header": headers.get("x-transport-fixture"), "second_header": headers.get("x-transport-second"),
                        "body": String::from_utf8_lossy(&body)});
                    use std::io::Write;
                    let record = serde_json::to_string_pretty(&record)
                        .expect("serialize structured request evidence")
                        .lines()
                        .map(str::trim)
                        .collect::<String>();
                    writeln!(fs::OpenOptions::new().create(true).append(true).open(&log).expect("open request log"), "{record}").expect("write request log");
                    if mode == "mutable-access" && root.join("deny-access").exists() {
                        let _ = request.respond(Response::empty(StatusCode(403)));
                    } else if mode == "stall-handshake" || (mode == "stall-fetch" && body.windows(b"command=fetch".len()).any(|part| part == b"command=fetch")) {
                        while !stop.load(Ordering::Acquire) { thread::sleep(Duration::from_millis(10)); }
                    } else if mode == "auth" && !authorized {
                        let response = Response::empty(StatusCode(401)).with_header(Header::from_bytes("WWW-Authenticate", "Basic realm=\"fixture\"").unwrap());
                        let _ = request.respond(response);
                    } else {
                        match git_http_backend(&root, &method, &path, &headers, &body) {
                            Ok((status, headers, payload)) => {
                                let mut response = Response::from_data(payload).with_status_code(StatusCode(status));
                                for (name, value) in headers {
                                    response = response.with_header(Header::from_bytes(name, value).expect("valid CGI header"));
                                }
                                let _ = request.respond(response);
                            }
                            Err(error) => { let _ = request.respond(Response::from_string(error).with_status_code(StatusCode(500))); }
                        }
                    }
                }));
            }
        });
        Self {
            url: format!("http://{address}/repo.git"),
            stopped,
            thread: Some(thread),
            workers,
            address,
        }
    }
}

impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.thread
            .take()
            .expect("fixture thread")
            .join()
            .expect("reap fixture thread");
        for worker in self.workers.lock().expect("lock fixture workers").drain(..) {
            worker.join().expect("reap fixture worker");
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while TcpStream::connect_timeout(&self.address, Duration::from_millis(10)).is_ok() {
            assert!(
                std::time::Instant::now() < deadline,
                "fixture listener did not close during owned shutdown"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}
