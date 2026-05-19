<div align="center">
  <img src="./frontend/src/assets/hero.png" alt="Echo Logo" width="128" height="128" />

  # Echo — P2P LAN Chat

  **Decentralized instant messaging for your local network. No server. No internet. Just talk.**

  <p>
    <a href="https://www.rust-lang.org/" target="_blank">
      <img src="https://img.shields.io/badge/rust-1.88+-dea584?logo=rust&logoColor=white" alt="Rust" />
    </a>
    <a href="https://tauri.app/" target="_blank">
      <img src="https://img.shields.io/badge/Tauri-2-ffc131?logo=tauri&logoColor=white" alt="Tauri 2" />
    </a>
    <a href="https://react.dev/" target="_blank">
      <img src="https://img.shields.io/badge/React-19-58c4dc?logo=react&logoColor=white" alt="React 19" />
    </a>
    <a href="https://tailwindcss.com/" target="_blank">
      <img src="https://img.shields.io/badge/Tailwind_CSS-4-06B6D4?logo=tailwindcss&logoColor=white" alt="Tailwind CSS 4" />
    </a>
    <br/>
    <img src="https://img.shields.io/badge/platform-macOS%20|%20Windows%20|%20Linux-lightgrey?logo=github" alt="Platform" />
    <a href="LICENSE">
      <img src="https://img.shields.io/badge/license-Apache%202.0-blue" alt="License" />
    </a>
    <img src="https://img.shields.io/badge/status-alpha-yellow" alt="Status" />
  </p>

  <h3>
    <a href="#features">Features</a>
    <span> · </span>
    <a href="#demo">Demo</a>
    <span> · </span>
    <a href="#quick-start">Quick Start</a>
    <span> · </span>
    <a href="#how-it-works">How It Works</a>
    <span> · </span>
    <a href="#build">Build</a>
  </h3>
</div>

---

## ✨ Why Echo?

> **Have you ever been in an office, school lab, or LAN party and needed to send a message or file to a colleague — but setting up a server or logging into Slack/WeChat felt like overkill?**

**Echo** is purpose-built for that moment. It discovers peers on your local network automatically via mDNS, connects directly over TCP, and requires **zero infrastructure** — no servers, no accounts, no internet connection.

✅ **100% offline** — works entirely on your LAN  
✅ **Zero configuration** — launch and instantly see who's online  
✅ **Privacy-first** — your data never touches the cloud  
✅ **Cross-platform** — macOS, Windows, Linux

---

## 🚀 Features

<table>
  <tr>
    <td align="center" width="50%">
      <h3>🔍 Auto-Discovery</h3>
      <p>mDNS service discovery finds peers on your LAN instantly — no IP guessing or manual setup.</p>
    </td>
    <td align="center" width="50%">
      <h3>💬 P2P Chat</h3>
      <p>Direct TCP connections for secure, low-latency messaging with send-failed retry.</p>
    </td>
  </tr>
  <tr>
    <td align="center">
      <h3>📎 File Sharing</h3>
      <p>Drag & drop, file picker, or paste images. Receive files with "Show in folder" and click-to-open.</p>
    </td>
    <td align="center">
      <h3>🖼️ Image Paste</h3>
      <p>Press <kbd>Ctrl+V</kbd> to paste screenshots and images directly into the chat — no saving needed.</p>
    </td>
  </tr>
  <tr>
    <td align="center">
      <h3>🟢 Online Status</h3>
      <p>Reliable health-check via TCP port probing every 8 seconds. See who's online at a glance.</p>
    </td>
    <td align="center">
      <h3>📜 Chat History</h3>
      <p>All messages persisted locally in SQLite. Full-text search across your conversations.</p>
    </td>
  </tr>
  <tr>
    <td align="center">
      <h3>🔔 Unread Badges</h3>
      <p>Unread message counts so you never miss a message, even with multiple conversations.</p>
    </td>
    <td align="center">
      <h3>📝 Profile Management</h3>
      <p>Editable username & department. Smart suggestions from saved data and online peers.</p>
    </td>
  </tr>
</table>

---

## 📸 Demo

<div align="center">
  <img src="./frontend/src/assets/hero.png" alt="Echo Screenshot" width="720" style="border-radius: 8px; box-shadow: 0 4px 20px rgba(0,0,0,0.15);" />
  <p><em>Echo in action — sidebar with online contacts, active chat window, and message input.</em></p>
</div>

---

## 🛠️ Tech Stack

| Layer | Technology |
|-------|-----------|
| 🖥️ Desktop Framework | [Tauri 2](https://tauri.app/) |
| 🎨 Frontend | [React 19](https://react.dev/) + [TypeScript](https://www.typescriptlang.org/) + [Tailwind CSS 4](https://tailwindcss.com/) + [Vite](https://vitejs.dev/) |
| ⚙️ Backend | [Rust](https://www.rust-lang.org/) (Tokio, SQLite, sqlx) |
| 🔎 Discovery | [mDNS](https://en.wikipedia.org/wiki/Multicast_DNS) via [mdns-sd](https://crates.io/crates/mdns-sd) |
| 🔗 Communication | TCP Direct (JSON-line protocol) |
| 🗄️ Storage | SQLite (local database) |

---

## ⚡ Quick Start

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) >= 1.88
- [Node.js](https://nodejs.org/) >= 18
- npm >= 9

### Install & Run

```bash
# 1. Clone the repository
git clone https://github.com/tf1997/echo.git
cd echo

# 2. Install frontend dependencies
cd frontend && npm install

# 3. Run the app (development mode)
cd ../src-tauri && cargo run
```

On first launch you'll be prompted to set a username and department — this is stored locally in SQLite and never leaves your machine.

### 🧪 Multi-instance Testing (Same Machine)

Want to see how Echo works on a single machine? Run two instances with different ports:

```bash
# Terminal A — Instance 1
ECHO_PORT=9527 ECHO_DATA_DIR=/tmp/echo-a cargo run

# Terminal B — Instance 2
ECHO_PORT=9528 ECHO_DATA_DIR=/tmp/echo-b cargo run
```

They'll discover each other instantly and you can test chat, file transfer, and more.

---

## 🔧 How It Works

```
┌─────────────┐     mDNS Discovery     ┌─────────────┐
│   Echo A    │◄──────────────────────►│   Echo B    │
│  (9527)     │                        │  (9528)     │
│             │◄── TCP Direct Chat ──►│             │
│  ┌───────┐  │                        │  ┌───────┐  │
│  │SQLite │  │                        │  │SQLite │  │
│  │ Local │  │                        │  │ Local │  │
│  └───────┘  │                        │  └───────┘  │
└─────────────┘                        └─────────────┘
```

1. **🚀 Startup** — Loads user profile from local SQLite; enters first-time setup if none exists
2. **🔎 Discovery** — Registers own mDNS service and continuously browses for other Echo instances on the LAN
3. **💾 Contact Storage** — Discovered peers are automatically saved to the local `peers` table (ip, port, online status, last seen time)
4. **❤️ Health Check** — Parallel TCP port probing every 8 seconds for reliable online detection
5. **💬 Chat** — TCP direct connection for JSON-line messages; all messages saved to local `messages` table
6. **📋 History** — Contact history and chat records are fully local; no central service dependency

---

## 🗄️ Database Schema

Three main tables (stored in `ECHO_DATA_DIR/echo.db` or the system app data directory):

| Table | Purpose |
|-------|---------|
| `user_profile` | Local user info (peer_id, username, department) |
| `peers` | Contact history (peer_id, username, department, ip, port, is_online, first_seen_at, last_seen_at) |
| `messages` | Chat records (sender_id, sender_name, receiver_id, content, msg_type, file_path, file_name, file_size, timestamp, is_read) |

---

## 🌍 Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `ECHO_PORT` | TCP listen port for chat | `9527` |
| `ECHO_DATA_DIR` | Data directory (SQLite database location) | System app data directory |

---

## 🏗️ Build

```bash
# Build frontend assets
cd frontend && npm run build

# Build the desktop app (release mode)
cd ../src-tauri && cargo build --release
```

The compiled binary will be available at `src-tauri/target/release/echo`.

---

## 📁 Project Structure

```
echo/
├── frontend/                # React frontend (TypeScript + Tailwind CSS)
│   └── src/
│       ├── App.tsx          # Main page: setup, contacts, chat
│       ├── api.ts           # Tauri invoke wrappers
│       ├── types.ts         # TypeScript type definitions
│       └── components/
│           ├── Sidebar.tsx      # Sidebar: profile, contacts, search
│           ├── ChatWindow.tsx   # Chat: messages, input, drag-drop, paste
│           └── MessageBubble.tsx # Messages: text, file, image preview
├── src-tauri/               # Rust backend (Tauri 2)
│   └── src/
│       ├── main.rs          # Entry point
│       ├── lib.rs           # Tauri bootstrap, state init, health-check loop
│       ├── commands.rs      # Tauri commands (IPC interface)
│       ├── state.rs         # Global state (RuntimeServices, AppState)
│       ├── chat/mod.rs      # TCP chat server
│       ├── db/mod.rs        # SQLite database (profile, peers, messages)
│       └── discovery/
│           ├── peer.rs      # Peer model
│           └── service.rs   # mDNS discovery service
└── target/                  # Rust build artifacts
```

---

## 🤝 Contributing

Contributions are welcome! Whether it's bug reports, feature suggestions, or pull requests — feel free to jump in.

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

---

## 📄 License

This project is licensed under the [Apache License 2.0](LICENSE).

---

<div align="center">
  <p>Made with ❤️ for local-first, offline-first communication</p>
  <p>
    <a href="https://github.com/tf1997/echo/issues">Report Bug</a>
    ·
    <a href="https://github.com/tf1997/echo/issues">Request Feature</a>
  </p>
</div>
