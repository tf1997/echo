import { useState, useEffect, useCallback, useMemo, useRef } from "react";
import type { Peer, ChatMessage, AppInfo, StoredPeer, UnreadCount, UpdateCheckResult } from "./types";
import { ask, message, open } from "@tauri-apps/api/dialog";
import { listen } from "@tauri-apps/api/event";
import { appWindow } from "@tauri-apps/api/window";
import { Sidebar } from "./components/Sidebar";
import { ChatWindow } from "./components/ChatWindow";
import { AvatarPreviewTrigger } from "./components/AvatarPreview";
import { applyTheme, getInitialTheme } from "./theme";
import type { ThemeId } from "./theme";
import { MESSAGE_TYPE_NUDGE, NUDGE_COOLDOWN_MS, NUDGE_MESSAGE_CONTENT } from "./messageTypes";
import {
  downloadUpdate,
  getAppInfo,
  getPeers,
  getConversation,
  getConversationHistory,
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

const MESSAGE_FETCH_LIMIT = 500;

interface ConversationUpdatedEvent {
  kind: "contact" | "group";
  peer_id?: string | null;
  group_id?: string | null;
  message?: ChatMessage | null;
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

function isIncomingNudge(message: ChatMessage | null | undefined, myId: string | undefined): message is ChatMessage {
  return !!message && message.msg_type === MESSAGE_TYPE_NUDGE && message.sender_id !== myId;
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

function areMessageListsEqual(left: ChatMessage[], right: ChatMessage[]) {
  if (left.length !== right.length) return false;

  for (let i = 0; i < left.length; i++) {
    const a = left[i];
    const b = right[i];
    if (
      a.id !== b.id ||
      a.sender_id !== b.sender_id ||
      a.receiver_id !== b.receiver_id ||
      a.content !== b.content ||
      a.msg_type !== b.msg_type ||
      a.file_path !== b.file_path ||
      a.file_name !== b.file_name ||
      a.file_size !== b.file_size ||
      a.timestamp !== b.timestamp ||
      a.is_read !== b.is_read ||
      a.client_msg_id !== b.client_msg_id
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
    const contactBuckets = new Map<string, Omit<TrayUnreadItem, "last_ts">>();

    for (const uc of unreadCounts) {
      if (uc.count <= 0) continue;

      const peer = peerById.get(uc.peer_id) ?? peerByEndpoint.get(uc.peer_id);
      const name = (peer?.username || uc.username || uc.peer_id).trim();
      const displayName = name || uc.peer_id;
      const key = isEndpointLike(displayName)
        ? `contact:${peer?.id ?? uc.peer_id}`
        : `contact:name:${displayName.toLowerCase()}`;
      const existing = contactBuckets.get(key);
      const next = {
        kind: "contact" as const,
        id: peer?.id ?? uc.peer_id,
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
    const map = new Map<string, Peer>();
    const endpointToId = new Map<string, string>();
    const now = Date.now();
    const onlineGraceMs = 12000;

    for (const item of stored) {
      const graceKey = onlineGraceUntilRef.current.get(item.peer_id) ?? 0;
      const peer: Peer = {
        id: item.peer_id,
        username: item.username,
        department: item.department,
        software_version: item.software_version ?? "",
        mac_address: item.mac_address ?? "",
        avatar_path: item.avatar_path ?? "",
        avatar_hash: item.avatar_hash ?? "",
        avatar_updated_at: item.avatar_updated_at ?? 0,
        ip: item.ip,
        port: item.port,
        online: item.is_online || graceKey > now,
        last_seen: item.last_seen_at ? new Date(item.last_seen_at).getTime() / 1000 : undefined,
      };
      if (item.is_online) {
        onlineGraceUntilRef.current.set(peer.id, now + onlineGraceMs);
      }
      const endpointKey = `${peer.ip}:${peer.port}`;
      if (!endpointToId.has(endpointKey)) {
        endpointToId.set(endpointKey, peer.id);
        map.set(peer.id, peer);
      }
    }

    for (const peer of onlinePeers) {
      const endpointKey = `${peer.ip}:${peer.port}`;
      const existingId = endpointToId.get(endpointKey);
      const cachedPeer = existingId ? map.get(existingId) : map.get(peer.id);
      // Keep the stored conversation id stable for a known endpoint. Discovery can
      // report endpoint-shaped ids before DB identity migration catches up.
      const displayId = existingId ?? peer.id;
      const nextAvatarHash = peer.avatar_hash || cachedPeer?.avatar_hash || "";
      const nextAvatarUpdatedAt = peer.avatar_updated_at || cachedPeer?.avatar_updated_at || 0;
      const cachedAvatarPath =
        cachedPeer?.avatar_hash && cachedPeer.avatar_hash === nextAvatarHash
          ? cachedPeer.avatar_path || ""
          : "";
      endpointToId.set(endpointKey, displayId);
      if (peer.online) {
        onlineGraceUntilRef.current.set(displayId, now + onlineGraceMs);
      }
      map.set(displayId, {
        id: displayId,
        username: peer.username || cachedPeer?.username || displayId,
        department: peer.department || cachedPeer?.department || "",
        software_version: peer.software_version || cachedPeer?.software_version || "",
        mac_address: peer.mac_address || cachedPeer?.mac_address || "",
        avatar_path: peer.avatar_path || cachedAvatarPath,
        avatar_hash: nextAvatarHash,
        avatar_updated_at: nextAvatarUpdatedAt,
        ip: peer.ip,
        port: peer.port,
        last_seen: peer.last_seen ?? cachedPeer?.last_seen,
        online: peer.online || (onlineGraceUntilRef.current.get(displayId) ?? 0) > now,
      });
    }

    for (const [peerId, until] of onlineGraceUntilRef.current) {
      if (until <= now || !map.has(peerId)) {
        onlineGraceUntilRef.current.delete(peerId);
      }
    }

    return Array.from(map.values()).sort((a, b) => {
      if (a.online !== b.online) return a.online ? -1 : 1;
      return a.username.localeCompare(b.username, "zh-CN");
    });
  }, []);

  const loadPeerState = useCallback(async () => {
    const [onlinePeers, storedPeers, unread] = await Promise.all([
      getPeers(),
      listStoredPeers(),
      getUnreadCounts(),
    ]);
    const mergedPeers = mergePeers(onlinePeers, storedPeers);
    setPeers(mergedPeers);
    setUnreadCounts(unread);
    setSelectedPeer((current) => {
      if (!current) return current;
      const canonicalPeer = mergedPeers.find(
        (peer) => peer.id === current.id || (peer.ip === current.ip && peer.port === current.port)
      );
      return canonicalPeer ?? current;
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
        setGroups(gs);
        setSelectedGroupId((prev) => prev && !gs.some((g) => g.group_id === prev) ? null : prev);
      }).catch(() => {});
    };
    tick();
    const interval = setInterval(tick, 2000);
    return () => clearInterval(interval);
  }, [appInfo?.initialized]);

  const handleSelectPeer = useCallback(async (peer: Peer, options?: SelectConversationOptions) => {
    const nonce = ++selectionNonceRef.current;
    activeConversationRef.current = { kind: "contact", id: peer.id };
    setConversationLoading(true);
    setConversationResetKey((key) => key + 1);
    if (!options?.preserveHistory) {
      setHistorySearchRequest(null);
    }
    setSelectedGroupId(null);
    setSelectedPeer(peer);
    setMessages([]);
    try {
      const [conv] = await Promise.all([
        getConversation(peer.id, MESSAGE_FETCH_LIMIT),
        markRead(peer.id),
      ]);
      if (selectionNonceRef.current !== nonce) return;
      setMessages((currentMessages) => conv.reduce(mergeMessageIntoList, currentMessages));
      checkPeerOnline(peer.id, peer.ip, peer.port).then((online) => {
        if (!online) {
          onlineGraceUntilRef.current.delete(peer.id);
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
  }, []);

  const handleSelectGroup = useCallback(async (groupId: string, options?: SelectConversationOptions) => {
    const nonce = ++selectionNonceRef.current;
    activeConversationRef.current = { kind: "group", id: groupId };
    setConversationLoading(true);
    setConversationResetKey((key) => key + 1);
    if (!options?.preserveHistory) {
      setHistorySearchRequest(null);
    }
    setSelectedPeer(null);
    setSelectedGroupId(groupId);
    setMessages([]);
    try {
      const [msgs] = await Promise.all([
        getGroupMessages(groupId, MESSAGE_FETCH_LIMIT),
        markGroupRead(groupId),
      ]);
      if (selectionNonceRef.current !== nonce) return;
      setMessages((currentMessages) => msgs.reduce(mergeMessageIntoList, currentMessages));
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

  const canInterruptForIncomingNudge = useCallback((kind: ConversationKind, targetId: string, senderId: string) => {
    const key = `${kind}:${targetId}:${senderId}`;
    const now = Date.now();
    const cooldownUntil = incomingNudgeCooldownRef.current.get(key) ?? 0;
    if (cooldownUntil > now) return false;
    incomingNudgeCooldownRef.current.set(key, now + NUDGE_COOLDOWN_MS);
    return true;
  }, []);

  const selectIncomingNudgePeer = useCallback(async (peerId: string) => {
    const currentPeer = peersRef.current.find((peer) => peer.id === peerId);
    if (currentPeer) {
      void selectPeerRef.current(currentPeer);
      return;
    }

    try {
      const refreshedPeers = await loadPeerState();
      const refreshedPeer = refreshedPeers.find((peer) => peer.id === peerId);
      if (refreshedPeer) {
        void selectPeerRef.current(refreshedPeer);
      }
    } catch (error) {
      console.error("Failed to select nudge peer:", error);
    }
  }, [loadPeerState]);

  useEffect(() => {
    if (!appInfo?.initialized) return;

    let unlisten: (() => void) | undefined;
    listen<ConversationUpdatedEvent>("conversation-updated", (event) => {
      const payload = event.payload;
      setRecentRefreshKey((key) => key + 1);

      if (payload.kind === "group") {
        const groupId = payload.group_id;
        if (!groupId) return;
        const incomingNudge = isIncomingNudge(payload.message, appInfo?.peer_id) ? payload.message : null;
        const canInterrupt = incomingNudge
          ? canInterruptForIncomingNudge("group", groupId, incomingNudge.sender_id)
          : false;
        const activeGroup = isActiveConversation("group", groupId);
        if (canInterrupt) {
          void bringAppToFrontForNudge();
          triggerNudge("group", groupId);
          if (!activeGroup) {
            void selectGroupRef.current(groupId);
          }
        }
        if (activeGroup) {
          if (payload.message) {
            setMessages((currentMessages) => mergeMessageIntoList(currentMessages, payload.message!));
          }
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
      const incomingNudge = isIncomingNudge(payload.message, appInfo?.peer_id) ? payload.message : null;
      const canInterrupt = incomingNudge
        ? canInterruptForIncomingNudge("contact", peerId, incomingNudge.sender_id)
        : false;
      const activeContact = isActiveConversation("contact", peerId);
      if (canInterrupt) {
        void bringAppToFrontForNudge();
        triggerNudge("contact", peerId);
        if (!activeContact) {
          void selectIncomingNudgePeer(peerId);
        }
      }
      if (activeContact) {
        if (payload.message) {
          setMessages((currentMessages) => mergeMessageIntoList(currentMessages, payload.message!));
        }
        markRead(peerId)
          .then(() => loadPeerState())
          .catch(console.error);
      } else {
        loadPeerState().catch(console.error);
      }
    }).then((fn) => { unlisten = fn; });

    return () => {
      unlisten?.();
    };
  }, [appInfo?.initialized, appInfo?.peer_id, canInterruptForIncomingNudge, loadPeerState, isActiveConversation, selectIncomingNudgePeer, triggerNudge]);

  useEffect(() => {
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
    }).then((fn) => { unlisten = fn; });
    return () => { unlisten?.(); };
  }, []);

  const handleSendMessage = useCallback(async (content: string, clientMsgId?: string) => {
    if (!selectedPeer) throw new Error("未选择联系人");
    const sent = await sendMessage(selectedPeer.id, content, clientMsgId);
    setMessages((prev) => mergeMessageIntoList(prev, sent));
    setRecentRefreshKey((key) => key + 1);
    return sent;
  }, [selectedPeer]);

  const handleSendGroupMsg = useCallback(async (groupId: string, content: string, clientMsgId?: string) => {
    const msg = await sendGroupMessage(groupId, content, clientMsgId);
    setMessages((prev) => mergeMessageIntoList(prev, msg));
    listGroups().then(setGroups).catch(console.error);
    return msg;
  }, []);

  const handleSendNudge = useCallback(async (clientMsgId?: string) => {
    if (selectedGroupId) {
      const msg = await sendGroupMessageTyped(selectedGroupId, NUDGE_MESSAGE_CONTENT, MESSAGE_TYPE_NUDGE, clientMsgId);
      setMessages((prev) => mergeMessageIntoList(prev, msg));
      triggerNudge("group", selectedGroupId);
      listGroups().then(setGroups).catch(console.error);
      return msg;
    }
    if (!selectedPeer) throw new Error("未选择联系人");
    const sent = await sendMessageTyped(selectedPeer.id, NUDGE_MESSAGE_CONTENT, MESSAGE_TYPE_NUDGE, clientMsgId);
    setMessages((prev) => mergeMessageIntoList(prev, sent));
    triggerNudge("contact", selectedPeer.id);
    setRecentRefreshKey((key) => key + 1);
    return sent;
  }, [selectedGroupId, selectedPeer, triggerNudge]);

  const handleSendFile = useCallback(async (filePath: string, clientMsgId?: string, fileName?: string | null) => {
    if (selectedGroupId) {
      return await sendGroupFile(selectedGroupId, filePath, clientMsgId, fileName);
    }
    if (!selectedPeer) throw new Error("未选择联系人");
    return await sendFile(selectedPeer.id, filePath, clientMsgId, fileName);
  }, [selectedPeer, selectedGroupId]);

  const handleSendSticker = useCallback(async (filePath: string, clientMsgId?: string) => {
    if (selectedGroupId) {
      const sent = await sendGroupSticker(selectedGroupId, filePath, clientMsgId);
      if (sent.id > 0) {
        setMessages((prev) => mergeMessageIntoList(prev, sent));
      }
      listGroups().then(setGroups).catch(console.error);
      return sent;
    }
    if (!selectedPeer) throw new Error("未选择联系人");
    return await sendSticker(selectedPeer.id, filePath, clientMsgId);
  }, [selectedPeer, selectedGroupId]);

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

  const handleJumpToContactSearchHit = useCallback((peerId: string, query: string, messageId?: number) => {
    const peer = peers.find((item) => item.id === peerId);
    if (!peer) return;
    startHistorySearch("contact", peer.id, query, messageId);
    void handleSelectPeer(peer, { preserveHistory: true });
  }, [handleSelectPeer, peers, startHistorySearch]);

  const handleJumpToGroupSearchHit = useCallback((groupId: string, query: string, messageId?: number) => {
    if (!groups.some((group) => group.group_id === groupId)) return;
    startHistorySearch("group", groupId, query, messageId);
    void handleSelectGroup(groupId, { preserveHistory: true });
  }, [groups, handleSelectGroup, startHistorySearch]);

  const handleLoadHistoryContext = useCallback(async (messageId: number) => {
    const beforeId = messageId + 1;
    if (selectedGroupId) {
      const context = await getGroupHistory(selectedGroupId, beforeId, MESSAGE_FETCH_LIMIT, "all");
      setMessages(context);
      return;
    }
    if (selectedPeer) {
      const context = await getConversationHistory(selectedPeer.id, beforeId, MESSAGE_FETCH_LIMIT, "all");
      setMessages(context);
    }
  }, [selectedGroupId, selectedPeer]);

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
    <div className="flex h-screen bg-gray-900 overflow-hidden">
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
        recentRefreshKey={recentRefreshKey}
      />
      <ChatWindow
        peer={selectedGroupId ? { id: selectedGroupId, username: groups.find(g => g.group_id === selectedGroupId)?.name || "群聊", department: "", software_version: "", mac_address: "", ip: "", port: 0, online: true } : selectedPeer}
        messages={messages}
        myId={appInfo.peer_id}
        myName={appInfo.username}
        conversationResetKey={conversationResetKey}
        loadingMessages={conversationLoading}
        isGroup={!!selectedGroupId}
        groupId={selectedGroupId}
        groupInfo={selectedGroupId ? groups.find(g => g.group_id === selectedGroupId) ?? null : null}
        peers={peers}
        groups={groups}
        onSendMessage={selectedGroupId ? ((content: string, clientMsgId?: string) => handleSendGroupMsg(selectedGroupId!, content, clientMsgId)) : handleSendMessage}
        onSendNudge={handleSendNudge}
        onSendFile={handleSendFile}
        onSendSticker={handleSendSticker}
        nudgeSignal={nudgeSignal}
        onGroupUpdated={() => listGroups().then(setGroups).catch(() => {})}
        onLoadHistoryContext={handleLoadHistoryContext}
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
