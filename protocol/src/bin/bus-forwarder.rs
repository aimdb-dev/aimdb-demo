//! Local stand-in for the relay's `/bus/<code>`: same join and topic rules,
//! no limits, no logging discipline. `bus-forwarder [port | addr:port]`,
//! default 127.0.0.1:9312; `0.0.0.0:9312` lets a phone on the LAN reach it.
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;

type Members = Arc<Mutex<HashMap<(String, String), UnboundedSender<Message>>>>;

#[tokio::main]
async fn main() {
    let arg = std::env::args().nth(1).unwrap_or_else(|| "9312".into());
    // A bare port listens on loopback; `addr:port` (e.g. 0.0.0.0:9312) as given.
    let addr = if arg.contains(':') {
        arg
    } else {
        format!("127.0.0.1:{arg}")
    };
    let listener = TcpListener::bind(&addr).await.unwrap();
    eprintln!("bus-forwarder on {addr}");
    let members: Members = Arc::default();
    loop {
        let (stream, _) = listener.accept().await.unwrap();
        let members = members.clone();
        tokio::spawn(async move {
            let mut path = String::new();
            let ws =
                tokio_tungstenite::accept_hdr_async(stream, |req: &Request, resp: Response| {
                    path = req.uri().to_string();
                    Ok(resp)
                })
                .await;
            let Ok(ws) = ws else { return };
            // /bus/<code>?as=<role>
            let Some(rest) = path.strip_prefix("/bus/") else {
                return;
            };
            let Some((code, role)) = rest.split_once("?as=") else {
                return;
            };
            let (code, role) = (code.to_string(), role.to_string());
            if !line_protocol::valid_role(&role) {
                eprintln!("refuse {code}: role {role:?}");
                return;
            }
            let (mut sink, mut stream) = ws.split();
            let (tx, mut rx) = unbounded_channel::<Message>();
            let taken = {
                let mut m = members.lock().unwrap();
                let key = (code.clone(), role.clone());
                let taken = m.contains_key(&key);
                if !taken {
                    m.insert(key, tx);
                }
                taken
            };
            if taken {
                {
                    let _ = sink
                        .send(Message::Close(Some(CloseFrame {
                            code: CloseCode::Library(line_protocol::CLOSE_TAKEN),
                            reason: "taken".into(),
                        })))
                        .await;
                    return;
                }
            }
            eprintln!("join {code} as {role}");
            let writer = tokio::spawn(async move {
                while let Some(m) = rx.recv().await {
                    if sink.send(m).await.is_err() {
                        break;
                    }
                }
            });
            while let Some(Ok(msg)) = stream.next().await {
                let Message::Binary(bytes) = &msg else {
                    continue;
                };
                // A place speaks only for itself; the payload stays unread.
                let allowed = line_protocol::decode(bytes)
                    .is_some_and(|(topic, _)| line_protocol::may_send(&role, topic));
                if !allowed {
                    eprintln!("drop {code} from {role}: topic not under its place");
                    continue;
                }
                // From the hub: to the place the topic names, or to all.
                let target = line_protocol::decode(bytes)
                    .and_then(|(topic, _)| line_protocol::hq_target(topic));
                let m = members.lock().unwrap();
                for ((c, r), tx) in m.iter() {
                    let to_hq = role != "hq" && r == "hq";
                    let from_hq = role == "hq" && r != "hq" && target.is_none_or(|t| t == r);
                    if *c == code && (to_hq || from_hq) {
                        let _ = tx.send(msg.clone());
                    }
                }
            }
            members
                .lock()
                .unwrap()
                .remove(&(code.clone(), role.clone()));
            writer.abort();
            eprintln!("leave {code} as {role}");
        });
    }
}
