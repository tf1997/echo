# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Run

```bash
# Install frontend dependencies (one-time)
cd frontend && npm install

# Run in development mode (single instance)
cd src-tauri && cargo run

# Build frontend assets
cd frontend && npm run build

# Build release binary
cd src-tauri && cargo build --release

# Lint frontend
cd frontend && npm run lint

# Multi-instance testing (same machine)
ECHO_PORT=9527 ECHO_DATA_DIR=/tmp/echo-a cargo run    # Terminal A
ECHO_PORT=9528 ECHO_DATA_DIR=/tmp/echo-b cargo run    # Terminal B
```

Environment variables:
- `ECHO_PORT` — TCP listen port (default `9527`)
- `ECHO_DATA_DIR` — SQLite database + log directory (default `~/.echo`)

## Architecture

Echo is a **P2P LAN chat** app built with **Tauri v1**. Rust backend, React 19 + TypeScript + Tailwind CSS 4 frontend. No servers, no internet — peers discover each other via UDP and communicate over direct TCP.

### Identity model

Each installation has a stable `node_id`; `peer_id` remains the current `IP:port` route for old-client compatibility. During the migration period both keys coexist: new records prefer confirmed node identity, while endpoint aliases keep legacy history readable. Never trust a wire `node_id` until it matches a persisted endpoint-to-node binding.

### Startup flow ([lib.rs](src-tauri/src/lib.rs))

1. Initialize file-based logging (5 MB rotation, `echo.log`)
2. Open SQLite database (`echo.db`)
3. Load user profile; if none exists, frontend shows first-time setup
4. `RuntimeServices::start()` launches two subsystems:
   - **DiscoveryService** — UDP broadcast/multicast/unicast peer discovery
   - **ChatServer** — TCP listener for incoming JSON-line messages
5. Spawn background loops: health check (8s), anti-entropy contact sync (5–8 min jittered), relay-to-DB processor

### Key Rust modules

| Module | Role |
|---|---|
| [lib.rs](src-tauri/src/lib.rs) | App bootstrap, background loop spawning, Tauri command registration |
| [state.rs](src-tauri/src/state.rs) | `AppState` (Tauri managed state) and `RuntimeServices` (bundle of discovery + chat) |
| [commands.rs](src-tauri/src/commands.rs) | All `#[tauri::command]` IPC handlers — the API surface between frontend and backend |
| [db/mod.rs](src-tauri/src/db/mod.rs) | SQLite via `sqlx` — tables, queries, migrations |
| [chat/mod.rs](src-tauri/src/chat/mod.rs) | TCP chat server — `WireMessage` protocol, file transfer, incoming message dispatch |
| [discovery/service.rs](src-tauri/src/discovery/service.rs) | Discovery orchestration (mDNS code exists but is **disabled**; uses UDP broadcast instead) |
| [discovery/broadcast.rs](src-tauri/src/discovery/broadcast.rs) | UDP broadcast + multicast + unicast subnet scan — the actual discovery mechanism |
| [discovery/peer.rs](src-tauri/src/discovery/peer.rs) | `Peer` and `PeerEntry` structs |
| [contact_sync.rs](src-tauri/src/contact_sync.rs) | Anti-entropy contact exchange: `contact_summary` → `contact_sync_res` protocol |

### Wire protocol

Messages are JSON lines over TCP (`\n`-delimited). The `WireMessage` struct ([chat/mod.rs:16-31](src-tauri/src/chat/mod.rs#L16-L31)) carries `msg_type`:

- `"text"` — chat message
- `"file_chunk"` / `"file_end"` — chunked base64 file transfer (2 MB raw per chunk)
- `"contact_summary"` / `"contact_sync_res"` — anti-entropy peer list exchange
- `"group_created"`, `"group_renamed"`, `"group_dissolved"`, `"group_member_left"` — group lifecycle
- `"profile_updated"` — identity change broadcast
- Every message includes `known_peers` (online peer list) for transitive discovery

### Discovery mechanism

mDNS code in [discovery/service.rs](src-tauri/src/discovery/service.rs) is gated behind `if false`. Actual discovery uses 3 strategies combined:

1. **UDP broadcast** startup burst (3 sends, 4–12s jitter), then every 8–15 minutes to `255.255.255.255:<chat_port+2>`
2. **UDP multicast** to `239.255.42.42:<chat_port+2>`
3. **Unicast subnet scan** every 25–45 minutes — probes at most 96 randomized hosts per cycle across configured `/24` subnets, with 80–250ms jitter

Discovery port = chat port + 2 (e.g., chat on 9527 → discovery on 9529).
Broadcast, multicast announcements, and subnet scans pause during the 21:00–09:00 quiet period. Listening remains active, and a fresh startup burst is sent after quiet hours.

### Health check & online detection

A background loop runs every 8 seconds ([lib.rs:168-256](src-tauri/src/lib.rs#L168-L256)). It concurrently TCP-connects to each known peer's chat port with a 2s timeout. A peer goes offline after 15s of failed probes. When a peer comes back online, pending messages are automatically delivered.

### Contact sync (anti-entropy)

Every 5–8 minutes (jittered), the app picks 2–3 online + 1 offline peer and exchanges full contact summaries. This propagates peer info across the network even when nodes join/leave at different times. The protocol:
- Initiator sends `contact_summary` (list of `{peer_id, username, department, ip, port, version}`)
- Receiver merges unknown peers, computes delta, responds with `contact_sync_res` (full summaries + missing_details)

### File transfer

Files are sent as chunked base64 over the existing TCP connection. `FILE_CHUNK_SIZE = 2 * 1024 * 1024` (2 MB raw per JSON line). The receiver decodes and streams chunks into a buffered file under `~/Echo/files/`, verifies the declared byte count, and removes incomplete files. Send and receive progress events are emitted to the frontend.

### Offline queuing

If a peer is unreachable, notifications (group messages, profile updates, group lifecycle events) are queued in the `pending_notifications` table. When the health check detects a peer coming back online, `deliver_pending_to_peer()` drains the queue.

### Frontend

Single-page React app with no router. Peer/group/unread summaries refresh defensively in the background, while message updates arrive through the `conversation-updated` event; window focus performs a reconciliation refresh. There is no 1-second message polling loop. Key components:

- [App.tsx](frontend/src/App.tsx) — top-level state, polling loops, profile setup
- [Sidebar.tsx](frontend/src/components/Sidebar.tsx) — peer list, groups, search, subnet config
- [ChatWindow.tsx](frontend/src/components/ChatWindow.tsx) — message display, input, drag-drop, paste
- [MessageBubble.tsx](frontend/src/components/MessageBubble.tsx) — individual message rendering
- [api.ts](frontend/src/api.ts) — thin wrappers around `tauri::invoke()`

### Database tables

SQLite at `$ECHO_DATA_DIR/echo.db`:

- `user_profile` — single row (id=1), local user info + scan_subnets config
- `peers` — all known peers (peer_id, username, department, ip, port, is_online, timestamps)
- `messages` — all chat messages (1:1 and group, with `group_id` column)
- `recent_contacts` — sidebar ordering
- `groups` + `group_members` — group chat memberships
- `pending_group_messages` — legacy offline queue (being replaced by `pending_notifications`)
- `pending_notifications` — generic offline delivery queue (payload is full WireMessage JSON)
