use std::collections::{HashMap, VecDeque};

use futures_util::StreamExt;
use irc::client::prelude::*;
use color_eyre::eyre::{eyre, Result};
use tokio::task;
use tracing::{info, trace};

enum State {
    SkipAudio(usize),
    MetaHeader,
    CaptureMeta(usize),
}

impl State {
    fn bytes_to_consume(&self) -> usize {
        match *self {
            Self::SkipAudio(n) | Self::CaptureMeta(n) => n,
            Self::MetaHeader => 1,
        }
    }
}

fn decode_meta(meta: &String) -> HashMap<String, String> {
    meta
        .split("';")
        .map(|s| s.trim().trim_matches('\0'))
        .filter(|s| !s.is_empty())
        .filter(|s| s.contains("='"))
        .map(|s| s.splitn(2, "='"))
        .map(|mut s| {
            let k = s.next().unwrap();
            let v = s.next().unwrap();
            (k.to_owned(), v.to_owned())
        })
        .collect()
}

async fn proc_notifier(sender: Sender) -> Result<()> {
    let cl = reqwest::Client::new();
    let res = cl.get("http://sleepy.zone:8000/blissomradio")
        .header("Icy-MetaData", "1")
        .send()
        .await?;
    let metaint: usize = res.headers().get("icy-metaint").ok_or_else(|| eyre!("no icy-metaint resp header"))?.to_str()?.parse()?;
    info!(metaint = metaint, "connected to stream");

    let mut stream = res.bytes_stream();

    let mut state = State::SkipAudio(metaint);
    let mut data: VecDeque<u8> = VecDeque::with_capacity(metaint);
    while let Some(chunk) = stream.next().await.transpose()? {
        info!("chunk");
        data.extend(chunk);
        while data.len() >= state.bytes_to_consume() {
            state = match state {
                State::SkipAudio(n) => {
                    let _ = data.drain(0..n);
                    State::MetaHeader
                }

                State::MetaHeader => {
                    State::CaptureMeta(data.pop_front().unwrap() as usize * 16)
                }

                State::CaptureMeta(n) => {
                    let meta: String = String::from_utf8(data.drain(0..n).collect())?;

                    if meta.len() > 0 {
                        let meta = decode_meta(&meta);
                        info!(meta=?meta, "metadata");
                        if let Some(title) = meta.get("StreamTitle") {
                            sender.send_privmsg("#sleepyfm", format!("now playing: {}", title))?;
                        }
                    }

                    State::SkipAudio(metaint)
                }
            }
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt::init();

    let config = Config {
        nickname: Some("Hecate".to_owned()),
        server: Some("irc.sleepy.zone".to_owned()),
        port: Some(6667),
        use_tls: Some(false),
        channels: vec!["#sleepyfm".to_owned()],
        ..Config::default()
    };

    let mut client = Client::from_config(config).await?;
    client.identify()?;

    let mut stream = client.stream()?;
    let sender = client.sender();

    let _jh = task::spawn(proc_notifier(sender.clone()));

    while let Some(message) = stream.next().await.transpose()? {
        match message.command {
            Command::PRIVMSG(ref tgt, ref msg) => {
                if msg.contains(client.current_nickname()) {
                    sender.send_privmsg(tgt, "hi!")?;
                }
            },
            _ => (),
        }
    }

    Ok(())
}
