use anyhow::{anyhow, Context, Result};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::io::{split, AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};
use tokio::time::interval;

use crate::input;
use crate::input::event::{InputEvent, KeyEvent, KeyState};
use crate::net::crypto::{self, Crypter, StaticSecret};
use crate::net::protocol::Message;

/// 启动一个服务端会话：绑定端口、接受单连接、加密握手、进入会话循环。
///
/// `enable_capture` 为 true 时，服务端会启动键盘捕获线程，将本地按键通过
/// `Message::Input` 发送给对端（**单向：server → client**）。
///
/// `test_input` 为 true 时，启动 500ms 后会发送一组 mock 键事件用于端到端链路
/// 验证（无需真实按键——沙箱/CI 友好）。
pub async fn run_server(
    bind: &str,
    port: u16,
    key: &StaticSecret,
    name: &str,
    enable_capture: bool,
    test_input: bool,
) -> Result<()> {
    let listener = TcpListener::bind((bind, port))
        .await
        .with_context(|| format!("bind {}:{}", bind, port))?;
    log::info!("server listening on {}:{}", bind, port);

    loop {
        let (stream, peer) = listener.accept().await?;
        log::info!("accepted connection from {}", peer);
        if let Err(e) = session_server(stream, key, name, enable_capture, test_input).await {
            log::error!("session ended with error: {:?}", e);
        }
        log::info!("session closed, waiting for next connection");
    }
}

/// 启动一个客户端会话：连接、加密握手、进入会话循环。
///
/// `enable_inject` 为 true 时，收到 `Message::Input` 会通过 `SendInput`
/// 注入到本机键盘队列。
pub async fn run_client(
    addr: &str,
    port: u16,
    fingerprint: Option<&str>,
    name: &str,
    enable_inject: bool,
) -> Result<()> {
    let stream = TcpStream::connect((addr, port))
        .await
        .with_context(|| format!("connect {}:{}", addr, port))?;
    log::info!("connected to {}:{}", addr, port);
    session_client(stream, fingerprint, name, enable_inject).await
}

async fn session_server(
    mut stream: TcpStream,
    key: &StaticSecret,
    name: &str,
    enable_capture: bool,
    test_input: bool,
) -> Result<()> {
    let (crypter, _fp) = crypto::server_handshake(&mut stream, key).await?;
    let (mut rd, wr) = split(stream);
    let wr = Arc::new(Mutex::new(wr));
    let crypter = Arc::new(Mutex::new(crypter));

    // 启动捕获桥：std mpsc (capture 线程) → tokio mpsc (async 任务)
    let (etx, mut erx) = mpsc::unbounded_channel::<InputEvent>();
    if enable_capture {
        let std_rx = input::capture::start_keyboard_capture();
        let etx_c = etx.clone();
        tokio::task::spawn_blocking(move || bridge_capture_to_tokio(std_rx, etx_c));
        log::info!("server: keyboard capture → wire enabled");
    } else {
        log::info!("server: capture disabled (--no-capture)");
    }

    // 测试模式：注入 mock 事件验证端到端链路（沙箱/CI 友好）
    if test_input {
        let etx_test = etx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            log::info!("test-input: sending 5 mock key events (a, b, 1, Tab, Enter)");
            // HID: 0x04=A, 0x05=B, 0x1E=1, 0x2B=Tab, 0x28=Enter
            let sequence: [(u16, KeyState); 10] = [
                (0x04, KeyState::Pressed),
                (0x05, KeyState::Pressed),
                (0x1E, KeyState::Pressed),
                (0x2B, KeyState::Pressed),
                (0x28, KeyState::Pressed),
                (0x28, KeyState::Released),
                (0x2B, KeyState::Released),
                (0x1E, KeyState::Released),
                (0x05, KeyState::Released),
                (0x04, KeyState::Released),
            ];
            for (h, s) in sequence {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let _ = etx_test.send(InputEvent::Key(KeyEvent { hid: h, state: s }));
            }
            log::info!("test-input: sequence done");
        });
    }
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
            // 捕获事件 → 发送到对端
            Some(ev) = erx.recv() => {
                if let Err(e) = send_msg(&wr, &crypter, &Message::Input(ev)).await {
                    log::error!("send input event error: {:?}", e);
                    break;
                }
            }
            // 接收对端消息
            res = recv_msg(&mut rd, &crypter) => {
                match res {
                    Ok(msg) => handle(msg).await?,
                    Err(e) => {
                        log::error!("recv error: {:?}", e);
                        break;
                    }
                }
            }
            // 心跳
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

async fn session_client(
    mut stream: TcpStream,
    fingerprint: Option<&str>,
    name: &str,
    enable_inject: bool,
) -> Result<()> {
    let (crypter, fp) = crypto::client_handshake(&mut stream, fingerprint).await?;
    log::info!("handshake complete, server fingerprint = {}", fp);
    let (mut rd, wr) = split(stream);
    let wr = Arc::new(Mutex::new(wr));
    let crypter = Arc::new(Mutex::new(crypter));

    // 启动注入 worker：从 tokio mpsc 拉事件，丢到 SendInput
    let (itx, irx) = mpsc::unbounded_channel::<InputEvent>();
    if enable_inject {
        tokio::task::spawn_blocking(move || bridge_tokio_to_inject(irx));
        log::info!("client: wire → SendInput inject enabled");
    } else {
        log::info!("client: inject disabled (--no-inject)");
    }

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
            // 接收对端消息
            res = recv_msg(&mut rd, &crypter) => {
                match res {
                    Ok(msg) => {
                        // 把 Input 事件转发到 inject worker
                        if let Message::Input(ev) = &msg {
                            if enable_inject {
                                if itx.send(*ev).is_err() {
                                    log::error!("client: inject channel closed");
                                }
                            }
                        }
                        handle(msg).await?;
                    }
                    Err(e) => {
                        log::error!("recv error: {:?}", e);
                        break;
                    }
                }
            }
            // 心跳
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

/// 桥接：std::sync::mpsc (capture 线程) → tokio::mpsc (async 任务)。
fn bridge_capture_to_tokio(
    std_rx: std::sync::mpsc::Receiver<InputEvent>,
    tokio_tx: mpsc::UnboundedSender<InputEvent>,
) {
    while let Ok(ev) = std_rx.recv() {
        if tokio_tx.send(ev).is_err() {
            log::info!("capture bridge: tokio receiver dropped, stopping");
            return;
        }
    }
    log::info!("capture bridge: std channel closed, stopping");
}

/// 注入 worker：从 tokio mpsc 拉事件并调用平台 inject。
fn bridge_tokio_to_inject(mut tokio_rx: mpsc::UnboundedReceiver<InputEvent>) {
    while let Some(ev) = tokio_rx.blocking_recv() {
        if let Err(e) = input::inject::inject_event(ev) {
            log::warn!("inject failed: {:?}", e);
        }
    }
    log::info!("inject worker: channel closed, stopping");
}

async fn handle(msg: Message) -> Result<()> {
    match msg {
        Message::Heartbeat { t } => {
            log::debug!("heartbeat: ignoring send-back (server side)");
            let _ = t;
        }
        Message::HeartbeatAck { t } => {
            log::info!("heartbeat ack, rtt = {} ms", now_ms().saturating_sub(t));
        }
        Message::Hello { name, platform } => {
            log::info!("peer hello: {} ({})", name, platform);
        }
        Message::Input(_) => {
            // 在 session_client 已被分发到 inject 队列；这里只 log
            log::trace!("handle: input event dispatched");
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
