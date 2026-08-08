# CrossLink — 跨平台键鼠共享（软件 KVM）

用一套鼠标 + 键盘无缝操控 **Windows** 与 **macOS**（Universal Control 式：光标越过屏幕边缘切到另一台，同一时刻只控制一台）。

> 当前进度：**M1 ✅ + M2.2 ✅ + M2.3 ✅ + M2.4 ✅ + M3 ✅**（M1 加密传输骨架；M2.2 Windows 单向键盘；M2.3 Windows 鼠标；M2.4 macOS 捕获/注入，**已在 macOS Tahoe 真机验证通过**；M3 边缘切换 / 指针漫游已实现）。
> 边缘切换（**M3**）已落地：双机指针漫游（Universal Control 式）。

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
- `--no-capture`（服务端）：不抓本地键盘/鼠标，但保持加密通道在线
- `--no-inject`（客户端）：不调 `SendInput`，仅做协议转发验证

> 不传 `--fingerprint` 时客户端采用 TOFU（首次信任并打印警告），仅建议在受信任局域网内调试使用。

### M2.3 端到端（Windows 键盘 + 鼠标）

`--test-input` 现同时发送键盘（a/b/1/Tab/Enter + release）**与鼠标**（一次相对移动 dx=24/dy=12 + 左键按下/释放）的 mock 事件，客户端日志应看到：

```
inject: hid=0x0004 -> VK 0x41 state=Pressed   (A)
...
inject: mouse move dx=24 dy=12
inject: mouse button Left Pressed
inject: mouse button Left Released
```

> 注：M2.3 仍**无条件转发**所有鼠标事件（屏幕边缘切换逻辑在 M3 才加入）。真实使用时请注意：服务端捕获的鼠标位移会被注入到客户端，因此在边缘切换落地前，仅建议在测试/受控环境下运行。

### M2.4 端到端（macOS）— 已真机验证通过 ✅

与 Windows 端对称：服务端（持有物理键鼠的 Mac）用 `CGEventTap` 捕获键盘/鼠标，客户端 Mac 用 `CGEventPost` 注入。

> ⚠️ **macOS Tahoe 关键坑**：裸 `cargo build --release` 产出的二进制在 Tahoe 上 **`CGEventTap` 拿不到稳定的「辅助功能」授权**（输入监控 OK，但辅助功能会失效 / 不持久）。
> **解决：必须打包成 `.app` bundle 并固定 `CFBundleIdentifier`**，TCC 才会稳定授权。一键打包脚本见 `tools/mac-bundle.sh`：
>
> ```bash
> zsh tools/mac-bundle.sh          # 在 Mac 上执行，产出 dist/crosslink.app
> ```
>
> 然后把 **`crosslink.app` 本体**（不是里面的二进制）加入：
> 系统设置 → 隐私与安全性 → **辅助功能** + **输入监控**，并勾选。

```bash
# 终端 A —— 服务端（Mac，持有物理键鼠；用 .app 内二进制）
dist/crosslink.app/Contents/MacOS/crosslink --server --name mac-srv

# 终端 B —— 客户端（另一台 Mac）
dist/crosslink.app/Contents/MacOS/crosslink --client <SERVER_IP> --name mac-cli
```

捕获端行为：键盘（含修饰键的 `FlagsChanged` 对比推断按下/释放）、鼠标按键与**相对位移**均经加密通道转发；注入端用 `CGEventPost`(HID) 重建事件，相对位移累积为本地绝对坐标。

> ⚠️ **禁止同机同时跑 server + client（标准 inject 模式）**：会产生输入回环风暴（捕获→注入→再捕获→无限放大），导致鼠标键盘失控。验证脚本 `tools/crosslink-verify.sh`（Mac）/ `tools/win-verify.ps1`（Windows）用 `--no-capture` / `--no-inject` 切断回环；**真正的端到端联动必须在两台不同机器上进行**。

非 Windows / 非 macOS 平台（如 CI 的 Linux 构建）提供 no-op 输入后端，`cargo build` 可正常通过，但运行时不做任何转发（冒烟测试仅验证连通性与心跳）。

> 🔐 **密钥安全**：服务端身份私钥 `crosslink-server.key` 已写入 `.gitignore` 且**绝不可提交 / 明文分享**（详见仓库根目录 `SECURITY.md`）。

---

## M3 端到端（边缘切换 / 指针漫游）— Universal Control 式 ✅

M3 让指针在双机之间「漫游」：任一时刻指针只属于一台机器（owner），owner 用本机物理键鼠**原生**操作本机；当 owner 把光标推到与对端共享的「接缝边」时，指针立即「交给」对端（对端在其接缝边对应位置落下光标并接管）。非 owner 一方光标停在接缝边、忽略本地输入。**稳态下不转发任何鼠标/键盘**——只交换极小的 `Transfer` 消息，彻底绕开跨平台「输入抑制」难题，也契合「同时只控一台」。

> ⚠️ 与 M2 的区别：M2 是「服务端单向转发键鼠到客户端」；M3 是「双机各自用各自物理键鼠，指针在屏幕间漫游」。两者互斥：`--switch` 开启 M3，否则走 M2 转发（**默认仍是 M2，向后兼容**）。

拓扑（默认水平拼接）：

```
┌─────────────┐   seam (右边缘)    ┌─────────────┐
│  Server 端   │ ─────────────────→│  Client 端   │
│ (初始 owner) │  指针在此越过边界   │ (初始待命)   │
└─────────────┘                    └─────────────┘
   --side right (默认)              --side left  (默认)
```

运行（两台**不同**机器，A 服务端 / B 客户端）：

```bash
# 机器 A（服务端，初始持有指针）—— Windows
cargo run --target x86_64-pc-windows-gnu -- --server --switch --name pc-a
# 机器 B（客户端，待命接指针）—— Windows
cargo run --target x86_64-pc-windows-gnu -- --client <A的IP> --switch --name pc-b

# macOS 端用 .app 内二进制（见 M2.4 的 .app 授权说明）
dist/crosslink.app/Contents/MacOS/crosslink --server --switch --name mac-a
dist/crosslink.app/Contents/MacOS/crosslink --client <A的IP> --switch --name mac-b
```

行为：
1. 启动后 A 持有指针，正常用 A 的键鼠；B 的光标停在屏幕接缝边（默认左边缘中点）。
2. 在 A 上把光标推到右边缘 → 指针「跳」到 B 的左边缘对应位置，B 变身 owner，用 B 的键鼠。
3. 在 B 上把光标推回左边缘 → 指针交还给 A。如此往复。
4. 两端分辨率不同时，y（或 x）按双方分辨率比例映射，落点不会跑偏。

布局微调：默认 server=右 / client=左（水平拼接）。若物理摆放是上下或左右相反，**两端用 `--side` 一致声明对端位置**即可（`right` / `left` / `top` / `bottom`）。例如 server 在左、client 在右：server 加 `--side left`，client 加 `--side right`。

> 真机双机联动需放行防火墙（服务端）：`netsh advfirewall firewall add rule name=crosslink dir=in action=allow protocol=TCP localport=4242`（端口按实际 `--port`）。
> macOS 端 M3 同样需要 `.app` + 「辅助功能 / 输入监控」授权（`set_cursor_pos` 用的 `CGWarpMouseCursorPosition` 也受 TCC 约束）。

### 单机自测（Windows，无需第二台机器）

`tools/win-verify.ps1` 新增 **Test 3**：本机 loopback 起 `--switch` 的服务端 + 客户端，验证双方监控线程启动、并交换屏幕几何（即 M3 协议接线正确）。真实「光标越界移交」仍需两台带显示器的机器手动验证。

```powershell
cd C:\Users\xiao\crosslink
powershell -ExecutionPolicy Bypass -File .\tools\win-verify.ps1
# 期望：injection: OK / capture: OK / switch: OK
```

---

## 路线图

- **M1** ✅：传输骨架 — TCP 连通 + 加密握手 + 心跳
- **M2.2** ✅：Windows 单向键盘 — `GetAsyncKeyState` 捕获 + `SendInput` 注入 + HID 键码（**本机 Win 端到端验证通过**）
- **M2.3** ✅：Windows 鼠标 — `GetCursorPos` 相对位移 + `GetAsyncKeyState` 按键 + `SendInput` 注入（**本机 Win 端到端验证通过**，含 `--test-input` 鼠标 mock）
- **M2.4** ✅：macOS 捕获/注入 — `CGEventTap`（捕获）+ `CGEventPost`（注入），修饰键用 FlagsChanged 对比推断；代码完成并通过两个 macOS 目标类型检查，**已在 macOS Tahoe 真机验证通过**（必须 `.app` bundle + TCC 双重授权，详见下方「macOS 端」与 `tools/mac-bundle.sh`）。
- **M3** ✅：边缘切换 / 指针漫游（Universal Control 式）— 屏幕几何交换 + 接缝边检测 + 指针跨机移交 + 反向回切（对称 ownership 模型，稳态不转发输入，仅交换极小 `Transfer` 消息）。`--switch` 开启，默认仍走 M2 转发（向后兼容）。
- **M4**：发现 / 配置 / GUI — mDNS、指纹授权、设置界面
- **M5**：增强 — 剪贴板共享、文件拖拽、加密加固、打包发布

---

## 已知限制

- M2.2/M2.3 当前为**单向 Windows 键鼠**（Server → Client）；M2.4 macOS 捕获/注入代码已完成（类型检查通过，已在 macOS Tahoe 真机验证通过），与 Windows 端链路对称。M3 边缘切换已实现：双机指针漫游（对称 ownership 模型，`--switch` 开启）。
- macOS 端需在「系统设置 → 隐私与安全性 → 辅助功能 / 输入监控」中授权本程序；未授权时 `CGEventTapCreate` 返回 NULL，程序仅 log 错误且不捕获/注入。
- GetAsyncKeyState 方案：foreground 切换时偶发滞后（约一个 5ms 轮询周期）。后续升级为 Raw Input（消息循环 + MakeCode 区分左右修饰键）以获得更好游戏兼容性。
- 加密采用 Noise 式 X25519+AES-GCM 自研握手（非完整 TLS），安全性依赖于指纹 pinning 的正确使用。
