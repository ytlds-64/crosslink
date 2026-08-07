use anyhow::{anyhow, Context, Result};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::io::{split, AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::time::interval;

use crate::input;
use crate::net::crypto::{self, Crypter, StaticSecret};
use crate::net::protocol::Message;

/// 服务端：绑定并循环接受连接（单连接依次处理，断开后等待下一个）。
pub async fn run_server(bind: &str, port: u16, key: &StaticSecret, name: &str) -> Result<()> {
    let listener = TcpListener::bind((bind, port))
        .await
        .with_context(|| format!("bind {}:{}", bind, port))?;
    log::info!("server listening on {}:{}", bind, port);

    loop {
        let (stream, peer) = listener.accept().await?;
        log::info!("accepted connection from {}", peer);
        if let Err(e) = session_server(stream, key, name).await {
            log::error!("session ended with error: {:?}", e);
        }
        log::info!("session closed, waiting for next connection");
    }
}

/// 客户端：连接到服务端并进入会话。
pub async fn run_client(
    addr: &str,
    port: u16,
    fingerprint: Option<&str>,
    name: &str,
) -> Result<()> {
    let stream = TcpStream::connect((addr, port))
        .await
        .with_context(|| format!("connect {}:{}", addr, port))?;
    log::info!("connected to {}:{}", addr, port);
    session_client(stream, fingerprint, name).await
}

async fn session_server(mut stream: TcpStream, key: &StaticSecret, name: &str) -> Result<()> {
    let (crypter, _fp) = crypto::server_handshake(&mut stream, key).await?;
    let (rd, wr) = split(stream);
    session(rd, wr, crypter, name).await
}

async fn session_client(mut stream: TcpStream, fingerprint: Option<&str>, name: &str) -> Result<()> {
    let (crypter, fp) = crypto::client_handshake(&mut stream, fingerprint).await?;
    log::info!("handshake complete, server fingerprint = {}", fp);
    let (rd, wr) = split(stream);
    session(rd, wr, crypter, name).await
}

/// 加密会话主循环：互相发送心跳，处理 Hello / Heartbeat / HeartbeatAck。
async fn session(
    mut rd: ReadHalf<TcpStream>,
    wr: WriteHalf<TcpStream>,
    crypter: Crypter,
    name: &str,
) -> Result<()> {
    let wr = Arc::new(Mutex::new(wr));
    let crypter = Arc::new(Mutex::new(crypter));

    send_msg(
        &wr,
        &crypter,
        &Message::Hello {
            name: name.to_string(),
            platform: input::platform().to_string(),
        },
    )
    .await?;

    let mut hb = interval(Duration::from_secs(3));
    loop {
        tokio::select! {
            res = recv_msg(&mut rd, &crypter) => {
                match res {
                    Ok(msg) => handle(msg, &wr, &crypter).await?,
                    Err(e) => {
                        log::error!("recv error: {:?}", e);
                        break;
                    }
                }
            }
            _ = hb.tick() => {
                let t = now_ms();
                if let Err(e) = send_msg(&wr, &crypter, &Message::Heartbeat { t }).await {
                    log::error!("send heartbeat error: {:?}", e);
                    break;
                }
            }
        }
    }
    Ok(())
}

async fn handle(
    msg: Message,
    wr: &Arc<Mutex<WriteHalf<TcpStream>>>,
    crypter: &Arc<Mutex<Crypter>>,
) -> Result<()> {
    match msg {
        Message::Heartbeat { t } => {
            send_msg(wr, crypter, &Message::HeartbeatAck { t }).await?;
        }
        Message::HeartbeatAck { t } => {
            log::info!("heartbeat ack, rtt = {} ms", now_ms().saturating_sub(t));
        }
        Message::Hello { name, platform } => {
            log::info!("peer hello: {} ({})", name, platform);
        }
    }
    Ok(())
}

/// 发送一帧加密消息。帧格式：`[u32 长度][u64 计数器][密文+GCM标签]`。
async fn send_msg(
    wr: &Arc<Mutex<WriteHalf<TcpStream>>>,
    crypter: &Arc<Mutex<Crypter>>,
    msg: &Message,
) -> Result<()> {
    let plaintext = bincode::serialize(msg)?;
    let (ctr, ct) = {
        let mut c = crypter.lock().await;
        c.encrypt(&plaintext)
    };

    let mut frame = Vec::with_capacity(4 + 8 + ct.len());
    frame.extend_from_slice(&((8 + ct.len()) as u32).to_be_bytes());
    frame.extend_from_slice(&ctr.to_be_bytes());
    frame.extend_from_slice(&ct);

    let mut w = wr.lock().await;
    w.write_all(&frame).await?;
    w.flush().await?;
    Ok(())
}

/// 接收并解密一帧消息。
async fn recv_msg(
    rd: &mut ReadHalf<TcpStream>,
    crypter: &Arc<Mutex<Crypter>>,
) -> Result<Message> {
    let len = read_u32(rd).await? as usize;
    if len < 8 {
        return Err(anyhow!("frame too small: {}", len));
    }
    let mut body = vec![0u8; len];
    rd.read_exact(&mut body).await?;

    let ctr = u64::from_be_bytes(body[0..8].try_into().map_err(|_| anyhow!("bad ctr"))?);
    let ciphertext = &body[8..];

    let plaintext = {
        let mut c = crypter.lock().await;
        c.decrypt(ctr, ciphertext)?
    };
    let msg: Message = bincode::deserialize(&plaintext)?;
    Ok(msg)
}

async fn read_u32(rd: &mut ReadHalf<TcpStream>) -> Result<u32> {
    let mut b = [0u8; 4];
    rd.read_exact(&mut b).await?;
    Ok(u32::from_be_bytes(b))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
