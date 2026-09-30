use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::StatusCode;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::{Html, Json, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::input::InputSender;
use crate::session::{self, SharedQmp};
use crate::video::{Framebuffer, VideoStream};

/// How many `name-N.img` variants a dropped file may get before the drop is refused.
const MAX_NAME_SUFFIX: u32 = 999;

/// Largest floppy image (2.88M) plus headroom; axum's default body limit is 2 MB.
const MAX_UPLOAD: usize = 3 * 1024 * 1024;

/// How often each client checks for a new frame; QEMU pushes about 30 a second. Checking
/// faster than that shortens the wait after an ack; unchanged frames cost nothing.
const FRAME_CHECK: Duration = Duration::from_millis(10);
/// Sent by the page once a frame is on screen.
const FRAME_ACK: &str = r#"{"type":"frame"}"#;
/// Send anyway if a frame goes unacknowledged this long.
const ACK_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone)]
struct AppState {
    framebuffer: Arc<Mutex<Framebuffer>>,
    input: InputSender,
    qmp: SharedQmp,
    /// Floppy images offered by the disk picker; clients pick by file name only.
    disk_dir: Arc<PathBuf>,
}

/// Serve the browser client on its own tokio runtime thread.
pub fn spawn_server(
    addr: String,
    framebuffer: Arc<Mutex<Framebuffer>>,
    input: InputSender,
    qmp: SharedQmp,
    disk_dir: PathBuf,
) {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().expect("failed to start tokio runtime");
        runtime.block_on(async move {
            let app = Router::new()
                .route("/", get(index))
                .route("/ws", get(ws_upgrade))
                .route("/disks", get(disks))
                .route("/disks/{name}", post(upload_disk).layer(DefaultBodyLimit::max(MAX_UPLOAD)))
                .with_state(AppState { framebuffer, input, qmp, disk_dir: Arc::new(disk_dir) });
            let listener = match tokio::net::TcpListener::bind(&addr).await {
                Ok(listener) => listener,
                Err(err) => return eprintln!("web: cannot listen on {addr}: {err}"),
            };
            println!("Web client: http://{}", listener.local_addr().unwrap());
            if let Err(err) = axum::serve(listener, app).await {
                eprintln!("web: server stopped: {err}");
            }
        });
    });
}

async fn index() -> Html<&'static str> {
    Html(include_str!("index.html"))
}

async fn disks(State(state): State<AppState>) -> Json<Value> {
    Json(json!(session::list_floppies(&state.disk_dir)))
}

/// Drag-and-drop target: saves a floppy image into disk_dir and inserts it in A:.
/// Re-dropping an identical file reuses it. A different file never overwrites one: it gets the
/// next free name (floppy.img, floppy-2.img, ...), which is returned so the page can show it.
async fn upload_disk(
    State(state): State<AppState>,
    Path(name): Path<String>,
    body: Bytes,
) -> Result<Json<Value>, (StatusCode, String)> {
    let bad_request = |message: String| (StatusCode::BAD_REQUEST, message);
    let valid_name = !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    if !valid_name {
        return Err(bad_request(format!("use a plain file name (letters, digits, . _ -): {name}")));
    }
    if !session::is_floppy_size(body.len() as u64) {
        return Err(bad_request(format!(
            "{name} is {} bytes, not a floppy image (360K, 720K, 1.2M, 1.44M or 2.88M)",
            body.len()
        )));
    }

    let internal = |err: std::io::Error| (StatusCode::INTERNAL_SERVER_ERROR, format!("saving {name}: {err}"));
    tokio::fs::create_dir_all(state.disk_dir.as_ref()).await.map_err(internal)?;
    let (stem, ext) = name.rsplit_once('.').map_or((name.as_str(), ""), |(stem, ext)| (stem, ext));
    let mut saved = None;
    for n in 1..=MAX_NAME_SUFFIX {
        let candidate = match (n, ext) {
            (1, _) => name.clone(),
            (_, "") => format!("{stem}-{n}"),
            _ => format!("{stem}-{n}.{ext}"),
        };
        let path = state.disk_dir.join(&candidate);
        match tokio::fs::read(&path).await {
            Ok(existing) if existing == body => {}
            Ok(_) => continue,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                tokio::fs::write(&path, &body).await.map_err(internal)?;
                println!("web: saved dropped disk {}", path.display());
            }
            Err(err) => return Err(internal(err)),
        }
        saved = Some((candidate, path));
        break;
    }
    let Some((name, path)) = saved else {
        return Err((StatusCode::CONFLICT, format!("too many different disks named like {name}; rename the file")));
    };

    let qmp = state.qmp.clone();
    let insert_path = path.clone();
    tokio::task::spawn_blocking(move || qmp.lock().unwrap().change_floppy(&insert_path).map_err(|err| err.to_string()))
        .await
        .map_err(|err| err.to_string())
        .and_then(|result| result)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, format!("inserting {name}: {err}")))?;
    println!("web: A: now holds {}", path.display());
    Ok(Json(json!({ "name": name })))
}

async fn ws_upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| client_session(socket, state))
}

/// Pushes changed screen areas as binary messages (see VideoStream::next_message), one at a time; receives JSON text: `{"type":"frame"}`
/// once a frame is shown,
/// `{"type":"key","code":"<qcode>","down":true,"repeat":false}`, `{"type":"release_all"}`,
/// `{"type":"disk","name":"<file in disk_dir>"}` or `{"type":"eject"}`.
async fn client_session(mut socket: WebSocket, state: AppState) {
    let mut video = VideoStream::new(state.framebuffer.clone());
    let mut ticker = tokio::time::interval(FRAME_CHECK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // One frame in flight: the next is sent only after the page shows this one ({"type":"frame"}).
    // Without this, frames pile up in the socket whenever the browser draws slower than we send,
    // and the picture (and so every key press) falls further and further behind.
    let mut in_flight_since: Option<Instant> = None;

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                // A lost ack (e.g. a page from before this protocol) must not freeze the picture.
                if in_flight_since.is_some_and(|sent| sent.elapsed() < ACK_TIMEOUT) {
                    continue;
                }
                if let Some(frame) = video.next_message().await {
                    if socket.send(Message::Binary(frame.into())).await.is_err() {
                        break;
                    }
                    in_flight_since = Some(Instant::now());
                }
            }
            message = socket.recv() => match message {
                Some(Ok(Message::Text(text))) if text.as_str() == FRAME_ACK => in_flight_since = None,
                Some(Ok(Message::Text(text))) => handle_client_message(&state, text.as_str()),
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            }
        }
    }

    // A closed tab must not leave keys stuck down in the guest.
    let _ = state.input.send(Box::new(|keyboard| keyboard.release_all()));
}

fn handle_client_message(state: &AppState, text: &str) {
    let input = &state.input;
    let Ok(message) = serde_json::from_str::<Value>(text) else {
        return eprintln!("web: bad message: {text}");
    };
    match message["type"].as_str() {
        Some("key") => {
            let (Some(code), Some(down)) = (message["code"].as_str(), message["down"].as_bool()) else {
                return eprintln!("web: bad key message: {text}");
            };
            let repeat = message["repeat"].as_bool().unwrap_or(false);
            let code = code.to_string();
            let _ = input.send(Box::new(move |keyboard| {
                let result = match (down, repeat) {
                    (true, true) => keyboard.key_repeat(&code),
                    (true, false) => keyboard.key_down(&code),
                    (false, _) => keyboard.key_up(&code),
                };
                if let Err(err) = result {
                    eprintln!("web: key {code} failed: {err}");
                }
            }));
        }
        Some("release_all") => {
            let _ = input.send(Box::new(|keyboard| keyboard.release_all()));
        }
        Some("disk") => {
            let Some(name) = message["name"].as_str() else {
                return eprintln!("web: bad disk message: {text}");
            };
            // Only names from the picker's own listing, so a client can't point QEMU at other files.
            if !session::list_floppies(&state.disk_dir).iter().any(|listed| listed == name) {
                return eprintln!("web: not a floppy in {}: {name}", state.disk_dir.display());
            }
            let path = state.disk_dir.join(name);
            let qmp = state.qmp.clone();
            tokio::task::spawn_blocking(move || match qmp.lock().unwrap().change_floppy(&path) {
                Ok(()) => println!("web: A: now holds {}", path.display()),
                Err(err) => eprintln!("web: disk change failed: {err}"),
            });
        }
        Some("eject") => {
            let qmp = state.qmp.clone();
            tokio::task::spawn_blocking(move || {
                if let Err(err) = qmp.lock().unwrap().eject_floppy() {
                    eprintln!("web: eject failed: {err}");
                }
            });
        }
        _ => eprintln!("web: unknown message: {text}"),
    }
}
