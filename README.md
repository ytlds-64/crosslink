# CrossLink — 跨平台键鼠共享（软件 KVM）

用一套鼠标 + 键盘无缝操控 **Windows** 与 **macOS**（Universal Control 式：光标越过屏幕边缘切到另一台，同一时刻只控制一台）。

> 当前进度：**M1 ✅ + M2.2 ✅**（M1 加密传输骨架；M2.2 Windows 单向键盘端到端：捕获→加密 wire→注入）。
> 鼠标 / 边缘切换 / macOS 端在后续里程碑（**M2.3**+）实现。

---

## 架构（M1）

```
┌──────────────┐    X25519 ECDH + AES-256-GCM (TLS 等价的安全通道)   ┌──────────────┐
│  Server 端    │  ←──── 加密消息流（Hello / Heartbeat / Input…） ──→  │  Client 端    │
│ (物理键鼠所在) │                                                   │ (被控的 Win/Mac)│
└──────────────┘                                                   └──────────────┘
```

- **握手**：服务端发送静态 X25519 公钥 → 客户端校验指纹（pinning）→ 客户端发送临时公钥 → 双方 ECDH 派生会话密钥（HKDF-SHA256）。
- **通道**：所有后续流量用 AES-256-GCM 认证加密，每帧携带单调递增计数器防重放。
- **信任模型**：服务端身份 = 静态公钥指纹（SHA-256，冒号十六进制）。首次连接打印指纹；客户端用 `--fingerprint` 锁定，未提供则 TOFU（信任首次）。

## 技术栈

- **Rust**（内存安全、单二进制、跨平台）
- 网络：`tokio`（异步 TCP）
- 加密（**纯 Rust，无需 C 编译器**）：`x25519-dalek` + `hkdf` + `aes-gcm` + `sha2`
- 序列化：`serde` + `bincode`
- 平台输入（仅 Windows，M2.2）：`windows` crate（`GetAsyncKeyState` + `SendInput`）
- CLI：`clap`；日志：`log` + `env_logger`

---

## 构建

### Windows

> 本工程**不**强制默认目标，因此 Windows 上必须显式指定 gnu 目标（宿主 msvc 缺导入库，链接会失败）。`.cargo/config.toml` 仅在 `--target x86_64-pc-windows-gnu` 时把 `gcc` 链接器指向绝对路径；`ar` / `dlltool` 仍需在 **PATH** 中（gnu 目标链接时调用）。加密库是**纯 Rust**，无需 C 编译器来编译依赖。

构建步骤：

1. 安装 MinGW-w64（提供 `gcc.exe` / `ar.exe` / `dlltool.exe`），例如 niXman mingw-builds 或 WinLibs，记下其 `bin` 目录。
2. 在**真正的 cmd / PowerShell** 中构建（用 Windows 反斜杠 PATH；不要在 MSYS / Git-Bash 里设 PATH，否则反斜杠会被改写导致找不到工具），并显式加 `--target`：

   ```powershell
   $env:PATH = "C:\path\to\mingw64\bin;" + $env:PATH
   cargo build --target x86_64-pc-windows-gnu --release
   ```

   > 在 Git-Bash 等 MSYS 环境下，可用 POSIX 风格路径（如 `/c/path/to/mingw64/bin`）加入 PATH，MSYS 会为 Windows 子进程转换成正确的 Windows 路径，再执行 `cargo build --target x86_64-pc-windows-gnu --release`。

产物位置（注意是 gnu 目标目录，不是 `target/release`）：

```
target\x86_64-pc-windows-gnu\debug\crosslink.exe      # 调试
target\x86_64-pc-windows-gnu\release\crosslink.exe    # 发布（--release）
```

### macOS

需要 Xcode 命令行工具提供链接器（`clang`/`ld`）：

```bash
xcode-select --install
cargo build --release
# 产物：target/release/crosslink
```

> 后续 M2 实现输入注入后，macOS 上需为应用授予「辅助功能 / 输入监控」权限（`CGEventTap` 要求），否则无法捕获/注入事件。

---

## 运行

### M1 联调（连通性 + 加密）

在一台机器上启动 **服务端**（持有物理键鼠），另一台（或同机另一终端）启动 **客户端**：

```bash
# 终端 A —— 服务端
cargo run -- --server
# 启动后会打印服务端身份指纹，例如：
#   AB:CD:EF:...:12

# 终端 B —— 客户端（把 <SERVER_IP> 换成服务端 IP；同机测试用 127.0.0.1）
cargo run -- --client <SERVER_IP> --fingerprint <上面打印的指纹>
```

> Windows 上请把上面的 `cargo run` 换成 `cargo run --target x86_64-pc-windows-gnu --`。macOS / Linux 直接 `cargo run` 即可（原生目标）。

连接成功后双方会：
1. 互发 `Hello`（携带节点名与平台）；
2. 每 3 秒互发 `Heartbeat`，收到后回 `HeartbeatAck` 并打印 RTT，证明加密通道端到端可用。

### M2.2 端到端（Windows 单向键盘）

服务端捕获本机键盘（或用 `--test-input` 注入 5 个 mock 键），客户端通过 `SendInput` 注入。

```bash
# 终端 A —— 服务端（启用键盘捕获）
cargo run --target x86_64-pc-windows-gnu -- --server --name srv

# 终端 B —— 客户端（启用 SendInput 注入）
cargo run --target x86_64-pc-windows-gnu -- --client 127.0.0.1 --fingerprint <FP> --name cli
```

**测试模式**（沙箱 / CI 友好，无真实按键也能跑通端到端）：

```bash
# 服务端：握手后 500ms 自动发 5 个 mock 键（a, b, 1, Tab, Enter）+ 5 个 release
cargo run --target x86_64-pc-windows-gnu -- --server --test-input
# 客户端正常连上后，日志应看到：
#   inject: hid=0x0004 -> VK 0x41 state=Pressed   (A)
#   inject: hid=0x0005 -> VK 0x42 state=Pressed   (B)
#   inject: hid=0x001E -> VK 0x31 state=Pressed   (1)
#   inject: hid=0x002B -> VK 0x09 state=Pressed   (Tab)
#   inject: hid=0x0028 -> VK 0x0D state=Pressed   (Enter)
#   ... 5 个 Released ...
```

**调试开关**：
- `--no-capture`（服务端）：不抓本地键盘，但保持加密通道在线
- `--no-inject`（客户端）：不调 `SendInput`，仅做协议转发验证

> 不传 `--fingerprint` 时客户端采用 TOFU（首次信任并打印警告），仅建议在受信任局域网内调试使用。

---

## 路线图

- **M1** ✅：传输骨架 — TCP 连通 + 加密握手 + 心跳
- **M2.2** ✅：Windows 单向键盘 — `GetAsyncKeyState` 捕获 + `SendInput` 注入 + HID 键码（**本机 Win 端到端验证通过**）
- **M2.3**：Windows 鼠标 — 鼠标移动 / 按键 / 滚轮
- **M2.4**：macOS 端到端 — `CGEventTap` / `CGEventPost`（需辅助功能权限）
- **M3**：边缘切换 — 屏幕几何 + 光标越界接管 + 反向回切
- **M4**：发现 / 配置 / GUI — mDNS、指纹授权、设置界面
- **M5**：增强 — 剪贴板共享、文件拖拽、加密加固、打包发布

---

## 已知限制

- M2.2 当前为**单向 Windows 键盘**（Server → Client），鼠标 / 边缘切换 / macOS 端尚未实现。
- GetAsyncKeyState 方案：foreground 切换时偶发滞后（约一个 5ms 轮询周期）。后续升级为 Raw Input（消息循环 + MakeCode 区分左右修饰键）以获得更好游戏兼容性。
- 加密采用 Noise 式 X25519+AES-GCM 自研握手（非完整 TLS），安全性依赖于指纹 pinning 的正确使用。
