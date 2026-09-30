use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::{Html, Response};
use axum::routing::get;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::input::InputSender;
use crate::video::{Framebuffer, VideoStream};

/// How often each client checks for a new frame; capture itself runs at CAPTURE_FPS.
const FRAME_CHECK: Duration = Duration::from_millis(50);

#[derive(Clone)]
struct AppState {
    framebuffer: Arc<Mutex<Framebuffer>>,
    input: InputSender,
}

/// Serve the browser client on its own tokio runtime thread.
pub fn spawn_server(addr: String, framebuffer: Arc<Mutex<Framebuffer>>, input: InputSender) {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().expect("failed to start tokio runtime");
        runtime.block_on(async move {
            let app = Router::new()
                .route("/", get(index))
                .route("/ws", get(ws_upgrade))
                .with_state(AppState { framebuffer, input });
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

async fn ws_upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| client_session(socket, state))
}

/// Pushes JPEG frames as binary messages; receives key events as JSON text:
/// `{"type":"key","code":"<qcode>","down":true,"repeat":false}` or `{"type":"release_all"}`.
async fn client_session(mut socket: WebSocket, state: AppState) {
    let mut video = VideoStream::new(state.framebuffer.clone());
    let mut ticker = tokio::time::interval(FRAME_CHECK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if let Some(jpeg) = video.next_jpeg().await {
                    if socket.send(Message::Binary(jpeg.into())).await.is_err() {
                        break;
                    }
                }
            }
            message = socket.recv() => match message {
                Some(Ok(Message::Text(text))) => handle_client_message(&state.input, text.as_str()),
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            }
        }
    }

    // A closed tab must not leave keys stuck down in the guest.
    let _ = state.input.send(Box::new(|keyboard| keyboard.release_all()));
}

fn handle_client_message(input: &InputSender, text: &str) {
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
        _ => eprintln!("web: unknown message: {text}"),
    }
}
