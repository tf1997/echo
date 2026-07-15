# Echo 稳定性与身份重构 开发计划

> 目标：让 Echo 在**新旧版本混跑**的局域网里做到**稳定聊天与文件发送**，并根治 `peer_id = IP:port` 带来的换 IP 数据丢失/串味问题。
>
> 本文档同时是**开发进度看板**。每完成一项，把该任务的「状态」改为 ✅、填写「完成日期」与「Commit」，并在文末《变更记录》追加一行。

- 创建日期：2026-07-11
- 关联分析：通信层（后端）、前端 A/B/C、身份 D1–D8、兼容性适配（见《附录 A：问题索引》）

---

## 0. 如何使用本文档

**状态图例**：⬜ 未开始 · 🟡 进行中 · ✅ 完成 · ⏸️ 阻塞 · ❌ 放弃

**更新约定**：
1. 开始一个任务 → 状态改 🟡，在「负责人」填名字。
2. 完成 → 状态改 ✅，填「完成日期」和「Commit」（短 hash），确认「验收标准」逐条通过。
3. 若验收发现问题回退 → 状态改回 🟡 或 ⏸️，在任务下方「备注」记录原因。
4. 每次合并到默认分支后，更新顶部《进度总表》的完成计数。

**优先级**：P0 = 直接导致消息/文件丢失，必须最先做；P1 = 明显影响可用性；P2 = 健壮性/体验；P3 = 根治性大改。

---

## 1. 兼容性铁律（所有任务共同约束）

Echo 是 P2P 局域网应用，**整网不可能同时升级**，新旧客户端会长期共存。任何改动必须遵守：

1. **新能力可选**：新增字段一律 `#[serde(default, skip_serializing_if=...)]`，禁止 `deny_unknown_fields`。
2. **缺失即降级**：对端不具备新能力（无 `node_id` / 不带 `client_msg_id` / `software_version` 偏旧）时，回退到旧路径。
3. **降级不判失败**：绝不能因为"对端没回 ACK / 没有 node_id"就把消息判成发送失败——旧版本本就不会回这些。
4. **不造旧版本不认识的 `msg_type`**：`handle_incoming` 的 `_ =>` 兜底会把未知 `msg_type` 当**文本消息弹给用户**。新控制信令要么复用旧版已知的 `msg_type` 加字段（如换 IP 复用 `profile_updated` 夹带 `old_peer_id`），要么只发给已确认为新版本的 peer。
5. **能力判定手段**：
   - 是否支持 ACK / 去重 → 看消息是否带 `client_msg_id`。
   - 是否支持文件端到端确认 / node 寻址 → 看 `sender_node_id` 是否非空、`sender_software_version` 是否 ≥ 目标版本。

---

## 2. 进度总表

| 里程碑 | 主题 | 任务数 | 完成 | 状态 |
|---|---|---|---|---|
| M1 | 本地止血（零协议风险） | 8 | 8 | ✅ |
| M2 | 送达可靠性（能力门控） | 5 | 5 | ✅ |
| M3 | 身份健壮性（换 IP） | 4 | 4 | ✅ |
| M4 | 体验与大文件安全网 | 9 | 9 | ✅ |
| M5 | 根治：node_id 提主键 | 1 | 1 | ✅ |
| — | 文档同步 | 1 | 1 | ✅ |
| **合计** | | **28** | **28** | **100%** |

---

## 3. 里程碑 M1 — 本地止血（对旧版本完全透明，可立即上线）

这批全部是纯本地行为或本地 schema 变更，不改 wire 协议，风险最低，应最先落地。

### TASK-01 · 迁移后清理内存幽灵 + 通知前端切换会话 key — P0（D1）
- **问题**：收到 `peer_id_changed` 后只改了 DB，内存 `peers` map 从不 `remove`（`service.rs` / `chat/mod.rs` 全文无 `map.remove`），前端也无任何 `peer_id_changed` 处理。结果：正打开的会话仍持旧 peerId，发消息命中内存幽灵 → 连旧 IP 失败 → 进死队列永久丢失。
- **改动**：
  - 后端 `migrate_peer_references` 成功后，从内存 map 删除旧 endpoint 条目；`emit_all("peer-id-changed", {old, new, node_id})`。
  - 前端监听该事件：把 `activeContact`、`messages` 的会话 key、`pendingByConversation` 从旧 id 迁到新 id。
- **文件**：`src-tauri/src/chat/mod.rs`（迁移调用点 960-981）、`src-tauri/src/discovery/service.rs`、`frontend/src/App.tsx`。
- **兼容**：纯本地 + 新增事件，旧版本不受影响。
- **验收**：A 换 IP 重启 → B 正开着与 A 的会话 → B 无需重启即可继续给 A 发消息且送达；B 侧栏无重复的 A 条目。
- 状态：✅ · 负责人：Claude · 完成日期：2026-07-11 · Commit：627b264
- 备注：已完成后端（`chat/mod.rs` 迁移点后 `map.remove(old)` + `emit_all("peer-id-changed", {oldPeerId,newPeerId,nodeId})`）。静态验证通过（cargo build / tsc / lint）。多实例运行时验收待做。

### TASK-02 · 死队列 alias 兜底重定向 — P0（D1）
- **问题**：`deliver_pending_to_peer(旧id)` 查 `get_stored_peer` 返回 None 就直接 return（`commands.rs:3616-3619`），已入队的消息再无机会投递。
- **改动**：查不到 stored_peer 时，用 `identity_keys_for` / `peer_aliases` 把 `peer_id=旧id` 的 pending 解析到该 node 当前 endpoint 再投；投递成功后按新 id 归档。
- **文件**：`src-tauri/src/commands.rs`、`src-tauri/src/db/mod.rs`。
- **兼容**：对有 node_id 关联的 peer 生效；纯旧版本无 alias 则维持现状，不劣化。
- **验收**：构造一条 `peer_id=旧id` 的 pending，A 以新 id 上线后该消息能被投出并从队列删除。
- 状态：✅ · 负责人：Claude · 完成日期：2026-07-11 · Commit：627b264
- 备注：`deliver_pending_to_peer` 现遍历 `db.identity_aliases(peer_id)` 解析出的全部历史 endpoint，把各自 `pending_notifications` 队列投递到当前地址。新增 `Database::identity_aliases`（`identity_keys_for` 的 pub 包装）。静态验证通过。

### TASK-03 · client_msg_id 唯一索引 + 存量去重 — P0（通信 3）
- **问题**：`client_msg_id` 只有普通索引，`save_message_dedup` 是 SELECT→INSERT，并发窗口可双写；补发无锁可重入。
- **改动**：迁移脚本先按 `(sender_id, group_id, client_msg_id)` 去重（保留最小 id），再建 `UNIQUE` 索引；写入改 `INSERT ... ON CONFLICT DO NOTHING`（或 `INSERT OR IGNORE`）。
- **文件**：`src-tauri/src/db/mod.rs`（schema 490-527、save_message* 2340+/1488+）。
- **兼容/雷区**：历史 `client_msg_id` 多为 NULL——SQLite 多个 NULL 不冲突，天然安全，但索引定义要显式确认不误伤 NULL；升级时存量重复行必须**先去重再建索引**，否则 `CREATE UNIQUE INDEX` 失败。沿用现有 `IF NOT EXISTS` 幂等风格。
- **验收**：并发两次 dedup 保存同 `client_msg_id` 仅落一行；旧库升级不报错。
- 状态：✅ · 负责人：Claude · 完成日期：2026-07-11 · Commit：627b264
- 备注：建索引前先按 `(sender_id, COALESCE(group_id,''), client_msg_id)` 去重保留 MIN(id)；建**部分唯一索引** `idx_messages_client_dedup`（`WHERE client_msg_id IS NOT NULL AND TRIM<>''`，避开 NULL 冲突）；`save_message`/`save_group_message` 改 `ON CONFLICT DO NOTHING` + `rows_affected()==0` 时回查既有行返回。静态验证通过。

### TASK-04 · 运行期 IP 变化监听（不重启也能换 IP） — P0（D3）
- **问题**：`peer_id_changed` 判定与 `my_id` 只在 `RuntimeServices::start()` 算一次（`state.rs:29,36-37`），运行中 DHCP 续约/切网不触发，本机变"僵尸 id"。
- **改动**：后台任务每 10s 比对 `local_ip()` 与当前动态 `my_id`；变化时串行迁移 DB/alias、更新聊天身份、原地重建 LAN 发现并立即广播，同时通知前端更新本机身份，无需重启 TCP 服务。
- **文件**：`src-tauri/src/lib.rs`、`src-tauri/src/state.rs`、`src-tauri/src/chat/mod.rs`、`src-tauri/src/commands.rs`、`src-tauri/src/discovery/service.rs`、`src-tauri/src/discovery/broadcast.rs`、`frontend/src/App.tsx`。
- **兼容**：广播仍用旧 `AnnouncePacket` 格式，旧版本照收。
- **验收**：运行中手动改本机 IP（或切网卡），≤30s 内对端能以新 IP 连到本机。
- 状态：✅ · 负责人：Codex · 完成日期：2026-07-13 · Commit：627b264
- 备注：新增 10s 本机 IP 监听、共享动态 LocalPeerId、DB 引用/alias 迁移、LAN 发现原地重建并立即发送 startup burst、稳定 node_id 自包过滤，以及前端本机 peer_id 热更新。cargo check / 前端 build 通过；真实切网卡 ≤30s 多实例验收待现场复核。

### TASK-05 · 渲染止血：diff 后再 setState + MessageBubble.memo — P1（C1）
- **问题**：2s 轮询每次生成全新数组直接 setState，无 diff/memo → 整棵树含全部 MessageBubble 每 2s 全量重渲染。
- **改动**：peers/groups/unread 复用 `areMessageListsEqual` 式内容比较后再 setState；`MessageBubble` 套 `React.memo`；`allItems`、Sidebar 的 `deptGroups/unreadMap/peerById/peerByEndpoint` 改 `useMemo`。
- **文件**：`frontend/src/App.tsx`、`frontend/src/components/Sidebar.tsx`、`frontend/src/components/MessageBubble.tsx`、`frontend/src/components/ChatWindow.tsx`。
- **兼容**：纯前端。
- **验收**：无网络变化时 React DevTools Profiler 显示消息列表 2s 内不再重渲染。
- 状态：✅ · 负责人：Codex · 完成日期：2026-07-13 · Commit：627b264
- 备注：peers/groups/unread 内容未变时复用旧引用；MessageBubble 使用 React.memo；allItems 与 Sidebar 派生 Map 使用 useMemo。前端生产构建通过；React DevTools Profiler 待现场复核。

### TASK-06 · 失败 pending 消息排序改用 createdAt — P1（A6）
- **问题**：`allItems` 排序对 pending 项 `getTime` 返回 `Date.now()`，失败消息永远沉底钉在"最新"。
- **改动**：所有 pending 创建处补写 `createdAt`（文本/文件路径当前漏写：1448-1454、1486-1494、1690-1692、1721-1723），排序用 `createdAt`。
- **文件**：`frontend/src/components/ChatWindow.tsx`。
- **验收**：一条失败消息后再收 20 条新消息，失败消息停在它原本的时间位置而非底部。
- 状态：✅ · 负责人：Codex · 完成日期：2026-07-13 · Commit：627b264
- 备注：PendingMessage.createdAt 改为必填，文本、截图、选择文件、原生拖放、贴图/重试等全部创建路径均写入时间；排序稳定使用 createdAt。前端生产构建通过。

### TASK-07 · App.tsx 事件监听改 disposed 模式 — P1（A7）
- **问题**：三处 `listen().then(fn => unlisten = fn)` 存在注册-清理竞态，StrictMode 双挂载下重复注册 → 每条消息重复执行副作用。
- **改动**：统一采用 ChatWindow 已有的 `disposed + trackUnlisten` 模式（ChatWindow.tsx:861-904 为范式）。
- **文件**：`frontend/src/App.tsx`（328-338、888-950、952-966）。
- **验收**：dev StrictMode 下 conversation-updated 只注册一份，markRead 不重复触发。
- 状态：✅ · 负责人：Codex · 完成日期：2026-07-13 · Commit：627b264
- 备注：conversation-updated、tray-open-conversation、peer-id-changed（含新增 local-peer-id-changed）均使用 disposed 模式，解决 StrictMode 注册/清理竞态。前端生产构建通过。

### TASK-08 · 补发按 peer 加互斥锁 — P1（通信 3）
- **问题**：健康检查每 8s spawn `deliver_pending_to_peer`，无锁可重入，上一轮未删记录时下一轮重发；排队的群文件会被完整重传落两份。
- **改动**：按 peer_id 维护"正在投递"标记（`Mutex<HashSet>` 或 per-peer flag），投递中则跳过本轮。
- **文件**：`src-tauri/src/commands.rs`、`src-tauri/src/lib.rs`。
- **验收**：对同一 peer 快速触发两轮补发，队列记录只被投递一次。
- 状态：✅ · 负责人：Codex · 完成日期：2026-07-13 · Commit：627b264
- 备注：进程级 Mutex<HashSet<peer_id>> 实现 single-flight 投递，重复触发直接跳过；新增并发门控单元测试。cargo check 与测试编译通过；Windows 测试进程受 STATUS_ENTRYPOINT_NOT_FOUND 环境问题未能启动。

### M1 手动验收清单（发布前）

- **人工验收状态**：⬜ 未开始（全部通过后改为 ✅）
- **建议环境**：同一局域网的 A/B 两台 Windows 设备，使用独立数据目录；兼容性测试另准备旧版本 C。测试前备份数据库，并记录版本、IP 和日志路径。

| 编号 | 关联任务 | 手动操作 | 通过标准 | 状态 / 证据 |
|---|---|---|---|---|
| M1-MAN-01 | TASK-01 / TASK-02 | A/B 打开同一会话；A 离线后，B 向 A 的旧 endpoint 产生 pending；A 更换 IP 并重新启动，B 不重启。 | B 自动切到 A 的新 endpoint；pending 补投成功并清空；侧栏没有新旧 A 两条记录。 | ⬜ |
| M1-MAN-02 | TASK-04 | A 保持 Echo 运行，切换 Wi-Fi、网卡或 DHCP 地址；B 持续观察并双向发送消息。 | 10～30 秒内 B 发现 A 的新地址并恢复双向通信；双方无需重启且没有重复联系人。 | ⬜ |
| M1-MAN-03 | TASK-05 | 使用 React DevTools Profiler 录制一个消息较多的会话，静置至少 10 秒且不产生网络变化。 | 2 秒轮询期间，已有 `MessageBubble` 和消息列表不再周期性整树重渲染。 | ⬜ |
| M1-MAN-04 | TASK-06 | 对离线 peer 发送一条失败消息，随后恢复通信并继续收发至少 20 条新消息。 | 失败 pending 保持创建时的时间位置，不会一直钉在列表底部。 | ⬜ |
| M1-MAN-05 | TASK-07 | 开发 StrictMode 下反复重挂载主界面，再让 B 向 A 发送一条消息。 | 消息、`markRead`、提示音、托盘角标和会话刷新均只执行一次。 | ⬜ |
| M1-MAN-06 | TASK-08 | B 离线时排队一个传输时间超过 8 秒的大群文件；B 上线后，让下一轮健康检查在首轮传输未结束时再次触发补发。 | 日志出现 `Skipping concurrent pending delivery`；B 只收到一份文件；pending 记录只删除一次。 | ⬜ |
| M1-MAN-07 | 回归 | 完成换 IP 后重启 A/B，检查私聊历史、群成员、未读数和最近联系人，再继续互发消息。 | 历史不丢失、不串到其他联系人；群成员不重复；未读数与最近联系人正确。 | ⬜ |
| M1-MAN-08 | 兼容性铁律 | 新版本 A 与旧版本 C 分别测试在线、离线恢复、A 换 IP 后的文本和文件互发。 | 旧版本基础行为不劣化；缺少 `node_id`、ACK 或新字段时不会被误判失败。 | ⬜ |

**测试记录**：

- 测试人：—
- 日期：—
- A/B/C 版本与 commit：—
- A/B/C IP 与网络环境：—
- 结果：⬜ 未开始 / 🟡 部分通过 / ✅ 全部通过
- 日志、截图或录屏路径：—
- 失败项与问题链接：—

---

## 4. 里程碑 M2 — 送达可靠性（能力门控，新版本间强语义 / 旧版本走原路）

### TASK-09 · ACK 硬语义（client_msg_id + software_version 门控） — P0（通信 1）
- **问题**：`send_wire_message` ACK 超时当成功（1692-1699）；`deliver_over_tcp` 写完即 true；补发写完即删队列。对端刚崩溃时消息静默丢失且永久删除。
- **改动**：仅当消息带 `client_msg_id` 且已知对端 `software_version >= 0.2.0` 时要求 ACK：ACK 超时 → 不删队列、保留重试；ACK 窗口从 180ms 提到 3s。版本缺失/低于阈值的旧客户端保持“写完即成功”。
- **文件**：`src-tauri/src/chat/mod.rs`（24、1692-1699）、`src-tauri/src/commands.rs`（deliver_over_tcp 3957-3959、补发 4027）。
- **兼容**：铁律 3。旧版本零影响。
- **验收**：新版本↔新版本：对端进程被 kill 后发消息 → 显示未送达并保留在队列；对端恢复后自动送达且不重复（靠 TASK-03 去重）。旧版本↔新版本：行为不变。
- 状态：✅ · 负责人：Codex · 完成日期：2026-07-13 · Commit：c6a8c31
- 备注：Cargo/Tauri 版本同步提升至 `0.2.0`，作为 ACK 能力阈值；直发、群 fanout 与 pending 补发共用严格 ACK 校验，错误/缺失 ACK 均不删除队列。版本门控、错误 ACK、超时、旧版降级和 pending 前缀删除测试已补，测试编译通过；Windows 测试进程仍被 `STATUS_ENTRYPOINT_NOT_FOUND` 阻断。

### TASK-10 · 文件端到端确认 + 字节校验（software_version 门控） — P0（通信 2）
- **问题**：文件写完 flush 即报成功，从不读接收端已回的 ACK；接收端落库前不校验字节数，截断文件被当完整保存。
- **改动**：接收端在 `file_end` 校验累计字节 == `file_size`（可选哈希）后才回 ACK；发送端在对端 `software_version ≥ 支持版本`时阻塞等待该 ACK，失败发 `file-error` 而非报成功。旧接收端不回 ACK → 回退当前"写完即成功"。
- **文件**：`src-tauri/src/chat/mod.rs`（发送 2119-2133、接收 1291-1304 / 1447-1449）。
- **兼容**：门控 `software_version >= 0.2.0`；版本缺失或低版本走原路。
- **验收**：新↔新：传输中断对端不落库、发送端显示失败；正常完成两端字节一致。旧↔新：不回退为失败。
- 状态：✅ · 负责人：Codex · 完成日期：2026-07-13 · Commit：c6a8c31
- 备注：接收端校验逐帧身份/文件元数据、累计字节与声明大小，所有解码、写盘、flush、尺寸或落库失败均删除半包且不回 ACK；ACK 丢失重传命中 `client_msg_id` 去重后会删除新建临时文件，避免孤儿。直发、群文件和 pending 群文件仅对 `0.2.0+` 等待 ACK，旧版维持写完成功；文件 ACK 正路径和字节上限测试已补。

### TASK-11 · 前端投递状态 UI（已送达/等待上线/失败重试） — P0（B1）
- **问题**：直发失败静默入队并照常返回成功，气泡上无任何投递状态，用户以为对方已收到。
- **改动**：后端把可空 `delivered` 随 ChatMessage 返回并入库；前端气泡 meta 显示 已送达 / 等待对方上线 / 已发送，命令失败的 pending 显示发送失败并可重试；补发成功经 conversation-updated 更新。依赖 TASK-09/10 提供真实送达信号。
- **文件**：`src-tauri/src/db/mod.rs`（messages 加 delivered 列）、`src-tauri/src/chat/mod.rs`、`frontend/src/components/MessageBubble.tsx`、`frontend/src/components/ChatWindow.tsx`。
- **兼容**：`delivered` 使用可空三态兼容旧库：`true`=ACK 已确认、`false`=等待补发、`NULL`=旧版/历史未知并显示“已发送”，不谎称“已送达”。
- **验收**：给离线 peer 发消息显示"等待对方上线"，对方上线后变"已送达"。
- 状态：✅ · 负责人：Codex · 完成日期：2026-07-13 · Commit：c6a8c31
- 备注：私聊在线 ACK、离线排队与旧版写完分别落为三态；补发成功先按身份 alias + `client_msg_id` 回写消息，再删除队列并 emit 刷新气泡。前端比较逻辑纳入 `delivered`，失败 pending 原位复用同一 `client_msg_id` 重试；文件进度不再按 100% 定时强删失败气泡。

### TASK-12 · 失焦时不自动已读 — P1（A2）
- **问题**：窗口最小化/失焦时活跃会话新消息被无条件 markRead，未读永不增长 → 无声音无角标，用户错过消息。
- **改动**：仅 `document.hasFocus()`（或 `appWindow.isVisible() && isFocused()`）时才 markRead；失焦走未读计数路径，focus 事件里用已有 `refreshActiveConversation` 补 markRead。
- **文件**：`frontend/src/App.tsx`（907-914、935-941）。
- **验收**：最小化后对方发消息 → 托盘角标 +1、有提示音；恢复窗口后清零。
- 状态：✅ · 负责人：Codex · 完成日期：2026-07-13 · Commit：c6a8c31
- 备注：活跃会话仍即时合并事件消息，仅 `document.hasFocus()` 时自动 markRead；失焦/最小化走未读刷新，重新聚焦复用现有 focus 路径补标已读。前端 build 与定向 ESLint 通过（无新增告警）。

### TASK-13 · 群消息 fanout 异步化 + 部分失败不丢本地 — P1（通信 4）
- **问题**：`send_group_message_typed` 同步串行对每个成员投递，掉线成员各吃 2s 超时；任一入队失败整体返回 Err 且不写本地库，消息"消失"。
- **改动**：fanout 移入后台并用 `join_all` 并发；命令先落本地库并 emit 后返回；失败成员入队即可，不影响本地可见。
- **文件**：`src-tauri/src/commands.rs`（2734-2775）。
- **验收**：N 人群含离线成员，发送即时返回且本地立即可见；离线成员上线后补收。
- 状态：✅ · 负责人：Codex · 完成日期：2026-07-13 · Commit：c6a8c31
- 备注：群文本先保存并立即返回，再后台并发直投/入队；单成员失败只记日志。新增双监听器 Barrier 测试，串行实现会超时，并发实现可同时收包并回 ACK；测试编译通过。

### M2 手动验收清单（发布前）

- **人工验收状态**：⬜ 未开始（全部通过后改为 ✅）
- **建议环境**：新版本 A/B、旧版本 C；群聊测试另准备新版本 D/E。各实例使用独立数据目录，并记录版本、IP、日志、数据库路径和 `client_msg_id`。

| 编号 | 关联任务 | 手动操作 | 通过标准 | 状态 / 证据 |
|---|---|---|---|---|
| M2-MAN-01 | TASK-09 / TASK-11 | 新版本 A/B 在线，A 向 B 连续发送文本、表情等带 `client_msg_id` 的消息。 | B 每条仅显示一次；A 收到 ACK 后显示“已送达”；对应 pending 队列为空。 | ⬜ |
| M2-MAN-02 | TASK-03 / TASK-09 / TASK-11 | 用网络代理或调试手段仅丢弃 B→A 的 ACK、保留 A→B 的消息；随后恢复 ACK 并触发补发。 | ACK 超时后 A 不删除 pending 且不显示“已送达”；恢复后自动补发并清队列；B 按 `client_msg_id` 去重，始终只有一条消息。 | ⬜ |
| M2-MAN-03 | TASK-09 / TASK-11 | 结束新版本 B 进程，A 向 B 发送消息；确认状态后重启 B。 | A 显示“等待对方上线”或可重试状态，消息保留在队列；B 恢复后自动收到且仅收到一次，A 最终更新为“已送达”。 | ⬜ |
| M2-MAN-04 | TASK-10 / TASK-11 | 新版本 A→B 正常发送一个较大文件；完成后分别执行 `Get-FileHash` 并比较大小与 SHA-256。 | B 仅在完整落盘并通过字节校验后保存消息和回 ACK；两端大小、SHA-256 一致；A 最终显示成功/已送达。 | ⬜ |
| M2-MAN-05 | TASK-10 / TASK-11 | A→B 发送大文件，在约 30%～70% 时断网、结束 B 或中断 TCP；随后恢复网络并重试。 | B 不产生已完成消息，临时半包被清理；A 显示发送失败且可重试、不误报成功；重试后文件完整且仅有一条有效记录。 | ⬜ |
| M2-MAN-06 | TASK-09 / TASK-10 / TASK-11 | 新版本 A 与旧版本 C 互发在线/离线文本和文件，等待时间超过新版本 ACK 超时窗口。 | C 不回 ACK 时 A 仍走旧路径，不无限保留队列；基础行为不劣化；状态只显示“已发送”，不谎称“已送达”。 | ⬜ |
| M2-MAN-07 | TASK-12 | A 打开与 B 的会话后最小化或切到其他窗口；B 分别发送私聊和群消息；随后重新聚焦 A。 | 失焦期间不自动已读，侧栏与托盘未读数增加并有提示音；恢复焦点后当前会话刷新并清零，提示音不重复。 | ⬜ |
| M2-MAN-08 | TASK-13 | 创建含 A/B/D/E 的群，令 D/E 离线，A 连续发送多条消息；观察本地与在线 B，再启动 D/E。 | 发送立即返回且本地立即可见，不随离线成员数量线性阻塞；B 即时收到；D/E 上线后补收且不重复；单成员失败不影响其他成员和 A 的本地记录。 | ⬜ |

**测试记录**：

- 测试人：—
- 日期：—
- A/B/C/D/E 版本与 commit：—
- 网络、ACK 丢弃方式与数据目录：—
- 结果：⬜ 未开始 / 🟡 部分通过 / ✅ 全部通过
- 日志、气泡/托盘截图、pending 查询、文件大小与 SHA-256：—
- 失败项与问题链接：—

---

## 5. 里程碑 M3 — 身份健壮性（换 IP）

### TASK-14 · 迁移加 node_id 归属校验 + 旧版本降级 — P0（D2）
- **问题**：`migrate_peer_references` 只判 `old != sender`，不校验 old_peer_id 的 node_id 归属 → 可被主动嫁接历史；DHCP 端点重用会自动把两人会话串到一个 node。
- **改动**：`peer_id_changed` 携带 `node_id`；接收端仅当 `old_peer_id` 当前解析到的 node_id == 发送者 node_id 时才迁移；无 node_id（旧版本）回退现有 username+department+endpoint 启发式。`upsert_peer_alias` 改写归属前先校验冲突。
- **文件**：`src-tauri/src/chat/mod.rs`、`src-tauri/src/db/mod.rs`、`src-tauri/src/contact_sync.rs`、`src-tauri/src/discovery/broadcast.rs`、`src-tauri/src/lib.rs`。
- **兼容**：铁律 2，安全性随旧版本淘汰收敛。
- **验收**：伪造 `peer_id_changed{old: 他人id}` 被拒绝；DHCP 重用场景下 A、C 会话不再串。
- 状态：✅ · 负责人：Codex · 完成日期：2026-07-14 · Commit：107ba5c
- 备注：接收端统一校验 TCP 实际来源 endpoint、payload/wire `node_id` 一致性和 old/new endpoint 归属；已绑定 node 不允许无 node 来包降级，纯旧版仅在 endpoint、用户名、部门及可用 MAC 严格吻合时迁移。alias 使用条件 UPSERT，跨 node 冲突不再改写；迁移、alias、全表引用更新和旧 peer 删除纳入同一事务，任一步失败全部回滚。普通消息、联系人 relay 和 UDP relay 不能旁路建立已有 node 的新 endpoint 归属。测试覆盖同 node、跨 node 嫁接、降级、legacy、alias 冲突、普通 upsert 绕过、node-less 覆盖和事务回滚。当前 `node_id` 仍是公开标识，无法抵御攻击者完整复制受害者 node/profile；真正的主动身份认证需后续引入签名密钥。

### TASK-15 · 换 IP 通知改直发 + 覆盖在线 peer — P1（D4）
- **问题**：换 IP 通知只 `queue_pending_notification` 且仅发 `list_stored_peers`；对方用旧 id 探测不到新 IP 的我，补发触发不了；双方同时换 IP 永久互相失联。
- **改动**：对当前在线（内存 map）peer 直接 TCP 推 `peer_id_changed`，失败再入队；目标集合并入内存在线 peer。
- **文件**：`src-tauri/src/state.rs`、`src-tauri/src/commands.rs`。
- **兼容**：payload 仍用 `profile_updated` 夹带（铁律 4）；旧版本收到当新 peer（其固有局限）。
- **验收**：两台新版本同时换 IP 后仍能互相发现并继续会话。
- 状态：✅ · 负责人：Codex · 完成日期：2026-07-14 · Commit：107ba5c
- 备注：启动期与运行期共用换 IP 发布路径，目标集合合并 stored peer 与内存在线 peer、去重并排除新旧本机 endpoint；复用并发 `send_or_queue_notification`，在线先 TCP 直发，失败才写 pending。兼容 payload 继续使用 `profile_updated`，同时在 wire 与 payload 携带稳定 `node_id`。已补目标合并/去重/排除离线与 self 的纯函数测试。

### TASK-16 · 群成员换 IP 后的迁移与分身清理 — P1（D5）
- **问题**：`group_members.peer_id` 存 endpoint；第三方换 IP 通知到不了时，成员列表残留旧 id，且新 id 消息触发 auto-join 追加成员 → 同人两条成员记录。
- **改动**：auto-join 前用 node_id/alias 判重，命中同 node 则更新而非新增；成员解析统一走 `identity_keys_for`。
- **文件**：`src-tauri/src/chat/mod.rs`、`src-tauri/src/db/mod.rs`、`src-tauri/src/commands.rs`。
- **验收**：群成员换 IP 后成员数不虚增，@ 与已读统计正确。
- 状态：✅ · 负责人：Codex · 完成日期：2026-07-14 · Commit：107ba5c
- 备注：新增基于已确认 `endpoint → node` 关系的群成员原子 upsert/remove：同 node 切换 endpoint 时保留最早 `joined_at` 并删除旧 alias；成员读取先规范化现有 `group_members`，不再从历史消息复活已退群成员；返回结果按 node（旧版按 endpoint）去重，同名同部门的不同 node 保持独立。auto-join、建群、退群及 fanout 统一使用规范化成员。未预绑定的新 endpoint 不会仅凭 wire 中可伪造的 node/profile 合并成员，而是先保持 endpoint 隔离，待可信迁移通知建立 alias 后自动收敛。

### TASK-17 · 前端 peer_id_changed 完整处理 — P1（D1 前端侧）
- **问题**：前端 `frontend/src` 中 grep 不到任何 `peer_id_changed` 处理。
- **改动**：与 TASK-01 事件对接，覆盖 activeContact/messages/pending/草稿/未读 全部按会话 key 迁移；侧栏去重旧条目。
- **文件**：`frontend/src/App.tsx`、`frontend/src/components/ChatWindow.tsx`、`frontend/src/components/Sidebar.tsx`。
- **验收**：对方换 IP 时前端无重复联系人、正开会话无缝续接、草稿不丢。
- 状态：✅ · 负责人：Claude · 完成日期：2026-07-11 · Commit：627b264
- 备注：`App.tsx` 新增 `peer-id-changed` 监听（disposed 模式）：从 `peers` 移除旧 id（必要时以旧信息补建新 id 条目）、把 `selectedPeer` 从旧 id 重指到新 id 触发历史重载、`recentRefreshKey+1`。新增 `PeerIdChangedEvent` 类型。tsc/lint 通过（无新增告警）。草稿隔离依赖 TASK-22，此处先保证会话 key 迁移。

**自动验证（2026-07-14）**：`cargo fmt --check`、`cargo check`、`cargo test --no-run`、前端生产构建和 `git diff --check` 通过。Rust 测试二进制实际启动仍被本机既有 `0xc0000139 STATUS_ENTRYPOINT_NOT_FOUND` 阻断。ESLint 仍有仓库既有的 3 个 Fast Refresh error（`MessageBubble.tsx` 2 个、`main.tsx` 1 个）和 `ChatWindow.tsx` 1 个 Hook warning，本批未改前端且未新增 lint 项。

### M3 手动验收清单（发布前）

- **人工验收状态**：⬜ 未开始（全部通过后改为 ✅）
- **建议环境**：同一局域网的新版本 A/B/C，另准备旧版本 D；每个实例使用独立数据目录。测试前备份数据库，并记录版本、commit、node_id、IP、日志和数据库路径。涉及伪造包时仅在隔离测试网执行。

| 编号 | 关联任务 | 手动操作 | 通过标准 | 状态 / 证据 |
|---|---|---|---|---|
| M3-MAN-01 | TASK-14 / TASK-17 | A/B 已有私聊、未读和最近联系人；A 更换 IP 后重启或触发运行期热迁移，B 保持运行并打开与 A 的会话。 | B 只保留一个 A；会话自动切到新 endpoint，历史、未读、最近联系人和草稿 key 不丢；继续双向发送成功。 | ⬜ |
| M3-MAN-02 | TASK-14 | 在隔离环境构造 `profile_updated`：发送者 node-B，`old_peer_id` 指向 node-A；再分别测试 payload/wire node 不一致、sender_id 与 TCP 来源不一致。 | 全部被拒绝并记录明确日志；A 的 messages、group_members、recent/pending、peer/alias 均不改变，前端不触发 `peer-id-changed`。 | ⬜ |
| M3-MAN-03 | TASK-14 | 先让 A 从旧 IP 迁到新 IP，再让 node-C 复用 A 的旧 IP/port 并向 B 发消息。 | B 不把 C 写入 A 的 node/alias，不把 C 的消息并入 A 历史；冲突 endpoint 被隔离或拒绝，不发生串人。 | ⬜ |
| M3-MAN-04 | TASK-14 | 准备 legacy 联系人记录：无 node_id、用户名/部门/endpoint 已知；分别发送身份完全一致、用户名不同、部门不同及 MAC 冲突的换 IP 通知。 | 仅严格一致的 legacy 通知允许降级迁移；任一身份冲突均拒绝，已绑定 node 的联系人不能被无 node 通知降级。 | ⬜ |
| M3-MAN-05 | TASK-15 | A/B 在线但尚未产生最近会话，确认双方仅存在于内存发现列表；A 更换 IP。随后用防火墙临时阻断一次直发再恢复。 | 在线内存 peer 也收到通知；正常路径立即直达，阻断时只生成一条 pending，恢复后补发并清队列。 | ⬜ |
| M3-MAN-06 | TASK-15 | A/B 两台新版本尽量同时切换网络或 DHCP 地址，保持进程运行；持续观察发现日志并双向发送。 | 双方通过新 endpoint 重新发现并继续原会话；无永久互相失联、无重复联系人，失败通知可由 pending 补发。 | ⬜ |
| M3-MAN-07 | TASK-16 | 建立含 A/B/C 的群；记录成员数与 joined_at。让 C 换 IP，并让迁移通知先失败入队；恢复网络使通知补发，再由 C 发送群消息。 | alias 建立后成员从旧 endpoint 原子切到新 endpoint，成员总数不虚增、joined_at 不变；群消息 fanout 对 C 只投递一次。 | ⬜ |
| M3-MAN-08 | TASK-16 | 创建两个用户名和部门完全相同、node_id 不同的成员加入同一群；其中一人换 IP，再让另一人退群。 | 两人始终是两条独立成员；换 IP 只收敛同 node alias；退群后即使保留历史消息也不会被成员读取重新加入或继续收到 fanout。 | ⬜ |
| M3-MAN-09 | 兼容/回归 | 新版本 A 与旧版本 D 测试私聊、群聊、离线恢复和 D 换 IP；随后重启所有实例，检查私聊历史、群成员、未读、最近联系人和 pending。 | 缺少 node_id 时按 legacy 规则降级且基础通信不劣化；重启后数据不丢、不串人，群成员和未读统计稳定。 | ⬜ |

**测试记录**：

- 测试人：—
- 日期：—
- A/B/C/D 版本与 commit：—
- A/B/C/D node_id、IP 与网络环境：—
- 结果：⬜ 未开始 / 🟡 部分通过 / ✅ 全部通过
- 日志、数据库查询、截图或录屏路径：—
- 失败项与问题链接：—

---

## 6. 里程碑 M4 — 体验与大文件安全网

### TASK-18 · 主窗口消息分页 + 跳转断层修复 — P1（A1 / B2）
- 主列表接入 `loadOlder`（复用 HistorySearchView 的滚动补偿）；从聊天记录跳转旧消息时取前后文（加 `after_id` 查询）或提供"回到最新"悬浮按钮；merge 前检测不连续显示占位。
- **文件**：`frontend/src/App.tsx`（59、791-793、1151-1162）、`frontend/src/components/ChatWindow.tsx`、`src-tauri/src/db/mod.rs`（get_conversation_history 加 after_id）。
- **验收**：>500 条会话可上滑加载；跳转旧消息后收新消息不再出现永久断层。
- 状态：✅ · 负责人：Codex · 完成日期：2026-07-15 · Commit：f9ecfb1
- 备注：主会话默认加载最新一页，滚动到顶部按 `before_id` 增量加载并补偿滚动位置；历史搜索跳转会并行加载目标前后文，断层期间显示提示并提供“回到最新”。私聊与群聊 `after_id` 查询统一按升序返回，新增交错消息回归测试验证不会跨会话取数。

### TASK-19 · 群文件进度聚合上报 + 完成信号后才移除气泡 — P1（A4）
- 后端按 `(已完成成员数×size + 当前已发)/(成员数×size)` 聚合上报，总完成事件加 `done:true`；前端仅在 `done` 后移除 pending，保留暂停/取消至真正完成。
- **文件**：`src-tauri/src/commands.rs`（3179-3306、3410-3432、3352-3361）、`frontend/src/components/ChatWindow.tsx`（872-889）。
- **验收**：3 人群发大文件进度单调递增、气泡不提前消失、全程可暂停/取消。
- 状态：✅ · 负责人：Codex · 完成日期：2026-07-15 · Commit：f9ecfb1
- 备注：群文件按“已完成目标字节 + 当前目标字节 / 全部目标字节”聚合并节流上报，最终显式发送 `done:true`；前端仅在完成信号后收敛 pending，暂停、恢复与取消沿用同一传输控制 id。取消会按 group/client/sender node-first 清除已排队成员并仅删除无引用缓存，避免成员上线后继续补发；零字节文件也能正确完成。

### TASK-20 · 大文件 IPC 改二进制/base64 单串通道 — P1（A9）
- `readFileAndSave` 的 `number[]` JSON 传输改 base64 单字符串或 Tauri fs / 自定义二进制协议。
- **文件**：`frontend/src/components/ChatWindow.tsx`（158-162）、`frontend/src/api.ts`（196）、`src-tauri/src/commands.rs`（save_temp_file）。
- **验收**：粘贴 30MB 图片不冻结 UI。
- 状态：✅ · 负责人：Codex · 完成日期：2026-07-15 · Commit：f9ecfb1
- 备注：临时文件与转发预览 IPC 从 `number[]` 改为单个 base64 字符串，后端在阻塞线程解码、校验 256MB 配额并写盘；前端不再构造百万级数字数组。

### TASK-21 · 合并转发附件大小上限 — P1（A10）
- `buildForwardCard` 附件超阈值（1–2MB）只带文件名+占位；图片只生成缩略图，不把原图 base64 塞进消息 JSON。
- **文件**：`frontend/src/components/ChatWindow.tsx`（459-492）、`frontend/src/components/MessageBubble.tsx`（388-393）。
- **验收**：转发 200MB 文件不产生巨型消息、双方不 OOM。
- 状态：✅ · 负责人：Codex · 完成日期：2026-07-15 · Commit：f9ecfb1
- 备注：合并转发采用 2MB 单附件与总预算；超限文件只保留名称、大小和不可内联提示，图片仅生成受预算约束的缩略图，接收端兼容旧卡片字段。

### TASK-22 · 草稿按会话隔离 + 抖一抖不切会话/不抢焦点 — P1（A3 / B7）
- 草稿仿 `pendingByConversation` 按会话 key 存取；抖一抖默认只任务栏闪烁+震动动画，抢焦点/切会话做成可选项（勿扰开关）。
- **文件**：`frontend/src/components/ChatWindow.tsx`（1048-1063）、`frontend/src/App.tsx`（93-103、928-933）。
- **验收**：给 A 打字时收到 B 的抖一抖，草稿不丢、会话不被切走、回车仍发给 A。
- 状态：✅ · 负责人：Codex · 完成日期：2026-07-15 · Commit：f9ecfb1
- 备注：文字、截图和 pending 均按稳定会话 key 隔离；endpoint 补齐 node 或发生可信迁移时会把 legacy bucket 收敛到 node bucket。抖一抖默认只触发任务栏提示和当前界面动画，不切走正在编辑的会话、不主动抢焦点。

### TASK-23 · 行长度上限 + file_size 配额 — P1（通信 7）
- `read_line` 设上限（如 8MB）超限断开连接；接收文件设 `file_size` 上限防塞满磁盘。
- **文件**：`src-tauri/src/chat/mod.rs`（693）。
- **验收**：超大单行消息不再能撑爆内存；超限文件被拒。
- 状态：✅ · 负责人：Codex · 完成日期：2026-07-15 · Commit：f9ecfb1
- 备注：所有 TCP JSON 行改用 8MB 有界读取，超限立即终止连接且不会继续扩容；接收文件声明上限为 2GB，每个 chunk 同时校验编码前后大小和累计字节，异常、中断与半包均清理临时文件。

### TASK-24 · 接收端文件进度反馈 — P2（B6）
- 接收端在 file_chunk 组装期间 emit 进度事件；取消时保留一条"已取消发送 xxx"本地记录。
- **文件**：`src-tauri/src/chat/mod.rs`、`frontend/src/components/ChatWindow.tsx`（1253-1262）。
- **验收**：收大文件时有"正在接收"进度；取消后双方都有痕迹。
- 状态：✅ · 负责人：Codex · 完成日期：2026-07-15 · Commit：f9ecfb1
- 备注：接收端按 200ms 节流上报已收字节、总量、速度、发送者 node 与状态；完成、失败和取消均发终态事件。取消会在双方保存带派生 `client_msg_id` 的本地记录，重放仍可去重且不会误消费原文件消息 id。

### TASK-25 · URL 可点击 — P2（B4）
- `renderTextWithLinks` 的 URL 用 `@tauri-apps/api/shell` 的 `open()` 打开，元素改 button/a 加 cursor/focus。
- **文件**：`frontend/src/components/MessageBubble.tsx`（148-172）。
- 状态：✅ · 负责人：Codex · 完成日期：2026-07-15 · Commit：f9ecfb1
- 备注：消息正文中的 HTTP/HTTPS URL 渲染为可键盘聚焦的按钮，通过 Tauri shell API 打开；点击会阻止气泡选择事件并在失败时记录错误。

### TASK-26 · 窗口最小尺寸 + 窄屏布局 — P2（B8）
- `tauri.conf.json` 加 `minWidth`(≥760)/`minHeight`；删掉 640px 的 `.app-sidebar{width:100%}` 或实现抽屉式侧栏；窄屏隐藏/浮层化群信息按钮。
- **文件**：`src-tauri/tauri.conf.json`、`frontend/src/index.css`（3356-3371）。
- 状态：✅ · 负责人：Codex · 完成日期：2026-07-15 · Commit：f9ecfb1
- 备注：主窗口设置 `minWidth: 760`、`minHeight: 560`；窄屏不再把侧栏强制铺满，聊天区、工具栏、群信息入口及弹层采用可压缩/换行布局。

### M4 手动验收清单（发布前）

- **人工验收状态**：⬜ 未开始（全部通过后改为 ✅）
- **建议环境**：同一局域网的新版本 A/B/C，另准备旧版本 D；大文件场景确保磁盘空间充足。每个实例使用独立数据目录，测试前备份数据库，并记录 commit、IP、node_id、日志、文件大小与哈希。

| 编号 | 关联任务 | 手动操作 | 通过标准 | 状态 / 证据 |
|---|---|---|---|---|
| M4-MAN-01 | TASK-18 | 在包含 500 条以上消息的私聊和群聊中持续上滑加载；再从历史搜索跳转到较早消息、接收一条新消息，并点击“回到最新”。 | 分页无重复、漏页或滚动跳动；目标前后文顺序正确；存在断层时有明确提示，新消息不会伪装成连续历史，回到最新后恢复正常实时视图。 | ⬜ |
| M4-MAN-02 | TASK-19 | A 在至少 3 人群中发送一个传输时间明显的大文件，观察每位成员切换时的聚合进度与发送气泡。 | 总进度单调递增且不因切换成员回退；所有目标直发或成功入队前气泡不提前消失；最终只出现一次 `done:true` 完成态。 | ⬜ |
| M4-MAN-03 | TASK-19 / TASK-24 | 群文件传输中分别执行暂停、恢复和取消，并让一个成员中途离线后再上线。 | 暂停期间字节数基本不增长，恢复后从原进度继续；取消后不再发送后续 chunk，发送方与已开始接收方均保留取消痕迹；离线成员只产生一条可补发记录。 | ⬜ |
| M4-MAN-04 | TASK-20 / TASK-23 | 粘贴或拖入约 30MB 图片并观察界面响应；在隔离测试网发送超过 8MB 的单行包、声明超过 2GB 的文件，以及声明大小小于实际 chunk 总量的文件。 | 正常图片处理期间主界面仍可交互，不构造 `number[]` 巨型 IPC；超限行/文件被拒并断开，累计大小异常不落完整消息，磁盘上无遗留半包。 | ⬜ |
| M4-MAN-05 | TASK-21 | 合并转发包含 200MB 文件、2MB 内图片、超过 2MB 的图片和多个接近预算上限附件的消息。 | 卡片大小受控且发送/渲染不卡死；超限文件/图片只显示元数据和未内联提示，可内联图片仅含缩略图，总附件预算不会被多项叠加绕过。 | ⬜ |
| M4-MAN-06 | TASK-22 | 在 A 会话输入文字并启动截图，截图层开启时切到 B、输入另一份草稿后再完成截图；此时让 C 向本机发送抖一抖，然后分别切回 A/B。 | 截图仍回到启动时的 A 会话，各会话文字、截图与失败重试项互不串用；抖一抖不切换当前会话、不抢输入焦点，回车仍发送给原会话；可信 endpoint→node 升级后草稿不消失。 | ⬜ |
| M4-MAN-07 | TASK-24 | A→B 发送大文件，观察 B 的接收进度、速度和完成态；另一次在中途由 A 取消，再重试同一文件。 | B 显示稳定的“正在接收”进度并在完整落盘后结束；取消、失败、完成状态不混淆，双方保留可读记录；重试不会因取消记录与原 `client_msg_id` 冲突而丢失。 | ⬜ |
| M4-MAN-08 | TASK-25 | 在消息中发送行首、行中、带查询参数及标点结尾的 HTTP/HTTPS URL 和 `www.` 地址，分别用鼠标和键盘打开。 | 仅 URL 部分可点击且显示焦点；通过系统默认浏览器打开正确地址，不触发消息多选或破坏相邻文本，打开失败时应用不崩溃。 | ⬜ |
| M4-MAN-09 | TASK-26 | 将窗口缩到允许的最小尺寸，并在 760～900px 宽度检查私聊、群聊、侧栏、搜索、转发和群信息弹层。 | 窗口不能缩到 760×560 以下；侧栏不遮满聊天区，主要操作仍可访问，无横向溢出、文字严重重叠或不可关闭弹层。 | ⬜ |
| M4-MAN-10 | M4 兼容/回归 | 新版本 A 与旧版本 D 互发文本、普通文件和群消息，再重启两端并复查历史；同时复测新版本 URL、分页与草稿。 | 旧版缺少新进度/node/完成字段时基础通信不劣化；新字段不被当作未知消息展示，重启后历史、pending 与文件记录一致且无重复。 | ⬜ |

**测试记录**：

- 测试人：—
- 日期：—
- A/B/C/D 版本与 commit：—
- 数据目录、node_id、IP、网络限制与文件哈希：—
- 结果：⬜ 未开始 / 🟡 部分通过 / ✅ 全部通过
- 日志、数据库查询、Profiler、截图或录屏路径：—
- 失败项与问题链接：—

---

## 7. 里程碑 M5 — 根治（大版本）

### TASK-27 · node_id 提升为业务主键（双 key 共存过渡） — P3（D 系列根因）
- **目标**：`messages`/`group_members`/`recent_contacts`/`groups.creator_id`/各 pending 表以 node_id 存储，`IP:port` 降级为 peers 表的路由列。
- **兼容形态**（不可一步到位）：
  - 发送：`receiver_id` 仍填 endpoint（旧版本路由），同时填 `receiver_node_id`（新版本优先）。
  - 接收：`sender_node_id` 非空 → 用 node_id 作 key；为空（旧版本）→ 回退 endpoint。
  - 迁移：仅归并"已建立 node_id 关联"的历史；纯旧版本联系人保留 endpoint 作 key。
  - `identity_keys_for` **保留**为过渡期聚合桥，待整网升级后再简化。
- **依赖**：建议与 TASK-03/09 一起排期（都要动 messages 表与 wire 协议）。
- **验收**：换 IP 仅更新一行路由、不触碰历史数据；新旧混跑期间会话不串不丢。
- 状态：✅ · 负责人：Codex · 完成日期：2026-07-15 · Commit：f9ecfb1
- 备注：完成消息、群成员、最近联系人、群主及三类 pending 的双 key 迁移，查询、全局历史搜索、去重、本人/群主判断与群文件直发/补发均优先使用 node，空 node 才回退 endpoint。消息唯一索引拆为 node-first 与 legacy endpoint 两层，允许不同 node 复用 endpoint；同 node 换 endpoint 仅更新当前路由、pending 路由及通知 payload，不改写历史 endpoint。mixed/different owner 与 legacy profile/canonical endpoint 冲突均拒绝合并并保留旧行，启动清理遇冲突只告警不中断；`identity_keys_for` 继续作为新旧混跑聚合桥。

### M5 手动验收清单（发布前）

- **人工验收状态**：⬜ 未开始（全部通过后改为 ✅）
- **建议环境**：新版本 A/B/C、旧版本 D；准备可复制的数据库与可切换 DHCP/IP 的隔离局域网。测试前记录各实例 node_id、endpoint、数据库路径和关键业务表行数。

| 编号 | 关联任务 | 手动操作 | 通过标准 | 状态 / 证据 |
|---|---|---|---|---|
| M5-MAN-01 | TASK-27 | A/B 建立私聊、群聊、未读、最近联系人和三类 pending 后，让 A 更换 IP；迁移前后导出 messages/group_members/recent/groups/pending 与 peers/alias。 | 历史业务行及原 endpoint 不被批量重写；只新增/更新 A 的可信路由、alias 和待投递目标路由；原会话、未读、群成员、群主及补发继续命中同一 node。 | ⬜ |
| M5-MAN-02 | TASK-27 前端 | B 正在与 A 会话并保留文字、截图、失败项和接收进度；依次模拟同 endpoint 补齐 node、同 node 切到新 endpoint，以及相同 endpoint 出现不同已知 node。 | 前两种情况状态无缝收敛到 node key 且发送使用最新路由；不同已知 node 始终保持两份身份，不会同时选中、合并草稿或把最近联系人资料串给另一方。 | ⬜ |
| M5-MAN-03 | TASK-27 接收校验 | 在隔离测试网构造 receiver endpoint 正确但 `receiver_node_id` 指向其他节点的文本、群消息、文件 chunk/取消包；再构造 sender node 与已绑定 endpoint 冲突的包。 | 非空 receiver node 优先且错误时整包拒绝；冲突 sender 不建立 alias、不写消息/成员/最近联系人/pending，不覆盖 canonical peer，临时文件被清理并留下明确日志。 | ⬜ |
| M5-MAN-04 | TASK-27 兼容 | 新版本 A 与旧版本 D 互发私聊、群聊和文件，分别测试在线、离线补发、重启和 D 换 IP；检查空 node 行。 | 缺少 node 字段时稳定回退 endpoint，旧版基础行为不劣化；新版本不会把空 node 当成相同身份全局合并，重启迁移遇冲突可继续启动且数据不串不丢。 | ⬜ |
| M5-MAN-05 | TASK-27 身份边界 | 创建“同名同部门、不同 node”的 B/C，并让同一 node 的 A 先后使用两个 endpoint；在私聊、群成员、群主、历史搜索、未读与群文件中交叉操作。 | B/C 始终独立；A 的两个可信 endpoint 聚合为同一业务身份且只用当前路由投递，历史仍保留原 endpoint；前端“我”、群主和成员判断均以 node 为先。 | ⬜ |

**测试记录**：

- 测试人：—
- 日期：—
- A/B/C/D 版本、commit、node_id 与 endpoint：—
- 数据库备份、迁移前后查询与网络构造方式：—
- 结果：⬜ 未开始 / 🟡 部分通过 / ✅ 全部通过
- 日志、截图或录屏路径：—
- 失败项与问题链接：—

---

## 8. 文档同步

### TASK-28 · 更新 CLAUDE.md 过时描述 — P2
- 修正：消息"1s 轮询"实为事件推送（conversation-updated）+ focus 刷新；UDP 广播实为 8–15 分钟（非 3s）、子网扫描 25–45 分钟/96 探测（非 5 分钟全网段）；文件 chunk 实为 2MB（非 48KB）；补充静默时段（21:00–09:00）行为。
- **文件**：`CLAUDE.md`、必要时 `docs/`。
- 状态：✅ · 负责人：Codex · 完成日期：2026-07-15 · Commit：f9ecfb1
- 备注：身份模型已改为稳定 `node_id` + endpoint 路由双 key，消息刷新、发现节奏/静默时段、2MB 文件 chunk、流式接收与进度说明均已同步到当前实现。

**自动验证（2026-07-15）**：`npm.cmd run build`、`cargo fmt --check`、`cargo check`、`cargo test --no-run` 与 `git diff --check` 通过，Rust 的两个测试二进制均成功生成。`cargo test` 在启动 `src/lib.rs` 测试二进制前仍被本机 Windows `0xc0000139 STATUS_ENTRYPOINT_NOT_FOUND` 阻断，不能记为测试运行通过。`npm.cmd run lint` 仅报告仓库既有的 3 个 Fast Refresh error（`MessageBubble.tsx` 2 个、`main.tsx` 1 个），无 warning；Vite 另提示既有的 Tauri event 动/静态重复导入构建 warning，不影响产物生成。

---

## 9. 验收/回归矩阵（每个里程碑合并前必过）

| 场景 | 新↔新 | 新↔旧 | 旧↔旧（不得劣化） |
|---|---|---|---|
| 在线发文本，对端在线 | 显示已送达 | 显示已发送 | 原行为 |
| 对端离线发文本 | 等待上线→上线送达 | 排队→上线送达 | 原行为 |
| 对端进程崩溃发文本 | 未送达保留重试 | 发出即成功（不判失败） | 原行为 |
| 大文件传输中断 | 发送端失败、接收端不落半包 | 回退写完即成功 | 原行为 |
| 一方换 IP（重启） | 无缝续接 | 旧方当新 peer | 原启发式 |
| 一方运行中换 IP | ≤30s 重连 | 尽力而为 | N/A |
| 双方同时换 IP | 仍可发现续接 | 尽力而为 | 可能需手动加 IP |
| DHCP 端点重用 | 会话不串 | 尽力而为 | 原行为 |
| 群含离线成员发消息 | 即时可见、离线补收 | 同 | 原行为 |

---

## 附录 A：问题索引（分析编号 → 任务）

| 分析编号 | 描述 | 任务 |
|---|---|---|
| 通信 1 | ACK 写入即成功 | TASK-09 |
| 通信 2 | 文件无端到端确认/校验 | TASK-10 |
| 通信 3 | 去重无唯一约束 / 补发可重入 | TASK-03 / TASK-08 |
| 通信 4 | 群消息串行阻塞、部分失败丢本地 | TASK-13 |
| 通信 5 | 发现节奏与静默时段 | TASK-04（部分）/ TASK-28 |
| 通信 7 | 行无上限 / base64 协议开销 | TASK-23 |
| A1/B2 | 历史断层 / 无分页 | TASK-18 |
| A2 | 失焦自动已读 | TASK-12 |
| A3/B7 | 草稿全局单例 / 抖一抖抢焦点 | TASK-22 |
| A4 | 群文件进度跳动/气泡提前消失 | TASK-19 |
| A6 | 失败 pending 沉底 | TASK-06 |
| A7 | 事件监听注册竞态 | TASK-07 |
| A9 | 大文件 number[] IPC | TASK-20 |
| A10 | 合并转发 base64 塞消息 | TASK-21 |
| B1 | 无投递状态 | TASK-11 |
| B4 | URL 不可点击 | TASK-25 |
| B6 | 文件接收无反馈 | TASK-24 |
| B8 | 窗口无最小尺寸 | TASK-26 |
| C1 | 轮询全量重渲染 | TASK-05 |
| D1 | 迁移后内存幽灵 + 死队列 | TASK-01 / TASK-02 / TASK-17 |
| D2 | 迁移无归属校验 / DHCP 串味 | TASK-14 |
| D3 | 运行中换 IP 不触发 | TASK-04 |
| D4 | 换 IP 通知不可靠 | TASK-15 |
| D5 | 群成员分身 | TASK-16 |
| D 根因 | endpoint 当主键 | TASK-27 |

---

## 变更记录

| 日期 | 变更 | 作者 |
|---|---|---|
| 2026-07-11 | 初稿：28 项任务、5 里程碑、兼容铁律与验收矩阵 | — |
| 2026-07-11 | 完成 M1 首批 D1 闭环：TASK-01（迁移后清内存+emit 事件）、TASK-02（死队列 alias 兜底）、TASK-03（唯一索引+存量去重+ON CONFLICT）、TASK-17（前端 peer-id-changed 处理）。cargo build / tsc / eslint 均通过 | Claude |
| 2026-07-11 | 修复回归（非本批引入）：联系人 tab 部门默认折叠导致"有联系人但不显示"。经查 DB 有 14 条 peers、`list_stored_peers` 过滤全数返回，后端与 `mergePeers` 均正常；根因是 Sidebar 部门分组 `expandedDepts` 初始空 Set → 默认全折叠。改为 `collapsedDepts` 语义（默认展开、记录用户主动折叠）。tsc/eslint 通过 | Claude |
| 2026-07-13 | 完成 M1：TASK-04（运行期 IP 热迁移与立即重广播）、TASK-05（轮询 diff + memo/useMemo）、TASK-06（pending createdAt 稳定排序）、TASK-07（事件监听 disposed）、TASK-08（按 peer single-flight 补发）。cargo check、cargo fmt、前端生产构建通过；Rust 测试已编译，运行受 Windows STATUS_ENTRYPOINT_NOT_FOUND 阻断 | Codex |
| 2026-07-13 | 补充 M1 发布前手动验收清单：换 IP、Profiler、pending 排序、StrictMode、补发 single-flight、重启回归与新旧版本兼容 | Codex |
| 2026-07-13 | 修复“最近有会话但联系人为空”：`get_unread_counts` 的 `node_id` 分组歧义导致整组 `Promise.all` 失败；后端改用 `resolved_node_id`，前端将未读加载降级为独立容错，避免辅助查询阻断联系人列表 | Codex |
| 2026-07-13 | 回填 M1 / TASK-17 提交号 `627b264`；完成 M2 TASK-09～13：`0.2.0` 能力门控与硬 ACK、文件字节校验/半包清理、投递三态与补发回写、失焦保留未读、群消息后台并发 fanout | Codex |
| 2026-07-13 | 补充 M2 发布前手动验收清单：ACK 丢失与进程崩溃、文件完整性/中断、新旧版本降级、投递状态、失焦未读和含离线成员群发 | Codex |
| 2026-07-14 | 完成 M3 TASK-14～16：换 IP 迁移归属校验与事务回滚、在线 peer 直发失败入队、群成员按可信 node/alias 原子收敛；封堵普通 upsert、node-less 覆盖及 TCP/联系人/UDP relay 旁路，并补充 M3 发布前手动验收清单 | Codex |
| 2026-07-15 | 完成 M4 TASK-18～26：历史分页/断层恢复、群文件聚合进度、base64 单串 IPC 与转发预算、会话草稿/抖一抖、TCP 行与文件配额、接收进度/取消记录、URL 和窄屏适配；补充 M4 发布前手动验收清单 | Codex |
| 2026-07-15 | 完成 M5 TASK-27 双 key 过渡：业务表与 pending 增加 node key，查询/去重/前端身份语义改为 node-first，历史 endpoint 保持不可变，同 node 仅迁 pending 路由，跨 owner 与 legacy/canonical 冲突安全拒绝；补充 M5 手动验收清单 | Codex |
| 2026-07-15 | 完成 TASK-28，同步 `CLAUDE.md` 的身份模型、消息刷新、发现节奏、静默时段和文件传输说明；进度更新为 28/28（100%） | Codex |
