use std::{net::SocketAddr, sync::Arc};

use axum::{
    extract::{
        ws::{Message, WebSocket},
        ConnectInfo, State, WebSocketUpgrade,
    },
    response::Response,
    routing::any,
    Router,
};
use futures::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::{
    net::TcpListener,
    sync::{broadcast, Mutex},
};
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().pretty())
        .init();

    let (tx, _rx) = broadcast::channel::<ChatMessage>(10);
    let app = Router::new()
        .route("/ws", any(ws_handler))
        .layer(TraceLayer::new_for_http())
        .with_state(Arc::new(Mutex::new(tx)));

    let listener = TcpListener::bind("127.0.0.1:3000").await.unwrap();
    tracing::info!("listening on {}", listener.local_addr().unwrap());
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();
}
#[derive(Clone, Debug, Serialize, Deserialize)]
enum ChatMessage {
    Message(UserMessage),
    Join(String),
    Leave(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct UserMessage {
    user: String,
    to: Option<Vec<String>>,
    message: String,
}

async fn ws_handler(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
    State(state): State<Arc<Mutex<broadcast::Sender<ChatMessage>>>>,
) -> Response {
    ws.on_upgrade(move |socket| handle_socket(addr, socket, state))
}

async fn handle_socket(
    addr: SocketAddr,
    socket: WebSocket,
    state: Arc<Mutex<broadcast::Sender<ChatMessage>>>,
) {
    let (socket_tx, socket_rx) = socket.split();

    let tx = state.lock().await.clone();
    let rx = tx.subscribe();

    tokio::spawn(write(addr, socket_tx, rx));
    tokio::spawn(read(addr, socket_rx, tx));

    let chat_message = ChatMessage::Join(format!("welcome {} joined!", addr).to_string());
    let chat = ChatMessage::Message(UserMessage {
        user: "System".to_string(),
        to: Some(vec![addr.to_string().into()]),
        message: "welcome joined!".to_string(),
    });
    let tx = state.lock().await.clone();
    tx.send(chat_message).unwrap();
    tx.send(chat).unwrap();
}

async fn read(
    addr: SocketAddr,
    mut receiver: SplitStream<WebSocket>,
    tx: broadcast::Sender<ChatMessage>,
) {
    while let Some(message) = receiver.next().await {
        match message {
            Ok(Message::Text(text)) => {
                let mut message: UserMessage = serde_json::from_str(&text).unwrap();
                message.user = addr.to_string();
                let chat_message = ChatMessage::Message(message);
                tx.send(chat_message).unwrap();
            }
            Ok(Message::Close(_)) => {
                let chat_message = ChatMessage::Leave(format!("{} Leave!", addr).to_string());
                tx.send(chat_message).unwrap();
            }
            _ => {}
        }
    }
}

async fn write(
    addr: SocketAddr,
    mut sender: SplitSink<WebSocket, Message>,
    mut receiver: broadcast::Receiver<ChatMessage>,
) {
    while let Ok(message) = receiver.recv().await {
        let message = match message {
            ChatMessage::Message(message) => {
                if let Some(ref to) = message.to {
                    if !to.contains(&addr.to_string()) {
                        continue;
                    }
                }
                Message::Text(message.message.into())
            }
            ChatMessage::Join(user) => {
                let message = json!(user);
                Message::Text(message.to_string().into())
            }
            ChatMessage::Leave(user) => {
                let message = json!(user);
                Message::Text(message.to_string().into())
            }
        };
        if let Err(_) = sender.send(message).await {
            break;
        }
    }
}
