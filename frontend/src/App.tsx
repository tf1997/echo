import { useState, useEffect, useCallback, useMemo, useRef } from "react";
import type { Peer, ChatMessage, AppInfo, StoredPeer, UnreadCount, UpdateCheckResult } from "./types";
import { isSameIdentity } from "./types";
import { ask, message, open } from "@tauri-apps/api/dialog";
import { listen } from "@tauri-apps/api/event";
import { appWindow, UserAttentionType } from "@tauri-apps/api/window";
import { Sidebar } from "./components/Sidebar";
import { ChatWindow } from "./components/ChatWindow";
import { AvatarPreviewTrigger } from "./components/AvatarPreview";
import { applyTheme, getInitialTheme } from "./theme";
import type { ThemeId } from "./theme";
import { MESSAGE_TYPE_NUDGE, MESSAGE_TYPE_RPS, NUDGE_COOLDOWN_MS, NUDGE_MESSAGE_CONTENT, getRpsMessageContent } from "./messageTypes";
import type { RpsMove } from "./messageTypes";
import {
  downloadUpdate,
  getAppInfo,
  getPeers,
  getConversation,
  getConversationHistory,
  deleteChatMessages,
  sendMessage,
  sendMessageTyped,
  sendFile,
  sendSticker,
  markRead,
  updateTrayUnread,
  checkPeerOnline,
  getDepartments,
  saveProfile,
  requestPeerAvatar,
  listStoredPeers,
  getUnreadCounts,
  getScanSubnets,
  setScanSubnets,
  getGroupMessages,
  getGroupHistory,
  sendGroupMessage,
  sendGroupMessageTyped,
  sendGroupFile,
  sendGroupSticker,
  listGroups,
  markGroupRead,
  restartAfterUpdate,
} from "./api";
import type { GroupInfo, TrayUnreadItem } from "./api";

function formatUpdatePrompt(result: UpdateCheckResult) {
  const version = result.latest_version || "";
  const notes = result.notes?.trim();
  if (!notes) {
    return `发现新版本 ${version}，是否现在下载？`;
  }
  return `发现新版本 ${version}，是否现在下载？\n\n更新说明：\n${notes}`;
}

function isEndpointLike(value: string) {
  return /^.+:\d+$/.test(value.trim());
}

const MESSAGE_FETCH_LIMIT = 100;
const HISTORY_CONTEXT_LIMIT = 50;

interface ConversationUpdatedEvent {
  kind: "contact" | "group";
  peer_id?: string | null;
  group_id?: string | null;
  message?: ChatMessage | null;
}

interface PeerIdChangedEvent {
  oldPeerId: string;
  newPeerId: string;
  nodeId?: string;
}

interface LocalPeerIdChangedEvent extends PeerIdChangedEvent {
  newIp: string;
}

interface HistorySearchRequest {
  kind: "contact" | "group";
  targetId: string;
  query: string;
  messageId?: number | null;
  nonce: number;
}

type ConversationKind = "contact" | "group";

interface ActiveConversation {
  kind: ConversationKind;
  id: string;
}

interface NudgeSignal {
  kind: ConversationKind;
  targetId: string;
  nonce: number;
}

const NUDGE_INTERRUPT_STORAGE_KEY = "echo.nudge.interrupt";

function getInitialNudgeInterruptEnabled(): boolean {
  try {
    return window.localStorage.getItem(NUDGE_INTERRUPT_STORAGE_KEY) === "true";
  } catch {
    return false;
  }
}

async function requestAttentionForNudge() {
  try {
    await appWindow.requestUserAttention(UserAttentionType.Informational);
  } catch (error) {
    console.error("Failed to request attention for nudge:", error);
  }
}

function isIncomingNudge(
  message: ChatMessage | null | undefined,
  myId: string | undefined,
  myNodeId: string | undefined,
): message is ChatMessage {
  return !!message
    && message.msg_type === MESSAGE_TYPE_NUDGE
    && !isSameIdentity(message.sender_node_id, message.sender_id, myNodeId, myId);
}

async function bringAppToFrontForNudge() {
  try {
    await appWindow.show();
    if (await appWindow.isMinimized().catch(() => false)) {
      await appWindow.unminimize();
    }
    await appWindow.setFocus();
  } catch (error) {
    console.error("Failed to bring app to front for nudge:", error);
  }
}

interface SelectConversationOptions {
  preserveHistory?: boolean;
}

function isSamePeerConversation(a: Peer | null | undefined, b: Peer | null | undefined) {
  if (!a || !b) return false;
  return isSameIdentity(a.node_id, a.id, b.node_id, b.id);
}

function getPeerEndpointKey(peer: Pick<Peer, "ip" | "port"> | null | undefined) {
  if (!peer?.ip || !peer.port) return "";
  return `${peer.ip}:${peer.port}`;
}

function getPeerRouteKey(peer: Pick<Peer, "id" | "ip" | "port">) {
  return getPeerEndpointKey(peer) || peer.id;
}

function getPeerIdentityKey(peer: Pick<Peer, "id" | "node_id" | "ip" | "port">) {
  const nodeId = peer.node_id?.trim();
  return nodeId ? `node:${nodeId}` : `endpoint:${getPeerRouteKey(peer)}`;
}

function findPeerIdentityIndex(peers: Peer[], candidate: Peer) {
  const candidateNode = candidate.node_id?.trim();
  if (candidateNode) {
    const nodeIndex = peers.findIndex((peer) => peer.node_id?.trim() === candidateNode);
    if (nodeIndex >= 0) return nodeIndex;
  }
  return peers.findIndex((peer) => {
    const peerNode = peer.node_id?.trim();
    if (candidateNode && peerNode) return candidateNode === peerNode;
    return getPeerRouteKey(peer) === getPeerRouteKey(candidate);
  });
}

function findMatchingPeer(peers: Peer[], candidate: Peer) {
  const index = findPeerIdentityIndex(peers, candidate);
  return index >= 0 ? peers[index] : undefined;
}

function findPeerBySenderIdentity(peers: Peer[], peerId: string, senderNodeId?: string | null) {
  const nodeId = senderNodeId?.trim();
  if (nodeId) {
    const byNode = peers.find((peer) => peer.node_id?.trim() === nodeId);
    if (byNode) return byNode;
  }
  return peers.find((peer) => {
    const peerNode = peer.node_id?.trim();
    if (nodeId && peerNode) return false;
    return peer.id === peerId || getPeerEndpointKey(peer) === peerId;
  });
}

function areMessageListsEqual(left: ChatMessage[], right: ChatMessage[]) {
  if (left.length !== right.length) return false;

  for (let i = 0; i < left.length; i++) {
    const a = left[i];
    const b = right[i];
    if (
      a.id !== b.id ||
      a.sender_id !== b.sender_id ||
      a.sender_node_id !== b.sender_node_id ||
      a.receiver_id !== b.receiver_id ||
      a.receiver_node_id !== b.receiver_node_id ||
      a.content !== b.content ||
      a.msg_type !== b.msg_type ||
      a.file_path !== b.file_path ||
      a.file_name !== b.file_name ||
      a.file_size !== b.file_size ||
      a.timestamp !== b.timestamp ||
      a.is_read !== b.is_read ||
      a.client_msg_id !== b.client_msg_id ||
      a.delivered !== b.delivered
    ) {
      return false;
    }
  }

  return true;
}

function arePeerListsEqual(left: Peer[], right: Peer[]) {
  if (left === right) return true;
  if (left.length !== right.length) return false;
  for (let i = 0; i < left.length; i++) {
    const a = left[i];
    const b = right[i];
    if (
      a.id !== b.id ||
      a.node_id !== b.node_id ||
      a.username !== b.username ||
      a.department !== b.department ||
      a.online !== b.online ||
      a.ip !== b.ip ||
      a.port !== b.port ||
      a.avatar_hash !== b.avatar_hash ||
      a.avatar_updated_at !== b.avatar_updated_at ||
      a.avatar_path !== b.avatar_path ||
      a.software_version !== b.software_version ||
      a.mac_address !== b.mac_address
    ) {
      return false;
    }
  }
  return true;
}

function areUnreadCountsEqual(left: UnreadCount[], right: UnreadCount[]) {
  if (left === right) return true;
  if (left.length !== right.length) return false;
  for (let i = 0; i < left.length; i++) {
    if (left[i].peer_id !== right[i].peer_id || left[i].count !== right[i].count) {
      return false;
    }
  }
  return true;
}

function areStoredPeerListsEqual(left: StoredPeer[], right: StoredPeer[]) {
  if (left === right) return true;
  if (left.length !== right.length) return false;
  for (let i = 0; i < left.length; i++) {
    const a = left[i];
    const b = right[i];
    if (
      a.peer_id !== b.peer_id ||
      a.node_id !== b.node_id ||
      a.username !== b.username ||
      a.department !== b.department ||
      a.software_version !== b.software_version ||
      a.mac_address !== b.mac_address ||
      a.avatar_path !== b.avatar_path ||
      a.avatar_hash !== b.avatar_hash ||
      a.avatar_updated_at !== b.avatar_updated_at ||
      a.ip !== b.ip ||
      a.port !== b.port ||
      a.is_online !== b.is_online ||
      a.last_seen_at !== b.last_seen_at
    ) {
      return false;
    }
  }
  return true;
}

function areGroupListsEqual(left: GroupInfo[], right: GroupInfo[]) {
  if (left === right) return true;
  if (left.length !== right.length) return false;
  for (let i = 0; i < left.length; i++) {
    const a = left[i];
    const b = right[i];
    if (
      a.group_id !== b.group_id ||
      a.name !== b.name ||
      a.creator_id !== b.creator_id ||
      a.creator_node_id !== b.creator_node_id ||
      a.unread_count !== b.unread_count ||
      a.last_message !== b.last_message ||
      a.last_message_at !== b.last_message_at ||
      a.last_message_sender !== b.last_message_sender ||
      !areStoredPeerListsEqual(a.members, b.members)
    ) {
      return false;
    }
  }
  return true;
}

function isSameStoredMessage(left: ChatMessage, right: ChatMessage) {
  if (left.id > 0 && right.id > 0) return left.id === right.id;
  if (left.client_msg_id && right.client_msg_id) return left.client_msg_id === right.client_msg_id;
  return false;
}

function compareMessages(left: ChatMessage, right: ChatMessage) {
  if (left.id > 0 && right.id > 0 && left.id !== right.id) return left.id - right.id;
  const leftTime = new Date(left.timestamp).getTime();
  const rightTime = new Date(right.timestamp).getTime();
  if (!Number.isNaN(leftTime) && !Number.isNaN(rightTime) && leftTime !== rightTime) {
    return leftTime - rightTime;
  }
  return left.id - right.id;
}

function mergeMessageIntoList(currentMessages: ChatMessage[], message: ChatMessage) {
  if (message.msg_type === "file_chunk" || message.msg_type === "file_end") {
    return currentMessages;
  }

  let changed = false;
  let found = false;
  const nextMessages = currentMessages.map((current) => {
    if (!isSameStoredMessage(current, message)) return current;
    found = true;
    if (areMessageListsEqual([current], [message])) return current;
    changed = true;
    return message;
  });

  if (!found) {
    changed = true;
    nextMessages.push(message);
  }

  if (!changed) return currentMessages;
  nextMessages.sort(compareMessages);
  return areMessageListsEqual(currentMessages, nextMessages) ? currentMessages : nextMessages;
}

function App() {
  const [appInfo, setAppInfo] = useState<AppInfo | null>(null);
  const [peers, setPeers] = useState<Peer[]>([]);
  const [selectedPeer, setSelectedPeer] = useState<Peer | null>(null);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [conversationLoading, setConversationLoading] = useState(false);
  const [hasOlderMessages, setHasOlderMessages] = useState(false);
  const [loadingOlderMessages, setLoadingOlderMessages] = useState(false);
  const [newerGapAfterId, setNewerGapAfterId] = useState<number | null>(null);
  const [loading, setLoading] = useState(true);
  const [unreadCounts, setUnreadCounts] = useState<UnreadCount[]>([]);

  const [username, setUsername] = useState("");
  const [department, setDepartment] = useState("");
  const [departmentOptions, setDepartmentOptions] = useState<string[]>([]);
  const [departmentPickerOpen, setDepartmentPickerOpen] = useState(false);
  const [savingProfile, setSavingProfile] = useState(false);
  const [profileError, setProfileError] = useState("");
  const [editingProfile, setEditingProfile] = useState(false);
  const [profileAvatarPath, setProfileAvatarPath] = useState("");
  const [profileAvatarSourcePath, setProfileAvatarSourcePath] = useState<string | null>(null);
  const [profileAvatarClearRequested, setProfileAvatarClearRequested] = useState(false);
  const [scanSubnets, setScanSubnetsState] = useState<string[]>([]);
  const [selectedGroupId, setSelectedGroupId] = useState<string | null>(null);
  const [groups, setGroups] = useState<GroupInfo[]>([]);
  const [recentRefreshKey, setRecentRefreshKey] = useState(0);
  const [themeId, setThemeId] = useState<ThemeId>(() => getInitialTheme());
  const [nudgeInterruptEnabled, setNudgeInterruptEnabled] = useState(getInitialNudgeInterruptEnabled);
  const [historySearchRequest, setHistorySearchRequest] = useState<HistorySearchRequest | null>(null);
  const [conversationResetKey, setConversationResetKey] = useState(0);
  const [nudgeSignal, setNudgeSignal] = useState<NudgeSignal | null>(null);
  const checkingUpdateRef = useRef(false);
  const departmentPickerRef = useRef<HTMLDivElement | null>(null);
  const historySearchNonceRef = useRef(0);

  // ── notification sound ────────────────────────────────────────────────
  const audioCtxRef = useRef<AudioContext | null>(null);
  const prevUnreadTotalRef = useRef(0);
  const prevGroupUnreadRef = useRef(new Map<string, number>());
  const prevContactUnreadRef = useRef(new Map<string, number>());
  const trayLastTsRef = useRef(new Map<string, number>());
  const unreadInitRef = useRef(true);
  const groupUnreadInitRef = useRef(true);
  const onlineGraceUntilRef = useRef(new Map<string, number>());
  const loadPeerStateInFlightRef = useRef(false);
  const activeConversationRef = useRef<ActiveConversation | null>(null);
  const selectedContactIdentityRef = useRef<{ id: string; endpoint: string } | null>(null);
  const selectionNonceRef = useRef(0);
  const nudgeNonceRef = useRef(0);
  const incomingNudgeCooldownRef = useRef(new Map<string, number>());
  const avatarRequestsRef = useRef(new Set<string>());
  const avatarRequestAttemptsRef = useRef(new Set<string>());

  // Silent WAV (1 sample) — used only to unlock autoplay policy on first click
  const SILENT_WAV = "data:audio/wav;base64,UklGRiQAAABXQVZFZm10IBAAAAABAAEARKwAAIhYAQACABAAZGF0YQAAAAA=";

  // Unlock audio on first user click — create AudioContext SYNCHRONOUSLY
  // during the gesture so it starts in "running" state.
  useEffect(() => {
    const warmup = () => {
      // Create during user gesture — required for running state
      if (!audioCtxRef.current) {
        const audioWindow = window as Window & typeof globalThis & {
          webkitAudioContext?: typeof AudioContext;
        };
        const AudioContextCtor = window.AudioContext ?? audioWindow.webkitAudioContext;
        if (AudioContextCtor) {
          audioCtxRef.current = new AudioContextCtor();
        }
      }
      // Belt-and-suspenders: also play a silent sound through an Audio element
      const a = new Audio(SILENT_WAV);
      a.play().catch(() => {});
    };
    document.addEventListener("click", warmup, { once: true });
    return () => document.removeEventListener("click", warmup);
  }, []);

  // Pleasant two-tone chime: C5→E5, soft and clear
  const playChime = useCallback((ctx: AudioContext) => {
    const now = ctx.currentTime;

    const tone = (freq: number, delay: number, vol: number) => {
      const t = now + delay;
      const osc = ctx.createOscillator();
      const gain = ctx.createGain();
      osc.connect(gain);
      gain.connect(ctx.destination);
      osc.type = "sine";
      osc.frequency.value = freq;
      gain.gain.setValueAtTime(0, t);
      gain.gain.linearRampToValueAtTime(vol, t + 0.02);
      gain.gain.exponentialRampToValueAtTime(0.001, t + 0.4);
      osc.start(t);
      osc.stop(t + 0.42);
    };

    tone(523, 0, 0.18);    // C5
    tone(659, 0.08, 0.14); // E5 — major third, overlapping
  }, []);

  const playNotificationSound = useCallback(async () => {
    const ctx = audioCtxRef.current;
    if (!ctx) return;
    if (ctx.state === "suspended") {
      await ctx.resume();
    }
    if (ctx.state !== "running") return;
    playChime(ctx);
  }, [playChime]);

  const promptAndDownloadUpdate = useCallback(async (result: UpdateCheckResult) => {
    const shouldDownload = await ask(formatUpdatePrompt(result), {
      title: "Echo 更新",
      type: "info",
      okLabel: "下载",
      cancelLabel: "稍后",
    });
    if (!shouldDownload) return;

    const downloaded = await downloadUpdate();
    await message(downloaded.message, {
      title: "Echo 更新",
      type: "info",
      okLabel: "确定",
    });
    if (downloaded.ready_to_restart) {
      await restartAfterUpdate();
    }
  }, []);

  const handleBackgroundUpdateAvailable = useCallback(async (result: UpdateCheckResult) => {
    if (!result.available || checkingUpdateRef.current) return;
    checkingUpdateRef.current = true;
    try {
      await promptAndDownloadUpdate(result);
    } catch (err) {
      await message(String(err), {
        title: "Echo 更新失败",
        type: "error",
      });
    } finally {
      checkingUpdateRef.current = false;
    }
  }, [promptAndDownloadUpdate]);

  useEffect(() => {
    let unlistenUpdate: (() => void) | undefined;
    listen<UpdateCheckResult>("update-available", (event) => {
      handleBackgroundUpdateAvailable(event.payload);
    }).then((fn) => {
      unlistenUpdate = fn;
    });
    return () => {
      unlistenUpdate?.();
    };
  }, [handleBackgroundUpdateAvailable]);

  // Detect new incoming CONTACT messages via unread-count changes
  // (same data that drives the sidebar red badges)
  useEffect(() => {
    const total = unreadCounts.reduce((sum, uc) => sum + uc.count, 0);
    if (unreadInitRef.current) {
      unreadInitRef.current = false;
    } else if (total > prevUnreadTotalRef.current) {
      console.log("[Echo] unread ↑ " + prevUnreadTotalRef.current + "→" + total);
      playNotificationSound();
    }
    prevUnreadTotalRef.current = total;
  }, [unreadCounts, playNotificationSound]);

  // Detect new incoming GROUP messages via per-group unread-count changes
  useEffect(() => {
    if (groupUnreadInitRef.current) {
      groupUnreadInitRef.current = false;
      for (const g of groups) prevGroupUnreadRef.current.set(g.group_id, g.unread_count || 0);
      return;
    }
    for (const g of groups) {
      const prev = prevGroupUnreadRef.current.get(g.group_id) || 0;
      const cur = g.unread_count || 0;
      if (cur > prev) {
        console.log("[Echo] group unread ↑ " + g.group_id.slice(0, 8) + " " + prev + "→" + cur);
        playNotificationSound();
      }
      prevGroupUnreadRef.current.set(g.group_id, cur);
    }
  }, [groups, playNotificationSound]);

  useEffect(() => {
    if (!appInfo?.initialized) return;
    const now = Date.now();
    const lastTs = trayLastTsRef.current;

    const items: TrayUnreadItem[] = [];
    const seen = new Set<string>();

    const peerById = new Map(peers.map((peer) => [peer.id, peer]));
    const peerByEndpoint = new Map(peers.map((peer) => [`${peer.ip}:${peer.port}`, peer]));
    const peerByNodeId = new Map(
      peers
        .map((peer) => [peer.node_id?.trim() ?? "", peer] as const)
        .filter(([nodeId]) => nodeId !== ""),
    );
    const contactBuckets = new Map<string, Omit<TrayUnreadItem, "last_ts">>();

    for (const uc of unreadCounts) {
      if (uc.count <= 0) continue;

      const peer = peerById.get(uc.peer_id) ?? peerByEndpoint.get(uc.peer_id) ?? peerByNodeId.get(uc.peer_id);
      const name = (peer?.username || uc.username || uc.peer_id).trim();
      const displayName = name || uc.peer_id;
      const routeId = peer?.id ?? uc.peer_id;
      const nodeId = peer?.node_id?.trim();
      const key = nodeId ? `contact:node:${nodeId}` : `contact:endpoint:${routeId}`;
      const existing = contactBuckets.get(key);
      const next = {
        kind: "contact" as const,
        id: routeId,
        name: displayName,
        count: uc.count,
      };

      if (!existing) {
        contactBuckets.set(key, next);
      } else {
        const keepNextName = isEndpointLike(existing.name) && !isEndpointLike(next.name);
        contactBuckets.set(key, {
          ...existing,
          id: keepNextName ? next.id : existing.id,
          name: keepNextName ? next.name : existing.name,
          count: Math.max(existing.count, next.count),
        });
      }
    }

    for (const [key, item] of contactBuckets) {
      seen.add(key);
      const prev = prevContactUnreadRef.current.get(key) ?? 0;
      if (item.count > prev || !lastTs.has(key)) {
        lastTs.set(key, now);
      }
      prevContactUnreadRef.current.set(key, item.count);
      items.push({
        ...item,
        last_ts: lastTs.get(key) ?? now,
      });
    }
    for (const key of Array.from(prevContactUnreadRef.current.keys())) {
      if (!contactBuckets.has(key)) {
        prevContactUnreadRef.current.delete(key);
      }
    }

    for (const g of groups) {
      const count = g.unread_count || 0;
      const key = `group:${g.group_id}`;
      seen.add(key);
      if (count > 0) {
        const prev = prevGroupUnreadRef.current.get(g.group_id) || 0;
        if (count > prev || !lastTs.has(key)) {
          lastTs.set(key, now);
        }
        items.push({
          kind: "group",
          id: g.group_id,
          name: g.name,
          count,
          last_ts: lastTs.get(key) ?? now,
        });
      }
    }

    for (const key of Array.from(lastTs.keys())) {
      if (!seen.has(key)) lastTs.delete(key);
    }

    updateTrayUnread(items).catch(console.error);
  }, [appInfo?.initialized, unreadCounts, groups, peers]);

  const mergePeers = useCallback((onlinePeers: Peer[], stored: StoredPeer[]): Peer[] => {
    const merged: Peer[] = [];
    const now = Date.now();
    const onlineGraceMs = 12000;

    for (const item of stored) {
      let candidate: Peer = {
        id: item.peer_id,
        node_id: item.node_id?.trim() || undefined,
        username: item.username,
        department: item.department,
        software_version: item.software_version ?? "",
        mac_address: item.mac_address ?? "",
        avatar_path: item.avatar_path ?? "",
        avatar_hash: item.avatar_hash ?? "",
        avatar_updated_at: item.avatar_updated_at ?? 0,
        ip: item.ip,
        port: item.port,
        online: item.is_online,
        last_seen: item.last_seen_at ? new Date(item.last_seen_at).getTime() / 1000 : undefined,
      };
      const graceKey = getPeerIdentityKey(candidate);
      const graceUntil = Math.max(
        onlineGraceUntilRef.current.get(graceKey) ?? 0,
        onlineGraceUntilRef.current.get(item.peer_id) ?? 0,
      );
      if (item.is_online) {
        onlineGraceUntilRef.current.set(graceKey, now + onlineGraceMs);
      }
      candidate = { ...candidate, online: item.is_online || graceUntil > now };

      const index = findPeerIdentityIndex(merged, candidate);
      if (index < 0) {
        merged.push(candidate);
        continue;
      }

      const existing = merged[index];
      const preferCandidate = candidate.online !== existing.online
        ? candidate.online
        : (candidate.last_seen ?? 0) >= (existing.last_seen ?? 0);
      const primary = preferCandidate ? candidate : existing;
      const fallback = preferCandidate ? existing : candidate;
      const newestLastSeen = Math.max(existing.last_seen ?? 0, candidate.last_seen ?? 0);
      merged[index] = {
        ...fallback,
        ...primary,
        node_id: primary.node_id?.trim() || fallback.node_id?.trim() || undefined,
        username: primary.username || fallback.username || primary.id,
        department: primary.department || fallback.department || "",
        software_version: primary.software_version || fallback.software_version || "",
        mac_address: primary.mac_address || fallback.mac_address || "",
        avatar_path: primary.avatar_path || fallback.avatar_path || "",
        avatar_hash: primary.avatar_hash || fallback.avatar_hash || "",
        avatar_updated_at: primary.avatar_updated_at || fallback.avatar_updated_at || 0,
        online: existing.online || candidate.online,
        last_seen: newestLastSeen || undefined,
      };
    }

    for (const discovered of onlinePeers) {
      const discoveredNodeId = discovered.node_id?.trim() || "";
      const discoveredRouteKey = getPeerRouteKey(discovered);
      // A node-less discovery can enrich a route only when that route has one
      // unambiguous known owner. With conflicting owners, keep a separate
      // legacy entry rather than assigning its live data to an arbitrary node.
      const routeNodeIds = new Set(
        merged
          .filter((peer) => getPeerRouteKey(peer) === discoveredRouteKey)
          .map((peer) => peer.node_id?.trim() || "")
          .filter(Boolean),
      );
      const ambiguousLegacyRoute = !discoveredNodeId && routeNodeIds.size > 1;
      const fallbackNodeId = !discoveredNodeId && routeNodeIds.size === 1
        ? routeNodeIds.values().next().value ?? ""
        : "";
      const identityNodeId = discoveredNodeId || fallbackNodeId;
      const compatiblePeers = merged.filter((peer) => {
        const peerNodeId = peer.node_id?.trim() || "";
        if (identityNodeId && peerNodeId) return identityNodeId === peerNodeId;
        if (ambiguousLegacyRoute && peerNodeId) return false;
        return getPeerRouteKey(peer) === discoveredRouteKey;
      });
      const cachedPeer = identityNodeId
        ? compatiblePeers.find((peer) => peer.node_id?.trim() === identityNodeId) ?? compatiblePeers[0]
        : compatiblePeers[0];
      const nodeId = discoveredNodeId || cachedPeer?.node_id?.trim();
      const candidate: Peer = { ...discovered, node_id: nodeId || undefined };
      const graceKey = getPeerIdentityKey(candidate);
      if (discovered.online) {
        onlineGraceUntilRef.current.set(graceKey, now + onlineGraceMs);
      }
      const graceUntil = Math.max(
        onlineGraceUntilRef.current.get(graceKey) ?? 0,
        onlineGraceUntilRef.current.get(discovered.id) ?? 0,
      );
      const nextAvatarHash = discovered.avatar_hash || cachedPeer?.avatar_hash || "";
      const nextAvatarUpdatedAt = discovered.avatar_updated_at || cachedPeer?.avatar_updated_at || 0;
      const cachedAvatarPath =
        cachedPeer?.avatar_hash && cachedPeer.avatar_hash === nextAvatarHash
          ? cachedPeer.avatar_path || ""
          : "";
      const next: Peer = {
        ...cachedPeer,
        ...discovered,
        id: discovered.id,
        node_id: nodeId || undefined,
        username: discovered.username || cachedPeer?.username || discovered.id,
        department: discovered.department || cachedPeer?.department || "",
        software_version: discovered.software_version || cachedPeer?.software_version || "",
        mac_address: discovered.mac_address || cachedPeer?.mac_address || "",
        avatar_path: discovered.avatar_path || cachedAvatarPath,
        avatar_hash: nextAvatarHash,
        avatar_updated_at: nextAvatarUpdatedAt,
        ip: discovered.ip,
        port: discovered.port,
        last_seen: discovered.last_seen ?? cachedPeer?.last_seen,
        online: discovered.online || graceUntil > now,
      };
      // Collapse every compatible alias/legacy row, not only the first match.
      // Known peers with a different node_id are deliberately left intact.
      for (let index = merged.length - 1; index >= 0; index--) {
        if (compatiblePeers.includes(merged[index])) merged.splice(index, 1);
      }
      merged.push(next);
    }

    const activeGraceKeys = new Set(merged.map(getPeerIdentityKey));
    for (const [key, until] of onlineGraceUntilRef.current) {
      if (until <= now || !activeGraceKeys.has(key)) {
        onlineGraceUntilRef.current.delete(key);
      }
    }

    return merged.sort((a, b) => {
      if (a.online !== b.online) return a.online ? -1 : 1;
      return a.username.localeCompare(b.username, "zh-CN");
    });
  }, []);

  const loadPeerState = useCallback(async () => {
    const unreadRequest = getUnreadCounts()
      .then((value) => ({ ok: true as const, value }))
      .catch((error: unknown) => ({ ok: false as const, error }));
    const [onlinePeers, storedPeers, unreadResult] = await Promise.all([
      getPeers(),
      listStoredPeers(),
      unreadRequest,
    ]);
    const mergedPeers = mergePeers(onlinePeers, storedPeers);
    // Reuse the previous array reference when content is unchanged so the 2s
    // poll doesn't force a re-render of the whole peer/message tree.
    setPeers((prev) => (arePeerListsEqual(prev, mergedPeers) ? prev : mergedPeers));
    if (unreadResult.ok) {
      setUnreadCounts((prev) =>
        areUnreadCountsEqual(prev, unreadResult.value) ? prev : unreadResult.value
      );
    } else {
      console.error("Failed to load unread counts:", unreadResult.error);
    }
    setSelectedPeer((current) => {
      if (!current) return current;
      return findMatchingPeer(mergedPeers, current) ?? current;
    });
    return mergedPeers;
  }, [mergePeers]);

  const loadMainData = useCallback(async () => {
    const [info, deps] = await Promise.all([getAppInfo(), getDepartments()]);
    setAppInfo(info);
    setDepartmentOptions(deps);
    if (info.initialized) {
      await loadPeerState();
      try {
        const subnets = await getScanSubnets();
        setScanSubnetsState(subnets);
      } catch {
        // ignore — scan subnets not critical for startup
      }
    }
  }, [loadPeerState]);

  useEffect(() => {
    async function init() {
      try {
        await loadMainData();
      } catch (err) {
        console.error("Failed to initialize:", err);
      } finally {
        setLoading(false);
      }
    }
    init();
  }, [loadMainData]);

  useEffect(() => {
    if (!appInfo?.initialized) return;

    const interval = setInterval(() => {
      if (loadPeerStateInFlightRef.current) return;
      loadPeerStateInFlightRef.current = true;
      loadPeerState()
        .catch(console.error)
        .finally(() => {
          loadPeerStateInFlightRef.current = false;
        });
    }, 2000);

    return () => {
      clearInterval(interval);
    };
  }, [appInfo?.initialized, loadPeerState]);

  useEffect(() => {
    if (!appInfo?.initialized) return;

    for (const peer of peers) {
      const avatarHash = peer.avatar_hash?.trim();
      if (!peer.online || !avatarHash || peer.avatar_path) continue;
      const requestKey = `${peer.id}:${avatarHash}`;
      if (avatarRequestsRef.current.has(requestKey) || avatarRequestAttemptsRef.current.has(requestKey)) {
        continue;
      }

      avatarRequestsRef.current.add(requestKey);
      avatarRequestAttemptsRef.current.add(requestKey);
      requestPeerAvatar(peer.id)
        .then((updated) => {
          if (!updated) return;
          return loadPeerState();
        })
        .catch((err) => {
          console.debug("Avatar request failed:", err);
        })
        .finally(() => {
          avatarRequestsRef.current.delete(requestKey);
        });
    }
  }, [appInfo?.initialized, peers, loadPeerState]);

  const selectedPeerId = selectedPeer?.id ?? null;

  useEffect(() => {
    if (selectedGroupId) {
      activeConversationRef.current = { kind: "group", id: selectedGroupId };
      return;
    }
    if (selectedPeerId) {
      activeConversationRef.current = { kind: "contact", id: selectedPeerId };
      return;
    }
    activeConversationRef.current = null;
  }, [selectedPeerId, selectedGroupId]);

  useEffect(() => {
    if (selectedGroupId || !selectedPeer) {
      selectedContactIdentityRef.current = null;
      return;
    }

    const endpoint = getPeerEndpointKey(selectedPeer);
    const currentIdentity = { id: selectedPeer.id, endpoint };
    const previousIdentity = selectedContactIdentityRef.current;
    selectedContactIdentityRef.current = currentIdentity;

    if (
      !appInfo?.initialized ||
      !endpoint ||
      !previousIdentity ||
      previousIdentity.endpoint !== endpoint ||
      previousIdentity.id === currentIdentity.id
    ) {
      return;
    }

    let cancelled = false;
    activeConversationRef.current = { kind: "contact", id: currentIdentity.id };
    getConversation(currentIdentity.id, MESSAGE_FETCH_LIMIT)
      .then((conversation) => {
        const active = activeConversationRef.current;
        if (cancelled || active?.kind !== "contact" || active.id !== currentIdentity.id) return;
        setMessages(conversation);
        setHasOlderMessages(conversation.length >= MESSAGE_FETCH_LIMIT);
        setNewerGapAfterId(null);
      })
      .catch(console.error);

    return () => {
      cancelled = true;
    };
  }, [appInfo?.initialized, selectedGroupId, selectedPeer, selectedPeerId]);

  const isActiveConversation = useCallback((kind: ConversationKind, id: string) => {
    const active = activeConversationRef.current;
    return active?.kind === kind && active.id === id;
  }, []);

  const refreshActiveConversation = useCallback(async () => {
    const active = activeConversationRef.current;
    if (!active) return;
    if (active.kind === "group") {
      const nextMessages = await getGroupMessages(active.id, MESSAGE_FETCH_LIMIT);
      if (!isActiveConversation("group", active.id)) return;
      setMessages((currentMessages) => nextMessages.reduce(mergeMessageIntoList, currentMessages));
      await markGroupRead(active.id);
      const nextGroups = await listGroups();
      setGroups(nextGroups);
      return;
    }

    const nextMessages = await getConversation(active.id, MESSAGE_FETCH_LIMIT);
    if (!isActiveConversation("contact", active.id)) return;
    setMessages((currentMessages) => nextMessages.reduce(mergeMessageIntoList, currentMessages));
    await markRead(active.id);
    await loadPeerState();
  }, [isActiveConversation, loadPeerState]);

  useEffect(() => {
    if (!appInfo?.initialized) return;
    const handleFocus = () => {
      refreshActiveConversation().catch(console.error);
    };
    window.addEventListener("focus", handleFocus);
    return () => window.removeEventListener("focus", handleFocus);
  }, [appInfo?.initialized, refreshActiveConversation]);

  // Load groups (with unread + last message)
  useEffect(() => {
    if (!appInfo?.initialized) return;
    const tick = () => {
      listGroups().then((gs) => {
        setGroups((prev) => (areGroupListsEqual(prev, gs) ? prev : gs));
        setSelectedGroupId((prev) => prev && !gs.some((g) => g.group_id === prev) ? null : prev);
      }).catch(() => {});
    };
    tick();
    const interval = setInterval(tick, 2000);
    return () => clearInterval(interval);
  }, [appInfo?.initialized]);

  const handleSelectPeer = useCallback(async (peer: Peer, options?: SelectConversationOptions) => {
    const currentActive = activeConversationRef.current;
    if (currentActive?.kind === "contact" && isSamePeerConversation(selectedPeer, peer)) {
      const shouldReloadEmptyConversation = messages.length === 0 && !conversationLoading;
      activeConversationRef.current = { kind: "contact", id: peer.id };
      setSelectedGroupId(null);
      setSelectedPeer((current) => {
        if (!current) return peer;
        return {
          ...current,
          ...peer,
          id: peer.id,
          node_id: peer.node_id?.trim() || current.node_id?.trim() || undefined,
          ip: peer.ip || current.ip,
          port: peer.port || current.port,
        };
      });
      if (!options?.preserveHistory) {
        setHistorySearchRequest(null);
      }
      if (shouldReloadEmptyConversation) {
        const nonce = ++selectionNonceRef.current;
        setConversationLoading(true);
        try {
          const [conv] = await Promise.all([
            getConversation(peer.id, MESSAGE_FETCH_LIMIT),
            markRead(peer.id),
          ]);
          if (selectionNonceRef.current !== nonce) return;
          setMessages(conv);
          setHasOlderMessages(conv.length >= MESSAGE_FETCH_LIMIT);
          setNewerGapAfterId(null);
        } catch (err) {
          console.error("Failed to reload empty conversation:", err);
        } finally {
          if (selectionNonceRef.current === nonce) {
            setConversationLoading(false);
          }
        }
      }
      return;
    }
    const nonce = ++selectionNonceRef.current;
    activeConversationRef.current = { kind: "contact", id: peer.id };
    setConversationLoading(true);
    setLoadingOlderMessages(false);
    setConversationResetKey((key) => key + 1);
    if (!options?.preserveHistory) {
      setHistorySearchRequest(null);
    }
    setSelectedGroupId(null);
    setSelectedPeer(peer);
    setMessages([]);
    setHasOlderMessages(false);
    setNewerGapAfterId(null);
    try {
      const [conv] = await Promise.all([
        getConversation(peer.id, MESSAGE_FETCH_LIMIT),
        markRead(peer.id),
      ]);
      if (selectionNonceRef.current !== nonce) return;
      setMessages((currentMessages) => conv.reduce(mergeMessageIntoList, currentMessages));
      setHasOlderMessages(conv.length >= MESSAGE_FETCH_LIMIT);
      checkPeerOnline(peer.id, peer.ip, peer.port).then((online) => {
        if (!online) {
          onlineGraceUntilRef.current.delete(getPeerIdentityKey(peer));
        }
        setPeers((prev) =>
          prev.map((p) => (p.id === peer.id ? { ...p, online } : p))
        );
        setSelectedPeer((current) =>
          current?.id === peer.id ? { ...current, online } : current
        );
      });
    } catch (err) {
      console.error("Failed to load conversation:", err);
    } finally {
      if (selectionNonceRef.current === nonce) {
        setConversationLoading(false);
      }
    }
  }, [conversationLoading, messages.length, selectedPeer]);

  const handleSelectGroup = useCallback(async (groupId: string, options?: SelectConversationOptions) => {
    const currentActive = activeConversationRef.current;
    if (currentActive?.kind === "group" && currentActive.id === groupId) {
      setSelectedPeer(null);
      setSelectedGroupId(groupId);
      if (!options?.preserveHistory) {
        setHistorySearchRequest(null);
      }
      return;
    }
    const nonce = ++selectionNonceRef.current;
    activeConversationRef.current = { kind: "group", id: groupId };
    setConversationLoading(true);
    setLoadingOlderMessages(false);
    setConversationResetKey((key) => key + 1);
    if (!options?.preserveHistory) {
      setHistorySearchRequest(null);
    }
    setSelectedPeer(null);
    setSelectedGroupId(groupId);
    setMessages([]);
    setHasOlderMessages(false);
    setNewerGapAfterId(null);
    try {
      const [msgs] = await Promise.all([
        getGroupMessages(groupId, MESSAGE_FETCH_LIMIT),
        markGroupRead(groupId),
      ]);
      if (selectionNonceRef.current !== nonce) return;
      setMessages((currentMessages) => msgs.reduce(mergeMessageIntoList, currentMessages));
      setHasOlderMessages(msgs.length >= MESSAGE_FETCH_LIMIT);
    } catch (err) {
      console.error("Failed to load group messages:", err);
    } finally {
      if (selectionNonceRef.current === nonce) {
        setConversationLoading(false);
      }
    }
  }, []);

  const peersRef = useRef(peers);
  const groupsRef = useRef(groups);
  const selectPeerRef = useRef(handleSelectPeer);
  const selectGroupRef = useRef(handleSelectGroup);
  useEffect(() => { peersRef.current = peers; }, [peers]);
  useEffect(() => { groupsRef.current = groups; }, [groups]);
  useEffect(() => { selectPeerRef.current = handleSelectPeer; }, [handleSelectPeer]);
  useEffect(() => { selectGroupRef.current = handleSelectGroup; }, [handleSelectGroup]);

  const triggerNudge = useCallback((kind: ConversationKind, targetId: string) => {
    setNudgeSignal({
      kind,
      targetId,
      nonce: ++nudgeNonceRef.current,
    });
  }, []);

  const handleNudgeSignalConsumed = useCallback((nonce: number) => {
    setNudgeSignal((current) => (current?.nonce === nonce ? null : current));
  }, []);

  const canInterruptForIncomingNudge = useCallback((kind: ConversationKind, targetId: string, senderIdentityKey: string) => {
    const key = `${kind}:${targetId}:${senderIdentityKey}`;
    const now = Date.now();
    const cooldownUntil = incomingNudgeCooldownRef.current.get(key) ?? 0;
    if (cooldownUntil > now) return false;
    incomingNudgeCooldownRef.current.set(key, now + NUDGE_COOLDOWN_MS);
    return true;
  }, []);

  const selectIncomingNudgePeer = useCallback(async (peerId: string, senderNodeId?: string | null) => {
    const currentPeer = findPeerBySenderIdentity(peersRef.current, peerId, senderNodeId);
    if (currentPeer) {
      void selectPeerRef.current(currentPeer);
      return;
    }

    try {
      const refreshedPeers = await loadPeerState();
      const refreshedPeer = findPeerBySenderIdentity(refreshedPeers, peerId, senderNodeId);
      if (refreshedPeer) {
        void selectPeerRef.current(refreshedPeer);
      }
    } catch (error) {
      console.error("Failed to select nudge peer:", error);
    }
  }, [loadPeerState]);

  useEffect(() => {
    if (!appInfo?.initialized) return;

    let disposed = false;
    let unlisten: (() => void) | undefined;
    listen<ConversationUpdatedEvent>("conversation-updated", (event) => {
      const payload = event.payload;
      setRecentRefreshKey((key) => key + 1);

      if (payload.kind === "group") {
        const groupId = payload.group_id;
        if (!groupId) return;
        const incomingNudge = isIncomingNudge(payload.message, appInfo?.peer_id, appInfo?.node_id) ? payload.message : null;
        const canInterrupt = incomingNudge
          ? canInterruptForIncomingNudge("group", groupId, incomingNudge.sender_node_id?.trim() || incomingNudge.sender_id)
          : false;
        const activeGroup = isActiveConversation("group", groupId);
        if (canInterrupt) {
          void requestAttentionForNudge();
          triggerNudge("group", groupId);
          if (nudgeInterruptEnabled) {
            void bringAppToFrontForNudge();
          }
          if (nudgeInterruptEnabled && !activeGroup) {
            void selectGroupRef.current(groupId);
          }
        }
        if (activeGroup && payload.message) {
          setMessages((currentMessages) => mergeMessageIntoList(currentMessages, payload.message!));
        }
        if (activeGroup && document.hasFocus()) {
          markGroupRead(groupId)
            .then(() => listGroups())
            .then(setGroups)
            .catch(console.error);
        } else {
          listGroups().then(setGroups).catch(console.error);
        }
        return;
      }

      const peerId = payload.peer_id;
      if (!peerId) return;
      const incomingNudge = isIncomingNudge(payload.message, appInfo?.peer_id, appInfo?.node_id) ? payload.message : null;
      const canInterrupt = incomingNudge
        ? canInterruptForIncomingNudge("contact", peerId, incomingNudge.sender_node_id?.trim() || incomingNudge.sender_id)
        : false;
      const activeContact = isActiveConversation("contact", peerId);
      if (canInterrupt) {
        void requestAttentionForNudge();
        triggerNudge("contact", peerId);
        if (nudgeInterruptEnabled) {
          void bringAppToFrontForNudge();
        }
        if (nudgeInterruptEnabled && !activeContact) {
          void selectIncomingNudgePeer(peerId, incomingNudge?.sender_node_id);
        }
      }
      if (activeContact && payload.message) {
        setMessages((currentMessages) => mergeMessageIntoList(currentMessages, payload.message!));
      }
      if (activeContact && document.hasFocus()) {
        markRead(peerId)
          .then(() => loadPeerState())
          .catch(console.error);
      } else {
        loadPeerState().catch(console.error);
      }
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });

    return () => { disposed = true; unlisten?.(); };
  }, [appInfo?.initialized, appInfo?.node_id, appInfo?.peer_id, canInterruptForIncomingNudge, loadPeerState, isActiveConversation, nudgeInterruptEnabled, selectIncomingNudgePeer, triggerNudge]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    listen<TrayUnreadItem>("tray-open-conversation", (event) => {
      const { kind, id } = event.payload;
      if (kind === "group") {
        if (groupsRef.current.some((g) => g.group_id === id)) {
          selectGroupRef.current(id);
        }
      } else {
        const peer = peersRef.current.find((p) => p.id === id);
        if (peer) selectPeerRef.current(peer);
      }
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => { disposed = true; unlisten?.(); };
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    listen<LocalPeerIdChangedEvent>("local-peer-id-changed", (event) => {
      const { newPeerId, newIp } = event.payload;
      if (!newPeerId) return;
      setAppInfo((current) => current
        ? { ...current, peer_id: newPeerId, my_ip: newIp || current.my_ip }
        : current
      );
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => { disposed = true; unlisten?.(); };
  }, []);

  // A peer changed its IP (peer_id = ip:port). The backend has already migrated
  // DB references and purged the stale in-memory entry; mirror that on the client
  // so the open conversation continues under the new id, drafts/history reload,
  // and the sidebar shows no duplicate. Endpoint reconciliation in mergePeers
  // can't bridge this because the endpoint itself changed.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    listen<PeerIdChangedEvent>("peer-id-changed", (event) => {
      const { oldPeerId, newPeerId, nodeId } = event.payload;
      if (!oldPeerId || !newPeerId || oldPeerId === newPeerId) return;

      const eventNodeId = nodeId?.trim() ?? "";
      const sep = newPeerId.lastIndexOf(":");
      const newIp = sep > 0 ? newPeerId.slice(0, sep) : "";
      const newPort = sep > 0 ? Number(newPeerId.slice(sep + 1)) : 0;
      const activeConversation = activeConversationRef.current;
      if (activeConversation?.kind === "contact" && activeConversation.id === oldPeerId) {
        // Update the synchronous routing ref before React commits selectedPeer;
        // otherwise a new-route event arriving in this gap is discarded.
        activeConversationRef.current = { kind: "contact", id: newPeerId };
      }
      const matchesMovedIdentity = (peer: Peer) => {
        const peerNodeId = peer.node_id?.trim() ?? "";
        if (eventNodeId && peerNodeId) return eventNodeId === peerNodeId;
        return peer.id === oldPeerId;
      };

      setPeers((current) => {
        const source = (eventNodeId
          ? current.find((peer) => peer.node_id?.trim() === eventNodeId)
          : undefined) ?? current.find(matchesMovedIdentity);
        if (!source) return current;
        const migrated: Peer = {
          ...source,
          id: newPeerId,
          node_id: eventNodeId || source.node_id?.trim() || undefined,
          ip: newIp || source.ip,
          port: newPort || source.port,
        };
        const next = current.filter((peer) => !matchesMovedIdentity(peer));
        const compatibleIndex = findPeerIdentityIndex(next, migrated);
        if (compatibleIndex >= 0) {
          const existing = next[compatibleIndex];
          next[compatibleIndex] = {
            ...existing,
            ...migrated,
            username: migrated.username || existing.username,
            department: migrated.department || existing.department,
            online: migrated.online || existing.online,
          };
        } else {
          next.push(migrated);
        }
        return next;
      });

      // Repoint the active conversation so its history effect refetches the
      // migrated rows under the new id.
      setSelectedPeer((current) =>
        current && matchesMovedIdentity(current)
          ? {
              ...current,
              id: newPeerId,
              node_id: eventNodeId || current.node_id?.trim() || undefined,
              ip: newIp || current.ip,
              port: newPort || current.port,
            }
          : current
      );

      setRecentRefreshKey((key) => key + 1);
    }).then((fn) => {
      if (disposed) { fn(); return; }
      unlisten = fn;
    });
    return () => { disposed = true; unlisten?.(); };
  }, []);

  const handleSendMessage = useCallback(async (content: string, clientMsgId?: string) => {
    if (!selectedPeer) throw new Error("未选择联系人");
    const targetPeerId = selectedPeer.id;
    const sent = await sendMessage(targetPeerId, content, clientMsgId);
    if (isActiveConversation("contact", targetPeerId)) {
      setMessages((prev) => mergeMessageIntoList(prev, sent));
    }
    setRecentRefreshKey((key) => key + 1);
    return sent;
  }, [isActiveConversation, selectedPeer]);

  const handleSendGroupMsg = useCallback(async (groupId: string, content: string, clientMsgId?: string) => {
    const msg = await sendGroupMessage(groupId, content, clientMsgId);
    if (isActiveConversation("group", groupId)) {
      setMessages((prev) => mergeMessageIntoList(prev, msg));
    }
    listGroups().then(setGroups).catch(console.error);
    return msg;
  }, [isActiveConversation]);

  const handleSendNudge = useCallback(async (clientMsgId?: string) => {
    if (selectedGroupId) {
      const targetGroupId = selectedGroupId;
      const msg = await sendGroupMessageTyped(targetGroupId, NUDGE_MESSAGE_CONTENT, MESSAGE_TYPE_NUDGE, clientMsgId);
      if (isActiveConversation("group", targetGroupId)) {
        setMessages((prev) => mergeMessageIntoList(prev, msg));
      }
      listGroups().then(setGroups).catch(console.error);
      return msg;
    }
    if (!selectedPeer) throw new Error("未选择联系人");
    if (!selectedPeer.online) throw new Error("对方离线，不能发送抖一抖");
    const targetPeerId = selectedPeer.id;
    const sent = await sendMessageTyped(targetPeerId, NUDGE_MESSAGE_CONTENT, MESSAGE_TYPE_NUDGE, clientMsgId);
    if (isActiveConversation("contact", targetPeerId)) {
      setMessages((prev) => mergeMessageIntoList(prev, sent));
    }
    setRecentRefreshKey((key) => key + 1);
    return sent;
  }, [isActiveConversation, selectedGroupId, selectedPeer]);

  const handleSendRps = useCallback(async (move: RpsMove, clientMsgId?: string) => {
    if (selectedGroupId) throw new Error("群聊暂不支持猜拳");
    if (!selectedPeer) throw new Error("未选择联系人");
    const targetPeerId = selectedPeer.id;
    const sent = await sendMessageTyped(targetPeerId, getRpsMessageContent(move), MESSAGE_TYPE_RPS, clientMsgId);
    if (isActiveConversation("contact", targetPeerId)) {
      setMessages((prev) => mergeMessageIntoList(prev, sent));
    }
    setRecentRefreshKey((key) => key + 1);
    return sent;
  }, [isActiveConversation, selectedGroupId, selectedPeer]);

  const handleSendFile = useCallback(async (filePath: string, clientMsgId?: string, fileName?: string | null) => {
    if (selectedGroupId) {
      return await sendGroupFile(selectedGroupId, filePath, clientMsgId, fileName);
    }
    if (!selectedPeer) throw new Error("未选择联系人");
    return await sendFile(selectedPeer.id, filePath, clientMsgId, fileName);
  }, [selectedPeer, selectedGroupId]);

  const handleSendSticker = useCallback(async (filePath: string, clientMsgId?: string) => {
    if (selectedGroupId) {
      const targetGroupId = selectedGroupId;
      const sent = await sendGroupSticker(targetGroupId, filePath, clientMsgId);
      if (sent.id > 0 && isActiveConversation("group", targetGroupId)) {
        setMessages((prev) => mergeMessageIntoList(prev, sent));
      }
      listGroups().then(setGroups).catch(console.error);
      return sent;
    }
    if (!selectedPeer) throw new Error("未选择联系人");
    return await sendSticker(selectedPeer.id, filePath, clientMsgId);
  }, [isActiveConversation, selectedPeer, selectedGroupId]);

  const handlePickProfileAvatar = useCallback(async () => {
    const selected = await open({
      multiple: false,
      filters: [{ name: "图片", extensions: ["png", "jpg", "jpeg", "gif", "webp"] }],
    });
    if (typeof selected !== "string") return;
    setProfileAvatarSourcePath(selected);
    setProfileAvatarPath(selected);
    setProfileAvatarClearRequested(false);
    setProfileError("");
  }, []);

  const handleClearProfileAvatar = useCallback(() => {
    setProfileAvatarSourcePath(null);
    setProfileAvatarPath("");
    setProfileAvatarClearRequested(true);
    setProfileError("");
  }, []);

  const handleSaveProfile = useCallback(async () => {
    const trimmedUser = username.trim();
    const trimmedDepartment = department.trim();
    if (!trimmedUser || !trimmedDepartment) {
      setProfileError("用户名和部门都必须填写");
      return;
    }

    setSavingProfile(true);
    setProfileError("");
    try {
      await saveProfile({
        username: trimmedUser,
        department: trimmedDepartment,
        avatar_source_path: profileAvatarSourcePath,
        clear_avatar: profileAvatarClearRequested,
      });
      await loadMainData();
      setEditingProfile(false);
    } catch (err) {
      console.error(err);
      setProfileError("保存失败，请重试");
    } finally {
      setSavingProfile(false);
    }
  }, [username, department, profileAvatarSourcePath, profileAvatarClearRequested, loadMainData]);

  const openEditProfile = useCallback(() => {
    if (!appInfo) return;
    setUsername(appInfo.username);
    setDepartment(appInfo.department);
    setProfileAvatarPath(appInfo.avatar_path || "");
    setProfileAvatarSourcePath(null);
    setProfileAvatarClearRequested(false);
    setProfileError("");
    setEditingProfile(true);
  }, [appInfo]);

  const refreshDepartments = useCallback(async () => {
    try {
      const deps = await getDepartments();
      setDepartmentOptions(deps);
    } catch { /* keep existing */ }
  }, []);

  const filteredDepartmentOptions = useMemo(() => {
    const query = department.trim().toLowerCase();
    const seen = new Set<string>();

    return departmentOptions.filter((dep) => {
      const normalized = dep.trim();
      if (!normalized) return false;

      const key = normalized.toLowerCase();
      if (seen.has(key)) return false;
      seen.add(key);

      return query ? key.includes(query) : true;
    });
  }, [department, departmentOptions]);

  useEffect(() => {
    if (!departmentPickerOpen) return;

    const handlePointerDown = (event: MouseEvent) => {
      if (departmentPickerRef.current?.contains(event.target as Node)) return;
      setDepartmentPickerOpen(false);
    };

    document.addEventListener("mousedown", handlePointerDown);
    return () => document.removeEventListener("mousedown", handlePointerDown);
  }, [departmentPickerOpen]);

  const handleThemeChange = useCallback((nextThemeId: ThemeId) => {
    setThemeId(nextThemeId);
    applyTheme(nextThemeId);
  }, []);

  const handleNudgeInterruptChange = useCallback((enabled: boolean) => {
    setNudgeInterruptEnabled(enabled);
    try {
      window.localStorage.setItem(NUDGE_INTERRUPT_STORAGE_KEY, enabled ? "true" : "false");
    } catch {
      // Preference persistence is optional.
    }
  }, []);

  const startHistorySearch = useCallback((kind: "contact" | "group", targetId: string, query: string, messageId?: number) => {
    const term = query.trim();
    if (!term) return;
    historySearchNonceRef.current += 1;
    setHistorySearchRequest({
      kind,
      targetId,
      query: term,
      messageId: messageId ?? null,
      nonce: historySearchNonceRef.current,
    });
  }, []);

  const handleJumpToContactSearchHit = useCallback((peerId: string, peerNodeId: string | null | undefined, query: string, messageId?: number) => {
    const nodeId = peerNodeId?.trim() ?? "";
    let peer: Peer | undefined;
    if (nodeId) {
      peer = peers.find((item) => item.node_id?.trim() === nodeId);
    } else {
      const candidates = peers.filter((item) => item.id === peerId || getPeerEndpointKey(item) === peerId);
      const first = candidates[0];
      if (first && candidates.every((item) => isSameIdentity(
        item.node_id,
        getPeerRouteKey(item),
        first.node_id,
        getPeerRouteKey(first),
      ))) {
        peer = first;
      }
    }
    if (!peer) return;
    startHistorySearch("contact", peer.id, query, messageId);
    void handleSelectPeer(peer, { preserveHistory: true });
  }, [handleSelectPeer, peers, startHistorySearch]);

  const handleJumpToGroupSearchHit = useCallback((groupId: string, query: string, messageId?: number) => {
    if (!groups.some((group) => group.group_id === groupId)) return;
    startHistorySearch("group", groupId, query, messageId);
    void handleSelectGroup(groupId, { preserveHistory: true });
  }, [groups, handleSelectGroup, startHistorySearch]);

  const handleLoadOlderMessages = useCallback(async (): Promise<number> => {
    const active = activeConversationRef.current;
    if (!active || loadingOlderMessages || !hasOlderMessages) return 0;
    const oldestId = messages.reduce<number | null>(
      (oldest, message) => message.id > 0 && (oldest === null || message.id < oldest) ? message.id : oldest,
      null,
    );
    if (oldestId === null) {
      setHasOlderMessages(false);
      return 0;
    }

    const selectionNonce = selectionNonceRef.current;
    setLoadingOlderMessages(true);
    try {
      const older = active.kind === "group"
        ? await getGroupHistory(active.id, oldestId, MESSAGE_FETCH_LIMIT, "all")
        : await getConversationHistory(active.id, oldestId, MESSAGE_FETCH_LIMIT, "all");
      if (
        selectionNonceRef.current !== selectionNonce
        || !isActiveConversation(active.kind, active.id)
      ) {
        return 0;
      }
      setMessages((current) => older.reduce(mergeMessageIntoList, current));
      setHasOlderMessages(older.length >= MESSAGE_FETCH_LIMIT);
      return older.length;
    } finally {
      if (
        selectionNonceRef.current === selectionNonce
        && isActiveConversation(active.kind, active.id)
      ) {
        setLoadingOlderMessages(false);
      }
    }
  }, [hasOlderMessages, isActiveConversation, loadingOlderMessages, messages]);

  const handleLoadHistoryContext = useCallback(async (messageId: number) => {
    const active = activeConversationRef.current;
    if (!active) return;
    const selectionNonce = selectionNonceRef.current;
    const beforeId = messageId + 1;
    const [before, after, latest] = active.kind === "group"
      ? await Promise.all([
        getGroupHistory(active.id, beforeId, HISTORY_CONTEXT_LIMIT, "all"),
        getGroupHistory(active.id, undefined, HISTORY_CONTEXT_LIMIT, "all", undefined, undefined, messageId),
        getGroupMessages(active.id, 1),
      ])
      : await Promise.all([
        getConversationHistory(active.id, beforeId, HISTORY_CONTEXT_LIMIT, "all"),
        getConversationHistory(active.id, undefined, HISTORY_CONTEXT_LIMIT, "all", undefined, undefined, messageId),
        getConversation(active.id, 1),
      ]);
    if (
      selectionNonceRef.current !== selectionNonce
      || !isActiveConversation(active.kind, active.id)
    ) {
      return;
    }

    const context = [...before, ...after].reduce(mergeMessageIntoList, []);
    const newestContextId = context.reduce((max, message) => Math.max(max, message.id), 0);
    const latestId = latest.reduce((max, message) => Math.max(max, message.id), 0);
    setMessages(context);
    setHasOlderMessages(before.length >= HISTORY_CONTEXT_LIMIT);
    setNewerGapAfterId(latestId > newestContextId ? newestContextId : null);
  }, [isActiveConversation]);

  const handleReturnToLatest = useCallback(async () => {
    const active = activeConversationRef.current;
    if (!active) return;
    const selectionNonce = selectionNonceRef.current;
    setConversationLoading(true);
    try {
      const latest = active.kind === "group"
        ? await getGroupMessages(active.id, MESSAGE_FETCH_LIMIT)
        : await getConversation(active.id, MESSAGE_FETCH_LIMIT);
      if (
        selectionNonceRef.current !== selectionNonce
        || !isActiveConversation(active.kind, active.id)
      ) {
        return;
      }
      setMessages(latest);
      setHasOlderMessages(latest.length >= MESSAGE_FETCH_LIMIT);
      setNewerGapAfterId(null);
    } finally {
      if (
        selectionNonceRef.current === selectionNonce
        && isActiveConversation(active.kind, active.id)
      ) {
        setConversationLoading(false);
      }
    }
  }, [isActiveConversation]);

  const handleDeleteMessages = useCallback(async (messageIds: number[]) => {
    const ids = Array.from(new Set(messageIds.filter((id) => Number.isFinite(id) && id > 0)));
    if (ids.length === 0) return;

    const deletedIds = new Set(ids);
    await deleteChatMessages(ids);
    setMessages((currentMessages) => currentMessages.filter((messageItem) => !deletedIds.has(messageItem.id)));
    setHistorySearchRequest((current) => (
      current?.messageId && deletedIds.has(current.messageId) ? null : current
    ));
    setRecentRefreshKey((key) => key + 1);

    if (selectedGroupId) {
      const nextGroups = await listGroups();
      setGroups(nextGroups);
      return;
    }

    await loadPeerState();
  }, [loadPeerState, selectedGroupId]);

  if (loading) {
    return (
      <div className="flex items-center justify-center h-screen bg-gray-900">
        <div className="text-center">
          <div className="w-12 h-12 border-4 border-indigo-500 border-t-transparent rounded-full animate-spin mx-auto mb-4" />
          <p className="text-gray-300 text-sm">正在启动 Echo...</p>
        </div>
      </div>
    );
  }

  if (!appInfo?.initialized || editingProfile) {
    return (
      <div className="min-h-screen bg-gray-900 text-white flex items-center justify-center px-4">
        <div className="w-full max-w-md bg-gray-800 border border-gray-700 rounded-2xl p-6 space-y-4">
          <h1 className="text-xl font-semibold">{appInfo?.initialized ? "编辑个人信息" : "首次启动设置"}</h1>
          <p className="text-sm text-gray-400">请填写用户名和部门，部门可使用已保存候选项。</p>
          <div className="flex items-center gap-4">
            <AvatarPreviewTrigger
              name={username || "我"}
              src={profileAvatarPath}
              size="xl"
              fallbackClassName="bg-indigo-500"
              title="预览头像"
            />
            <div className="flex min-w-0 flex-1 items-center gap-2">
              <button
                type="button"
                onClick={handlePickProfileAvatar}
                className="rounded-lg bg-gray-700 px-3 py-1.5 text-sm hover:bg-gray-600"
              >
                选择头像
              </button>
              {(profileAvatarPath || appInfo?.avatar_path) ? (
                <button
                  type="button"
                  onClick={handleClearProfileAvatar}
                  className="rounded-lg bg-gray-700 px-3 py-1.5 text-sm text-gray-300 hover:bg-gray-600"
                >
                  移除
                </button>
              ) : null}
            </div>
          </div>
          <div className="space-y-2">
            <label className="text-sm text-gray-300">用户名</label>
            <input value={username} onChange={(e) => setUsername(e.target.value)} placeholder="例如：张三" className="w-full bg-gray-700 text-white text-sm rounded-lg px-3 py-2 outline-none focus:ring-2 focus:ring-indigo-500" />
          </div>
          <div className="space-y-2">
            <label className="text-sm text-gray-300">部门</label>
            <div ref={departmentPickerRef} className="relative">
              <input
                value={department}
                onChange={(e) => {
                  setDepartment(e.target.value);
                  setDepartmentPickerOpen(true);
                }}
                onFocus={() => {
                  setDepartmentPickerOpen(true);
                  void refreshDepartments();
                }}
                onKeyDown={(e) => {
                  if (e.key === "Escape") setDepartmentPickerOpen(false);
                }}
                placeholder="例如：研发部"
                className="w-full bg-gray-700 text-white text-sm rounded-lg px-3 py-2 outline-none focus:ring-2 focus:ring-indigo-500"
                autoComplete="off"
              />
              {departmentPickerOpen && departmentOptions.length > 0 ? (
                <div className="absolute left-0 right-0 top-full z-50 mt-1 overflow-hidden rounded-lg border border-gray-600 bg-gray-800 shadow-xl">
                  <div className="max-h-60 overflow-y-auto py-1">
                    {filteredDepartmentOptions.length > 0 ? (
                      filteredDepartmentOptions.map((dep) => (
                        <button
                          key={dep}
                          type="button"
                          title={dep}
                          onMouseDown={(e) => e.preventDefault()}
                          onClick={() => {
                            setDepartment(dep);
                            setDepartmentPickerOpen(false);
                          }}
                          className="block w-full px-3 py-2 text-left text-sm text-gray-100 outline-none hover:bg-gray-700 focus:bg-gray-700"
                        >
                          <span className="block truncate">{dep}</span>
                        </button>
                      ))
                    ) : (
                      <div className="px-3 py-2 text-sm text-gray-400">没有匹配的部门</div>
                    )}
                  </div>
                </div>
              ) : null}
            </div>
          </div>
          {profileError ? <p className="text-sm text-red-400">{profileError}</p> : null}
          <div className="flex gap-2">
            <button onClick={handleSaveProfile} disabled={savingProfile} className="flex-1 rounded-lg bg-indigo-600 hover:bg-indigo-500 disabled:opacity-50 py-2 text-sm font-medium">
              {savingProfile ? "保存中..." : "保存"}
            </button>
            {appInfo?.initialized ? (
              <button onClick={() => setEditingProfile(false)} className="px-4 rounded-lg bg-gray-700 hover:bg-gray-600 text-sm">取消</button>
            ) : null}
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="echo-app-shell flex h-screen bg-gray-900 overflow-hidden">
      <Sidebar
        peers={peers}
        selectedPeerId={selectedPeer?.id ?? null}
        selectedPeer={selectedPeer}
        onSelectPeer={handleSelectPeer}
        onJumpToSearchHit={handleJumpToContactSearchHit}
        onJumpToGroupSearchHit={handleJumpToGroupSearchHit}
        myId={appInfo.peer_id}
        myName={appInfo.username}
        myDepartment={appInfo.department}
        mySoftwareVersion={appInfo.software_version}
        myMacAddress={appInfo.mac_address}
        myAvatarPath={appInfo.avatar_path}
        myIp={appInfo.my_ip}
        myPort={appInfo.listen_port}
        onEditProfile={openEditProfile}
        unreadCounts={unreadCounts}
        scanSubnets={scanSubnets}
        onSaveScanSubnets={async (list: string[]) => {
          await setScanSubnets(list);
          setScanSubnetsState(list);
        }}
        selectedGroupId={selectedGroupId}
        onSelectGroup={handleSelectGroup}
        groups={groups}
        themeId={themeId}
        onThemeChange={handleThemeChange}
        nudgeInterruptEnabled={nudgeInterruptEnabled}
        onNudgeInterruptChange={handleNudgeInterruptChange}
        recentRefreshKey={recentRefreshKey}
      />
      <ChatWindow
        peer={selectedGroupId ? { id: selectedGroupId, username: groups.find(g => g.group_id === selectedGroupId)?.name || "群聊", department: "", software_version: "", mac_address: "", ip: "", port: 0, online: true } : selectedPeer}
        messages={messages}
        myId={appInfo.peer_id}
        myNodeId={appInfo.node_id}
        myName={appInfo.username}
        conversationResetKey={conversationResetKey}
        loadingMessages={conversationLoading}
        hasOlderMessages={hasOlderMessages}
        loadingOlderMessages={loadingOlderMessages}
        onLoadOlderMessages={handleLoadOlderMessages}
        newerGapAfterId={newerGapAfterId}
        onReturnToLatest={handleReturnToLatest}
        isGroup={!!selectedGroupId}
        groupId={selectedGroupId}
        groupInfo={selectedGroupId ? groups.find(g => g.group_id === selectedGroupId) ?? null : null}
        peers={peers}
        groups={groups}
        onSendMessage={selectedGroupId ? ((content: string, clientMsgId?: string) => handleSendGroupMsg(selectedGroupId!, content, clientMsgId)) : handleSendMessage}
        onSendNudge={handleSendNudge}
        onSendRps={handleSendRps}
        onSendFile={handleSendFile}
        onSendSticker={handleSendSticker}
        nudgeSignal={nudgeSignal}
        onNudgeSignalConsumed={handleNudgeSignalConsumed}
        onGroupUpdated={() => listGroups().then(setGroups).catch(() => {})}
        onLoadHistoryContext={handleLoadHistoryContext}
        onDeleteMessages={handleDeleteMessages}
        historySearchRequest={
          historySearchRequest && (
            selectedGroupId
              ? historySearchRequest.kind === "group" && historySearchRequest.targetId === selectedGroupId
              : selectedPeer
                ? historySearchRequest.kind === "contact" && historySearchRequest.targetId === selectedPeer.id
                : false
          )
            ? {
              query: historySearchRequest.query,
              messageId: historySearchRequest.messageId,
              nonce: historySearchRequest.nonce,
            }
            : null
        }
      />
    </div>
  );
}

export default App;
