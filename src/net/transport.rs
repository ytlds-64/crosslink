use anyhow::{anyhow, Context, Result};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::io::{split, AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};
use tokio::time::interval;

use crate::input;
use crate::input::event::{InputEvent, KeyEvent, KeyState, MouseButton, MouseEvent};
use crate::input::screen;
use crate::input::{CaptureMsg, CaptureOptions};
use crate::net::crypto::{self, Crypter, StaticSecret};
use crate::net::protocol::{CursorState, Message, Transfer};
use crate::switch::{Side, Switch};

/// 启动一个服务端会话：绑定端口、接受单连接、加密握手、进入会话循环。
///
/// `enable_capture` 为 true 时，服务端会启动键盘捕获线程，将本地按键通过
/// `Message::Input` 发送给对端（**单向：server → client**，M2 模式）。
///
/// `test_input` 为 true 时，启动 500ms 后会发送一组 mock 键事件用于端到端链路
/// 验证（无需真实按键——沙箱/CI 友好）。
///
/// `switch_mode` 为 true 时启用 M3 边缘切换：服务端初始持有指针，不再转发输入，
/// 仅在对端接缝边时交换 `Transfer` 消息。
///
/// `m4_mode` 为 true 时启用 M4 无缝单光标：Win 光标在 Win 右缘自动 warp 到左缘
/// 切到 Mac 区域（光标隐藏），通过 `Message::CursorState` 通知 Mac 显示。
pub async fn run_server(
    bind: &str,
    port: u16,
    key: &StaticSecret,
    name: &str,
    enable_capture: bool,
    test_input: bool,
    switch_mode: bool,
    side: Side,
    m4_mode: bool,
    m4_fallback: bool,
) -> Result<()> {
    let listener = TcpListener::bind((bind, port))
        .await
        .with_context(|| format!("bind {}:{}", bind, port))?;
    log::info!("server listening on {}:{}", bind, port);

    loop {
        let (stream, peer) = listener.accept().await?;
        log::info!("accepted connection from {}", peer);
        if let Err(e) = session_server(stream, key, name, enable_capture, test_input, switch_mode, side, m4_mode, m4_fallback).await {
            log::error!("session ended with error: {:?}", e);
        }
        log::info!("session closed, waiting for next connection");
    }
}

/// 启动一个客户端会话：连接、加密握手、进入会话循环。
///
/// `enable_inject` 为 true 时，收到 `Message::Input` 会通过 `SendInput`
/// 注入到本机键盘队列（M2 模式）。
///
/// `m4_mode` 为 true 时启用 M4 无缝单光标：收到 `Message::CursorState` 会
/// 通过 `[NSCursor hide/unhide]` 控制 Mac 光标显隐并 warp 到指定位置。
pub async fn run_client(
    addr: &str,
    port: u16,
    fingerprint: Option<&str>,
    name: &str,
    enable_inject: bool,
    switch_mode: bool,
    side: Side,
    m4_mode: bool,
) -> Result<()> {
    let stream = TcpStream::connect((addr, port))
        .await
        .with_context(|| format!("connect {}:{}", addr, port))?;
    log::info!("connected to {}:{}", addr, port);
    session_client(stream, fingerprint, name, enable_inject, switch_mode, side, m4_mode).await
}

async fn session_server(
    mut stream: TcpStream,
    key: &StaticSecret,
    name: &str,
    enable_capture: bool,
    test_input: bool,
    switch_mode: bool,
    side: Side,
    m4_mode: bool,
    m4_fallback: bool,
) -> Result<()> {
    let (crypter, _fp) = crypto::server_handshake(&mut stream, key).await?;
    log::info!("handshake complete");
    let (mut rd, wr) = split(stream);
    let wr = Arc::new(Mutex::new(wr));
    let crypter = Arc::new(Mutex::new(crypter));

    // ---- 屏幕几何（用于 Hello 交换；M3 边缘切换需要）----
    let (my_w, my_h) = screen::screen_size();

    // ---- 对端屏幕几何（M4 用于 y 坐标映射与光标位置 clamp）----
    let mac_w_atomic = Arc::new(AtomicU32::new(0));
    let mac_h_atomic = Arc::new(AtomicU32::new(0));

    // ---- 边缘切换控制器（仅 switch 模式）----
    let (sw_tx, mut sw_rx) = mpsc::unbounded_channel::<Transfer>();
    let switch: Option<Switch> = if switch_mode {
        let s = Switch::new(side, true, my_w, my_h, sw_tx);
        s.start_monitor();
        Some(s)
    } else {
        None
    };

    // ---- M2/M4 捕获桥（仅非 switch 模式）----
    let (etx, mut erx) = mpsc::unbounded_channel::<CaptureMsg>();
    if !switch_mode && enable_capture {
        let std_rx = input::capture::start_capture(CaptureOptions {
            win_w: my_w,
            win_h: my_h,
            mac_w: mac_w_atomic.clone(),
            mac_h: mac_h_atomic.clone(),
            m4_mode,
            m4_fallback,
        });
        let etx_c = etx.clone();
        tokio::task::spawn_blocking(move || bridge_capture_to_tokio(std_rx, etx_c));
        if m4_mode {
            if m4_fallback {
                log::info!(
                    "server: M4 seamless-cursor capture enabled (win={}x{}, FALLBACK GetCursorPos dx)",
                    my_w, my_h
                );
            } else {
                log::info!("server: M4 seamless-cursor capture enabled (win={}x{})", my_w, my_h);
            }
        } else {
            log::info!("server: keyboard+mouse capture → wire enabled");
        }
    } else if switch_mode {
        log::info!("server: --switch mode (edge switching), capture disabled");
    } else {
        log::info!("server: capture disabled (--no-capture)");
    }

    // ---- 测试模式（仅非 switch 模式）----
    if !switch_mode && test_input {
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
                let _ = etx_test.send(CaptureMsg::Input(InputEvent::Key(KeyEvent { hid: h, state: s })));
            }

            // 鼠标：一次相对移动 + 左键按下/释放
            log::info!("test-input: sending mock mouse events (move + left button)");
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = etx_test.send(CaptureMsg::Input(InputEvent::Mouse(MouseEvent {
                dx: 24,
                dy: 12,
                button: None,
                state: None,
            })));
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = etx_test.send(CaptureMsg::Input(InputEvent::Mouse(MouseEvent {
                dx: 0,
                dy: 0,
                button: Some(MouseButton::Left),
                state: Some(KeyState::Pressed),
            })));
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = etx_test.send(CaptureMsg::Input(InputEvent::Mouse(MouseEvent {
                dx: 0,
                dy: 0,
                button: Some(MouseButton::Left),
                state: Some(KeyState::Released),
            })));
            log::info!("test-input: sequence done");
        });
    }

    send_msg(
        &wr,
        &crypter,
        &Message::Hello {
            name: name.to_string(),
            platform: input::platform().to_string(),
            screen_w: my_w,
            screen_h: my_h,
        },
    )
    .await?;

    let mut hb = interval(Duration::from_secs(3));
    loop {
        tokio::select! {
            // 捕获事件 → 发送到对端（M2 / M4 模式）
            Some(msg) = erx.recv(), if switch.is_none() => {
                let wire_msg = match &msg {
                    CaptureMsg::Input(ev) => {
                        log::trace!("server: forwarding Input event: {:?}", ev);
                        Message::Input(*ev)
                    }
                    CaptureMsg::CursorState { on_mac, x, y } => {
                        log::trace!("server: forwarding CursorState: on_mac={} x={} y={}", on_mac, x, y);
                        Message::CursorState(CursorState { on_mac: *on_mac, x: *x, y: *y })
                    }
                };
                if let Err(e) = send_msg(&wr, &crypter, &wire_msg).await {
                    log::error!("send capture msg error: {:?}", e);
                    break;
                }
            }
            // 边缘切换：本端触发穿越 → 加密送 Transfer（M3 模式）
            Some(t) = sw_rx.recv(), if switch.is_some() => {
                if let Err(e) = send_msg(&wr, &crypter, &Message::Transfer(t)).await {
                    log::error!("send transfer error: {:?}", e);
                    break;
                }
            }
            // 接收对端消息
            res = recv_msg(&mut rd, &crypter) => {
                match res {
                    Ok(msg) => {
                        if !handle_msg(msg, &switch, &None, false, &mac_w_atomic, &mac_h_atomic, &wr, &crypter).await? {
                            break;
                        }
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

async fn session_client(
    mut stream: TcpStream,
    fingerprint: Option<&str>,
    name: &str,
    enable_inject: bool,
    switch_mode: bool,
    side: Side,
    _m4_mode: bool,
) -> Result<()> {
    let (crypter, fp) = crypto::client_handshake(&mut stream, fingerprint).await?;
    log::info!("handshake complete, server fingerprint = {}", fp);
    let (mut rd, wr) = split(stream);
    let wr = Arc::new(Mutex::new(wr));
    let crypter = Arc::new(Mutex::new(crypter));

    let (my_w, my_h) = screen::screen_size();

    // M4 对端几何（服务端在 Hello 中写入；客户端仅占位，保持签名统一）。
    let mac_w_atomic = Arc::new(AtomicU32::new(0));
    let mac_h_atomic = Arc::new(AtomicU32::new(0));

    let (sw_tx, mut sw_rx) = mpsc::unbounded_channel::<Transfer>();
    let switch: Option<Switch> = if switch_mode {
        // 客户端初始不持有指针（服务端先持有）。
        let s = Switch::new(side, false, my_w, my_h, sw_tx);
        s.start_monitor();
        Some(s)
    } else {
        None
    };

    // M2 注入 worker（仅非 switch 模式）。switch 模式下 itx 为 None，
    // 收到的 Input 既不会被注入也不会被转发（指针由本地物理输入接管）。
    let itx: Option<mpsc::UnboundedSender<InputEvent>> = if !switch_mode && enable_inject {
        let (tx, rx) = mpsc::unbounded_channel::<InputEvent>();
        tokio::task::spawn_blocking(move || bridge_tokio_to_inject(rx));
        log::info!("client: wire → SendInput inject enabled");
        Some(tx)
    } else {
        if switch_mode {
            log::info!("client: --switch mode (edge switching), inject disabled");
        } else {
            log::info!("client: inject disabled (--no-inject)");
        }
        None
    };

    send_msg(
        &wr,
        &crypter,
        &Message::Hello {
            name: name.to_string(),
            platform: input::platform().to_string(),
            screen_w: my_w,
            screen_h: my_h,
        },
    )
    .await?;

    let mut hb = interval(Duration::from_secs(3));
    loop {
        tokio::select! {
            // 边缘切换：本端触发穿越 → 加密送 Transfer（M3 模式）
            Some(t) = sw_rx.recv(), if switch.is_some() => {
                if let Err(e) = send_msg(&wr, &crypter, &Message::Transfer(t)).await {
                    log::error!("send transfer error: {:?}", e);
                    break;
                }
            }
            // 接收对端消息
            res = recv_msg(&mut rd, &crypter) => {
                match res {
                    Ok(msg) => {
                        if !handle_msg(msg, &switch, &itx, true, &mac_w_atomic, &mac_h_atomic, &wr, &crypter).await? {
                            break;
                        }
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

/// 处理一条收到的消息。返回 `false` 表示会话应结束。
///
/// `switch` 为 `Some` 时处于 M3 模式：收到 `Transfer` 即接管指针；收到 `Hello`
/// 时回填对端几何。M2 模式下 `Transfer` 会被忽略并告警。
///
/// `itx` 为 M2 客户端注入队列；非 M2（服务端 / switch 模式）传 `None`，
/// 此时收到 `Input` 不会注入。
async fn handle_msg(
    msg: Message,
    switch: &Option<Switch>,
    itx: &Option<mpsc::UnboundedSender<InputEvent>>,
    is_client: bool,
    mac_w: &Arc<AtomicU32>,
    mac_h: &Arc<AtomicU32>,
    wr: &Arc<Mutex<WriteHalf<TcpStream>>>,
    crypter: &Arc<Mutex<Crypter>>,
) -> Result<bool> {
    match msg {
        Message::Heartbeat { t } => {
            send_msg(wr, crypter, &Message::HeartbeatAck { t }).await?;
        }
        Message::HeartbeatAck { t } => {
            log::info!("heartbeat ack, rtt = {} ms", now_ms().saturating_sub(t));
        }
        Message::Hello {
            name,
            platform,
            screen_w,
            screen_h,
        } => {
            log::info!("peer hello: {} ({}) screen {}x{}", name, platform, screen_w, screen_h);
            if let Some(s) = switch {
                s.set_peer_geom(screen_w, screen_h);
            }
            // M4：服务端记录对端（Mac）几何，capture 线程每帧读取用于 y 映射与 clamp。
            mac_w.store(screen_w, Ordering::Relaxed);
            mac_h.store(screen_h, Ordering::Relaxed);
        }
        Message::Transfer(t) => {
            if let Some(s) = switch {
                s.on_receive(t.entry_x, t.entry_y);
            } else {
                log::warn!("received Transfer but --switch mode not enabled; ignoring");
            }
        }
        Message::Input(ev) => {
            log::trace!("recv Input: {:?} (is_client={})", ev, is_client);
            // M2 客户端：转发到本机注入 worker。服务端（itx=None）或不注入模式忽略。
            if let Some(itx) = itx {
                if itx.send(ev).is_err() {
                    log::error!("client: inject channel closed");
                }
            }
        }
        Message::CursorState(c) => {
            // M4：仅客户端（Mac）按 server 指令显隐并 warp 光标；服务端不会收到该消息。
            log::trace!(
                "recv CursorState: on_mac={} x={} y={} (is_client={})",
                c.on_mac,
                c.x,
                c.y,
                is_client
            );
            if is_client {
                if let Err(e) = input::inject::handle_cursor_state(c.on_mac, c.x, c.y) {
                    log::warn!("handle_cursor_state failed: {:?}", e);
                }
            } else {
                log::debug!("server received CursorState (ignored)");
            }
        }
    }
    Ok(true)
}

/// 桥接：std::sync::mpsc (capture 线程) → tokio::mpsc (async 任务)。
/// 泛型化以支持 `InputEvent` 与 M4 的 `CaptureMsg` 两种通道类型。
fn bridge_capture_to_tokio<T: Send + 'static>(
    std_rx: std::sync::mpsc::Receiver<T>,
    tokio_tx: mpsc::UnboundedSender<T>,
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
        log::trace!("bridge → inject_event: {:?}", ev);
        if let Err(e) = input::inject::inject_event(ev) {
            log::warn!("inject failed: {:?}", e);
        }
    }
    log::info!("inject worker: channel closed, stopping");
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
