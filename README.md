# Echo — P2P LAN Chat

Decentralized LAN chat tool with mDNS auto-discovery and TCP direct connection. No central server required.

Supports Windows / macOS / Linux.

## Tech Stack

| Layer | Technology |
|---|---|
| Desktop Framework | Tauri 2 |
| Frontend | React 19 + TypeScript + Tailwind CSS 4 + Vite |
| Backend | Rust (Tokio, SQLite, sqlx) |
| Discovery | mDNS (mdns-sd) |
| Communication | TCP Direct (JSON-line protocol) |
| Storage | SQLite (local database) |

## Project Structure

```
echo/
├── frontend/                # React frontend
│   └── src/
│       ├── App.tsx          # Main page (first-time setup, contacts, chat)
│       ├── api.ts           # Tauri invoke wrappers
│       ├── types.ts         # TypeScript type definitions
│       └── components/
│           ├── Sidebar.tsx      # Sidebar (profile, contacts, search)
│           ├── ChatWindow.tsx   # Chat window (messages, input, file drag-drop, paste)
│           └── MessageBubble.tsx # Message bubble (text, file, image preview)
├── src-tauri/               # Rust backend
│   └── src/
│       ├── main.rs          # Entry point
│       ├── lib.rs           # Tauri bootstrap, state init, health-check loop
│       ├── commands.rs      # Tauri commands (IPC interface)
│       ├── state.rs         # Global state (RuntimeServices, AppState)
│       ├── chat/
│       │   └── mod.rs       # TCP chat server
│       ├── db/
│       │   └── mod.rs       # SQLite database (profile, peers, messages)
│       └── discovery/
│           ├── mod.rs       # Module exports
│           ├── peer.rs      # Peer model
│           └── service.rs   # mDNS discovery service
└── target/                  # Rust build artifacts
```

## Quick Start

### Requirements

- Rust >= 1.88
- Node.js >= 18
- npm >= 9

### Install & Run

```bash
# 1. Install frontend dependencies
cd frontend
npm install

# 2. Run
cd ../src-tauri
cargo run
```

On first launch you will be prompted to set a username and department, which is persisted to local SQLite.

### Multi-instance Testing (Same Machine)

```bash
# Terminal A (port 9527)
cd src-tauri
ECHO_PORT=9527 ECHO_DATA_DIR=/tmp/echo-a cargo run

# Terminal B (port 9528)
cd src-tauri
ECHO_PORT=9528 ECHO_DATA_DIR=/tmp/echo-b cargo run
```

Each instance uses a separate port and data directory. They can discover and chat with each other.

## Features

- LAN mDNS auto-discovery of online peers
- TCP direct P2P chat (no central server)
- First-time setup for username and department
- Editable personal profile with copy-to-clipboard
- Department suggestions from saved data and online peers
- Persistent contact history (online/offline state, last seen time)
- TCP port health-check for reliable online detection
- Text messaging with send-failed retry
- Image paste (Ctrl+V) and preview
- File drag-and-drop, file picker, and click-to-open
- "Show in folder" for received files
- Unread message badges
- Chat history search
- Files stored in `~/Echo/files/`

## How It Works

1. **Startup** — Loads user profile from local SQLite; enters first-time setup if none exists
2. **Discovery** — Registers own mDNS service and continuously browses for other Echo instances
3. **Contact Storage** — Discovered peers are automatically written to the local `peers` table (ip, port, online status, last seen time)
4. **Health Check** — Parallel TCP port probing every 8 seconds for reliable online detection
5. **Chat** — TCP direct connection for JSON-line messages; all messages saved to local `messages` table
6. **History** — Contact history and chat records are fully local; no central service dependency

## Database

Three main tables (stored in `ECHO_DATA_DIR/echo.db` or the system app data directory):

- `user_profile` — Local user info (peer_id, username, department)
- `peers` — Contact history (peer_id, username, department, ip, port, is_online, first_seen_at, last_seen_at)
- `messages` — Chat records (sender_id, sender_name, receiver_id, content, msg_type, file_path, file_name, file_size, timestamp, is_read)

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `ECHO_PORT` | Listen port | `9527` |
| `ECHO_DATA_DIR` | Data directory (contains SQLite) | System app data directory |

## Build

```bash
# Build frontend
cd frontend && npm run build

# Build backend
cd ../src-tauri && cargo build --release
```

## License

This project is licensed under the Apache License 2.0 - see the LICENSE file for details.
