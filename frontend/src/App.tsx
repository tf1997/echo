import { useState, useEffect, useCallback, useRef } from "react";
import type { Peer, ChatMessage, AppInfo, StoredPeer, UnreadCount, UpdateCheckResult } from "./types";
import { ask, message } from "@tauri-apps/api/dialog";
import { listen } from "@tauri-apps/api/event";
import { Sidebar } from "./components/Sidebar";
import { ChatWindow } from "./components/ChatWindow";
import { applyTheme, getInitialTheme } from "./theme";
import type { ThemeId } from "./theme";
import {
  downloadUpdate,
  getAppInfo,
  getPeers,
  getConversation,
  sendMessage,
  sendFile,
  sendSticker,
  markRead,
  updateTrayUnread,
  checkPeerOnline,
  getDepartments,
  saveProfile,
  listStoredPeers,
  getUnreadCounts,
  getScanSubnets,
  setScanSubnets,
  getGroupMessages,
  sendGroupMessage,
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

function App() {
  const [appInfo, setAppInfo] = useState<AppInfo | null>(null);
  const [peers, setPeers] = useState<Peer[]>([]);
  const [selectedPeer, setSelectedPeer] = useState<Peer | null>(null);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [loading, setLoading] = useState(true);
  const [unreadCounts, setUnreadCounts] = useState<UnreadCount[]>([]);

  const [username, setUsername] = useState("");
  const [department, setDepartment] = useState("");
  const [departmentOptions, setDepartmentOptions] = useState<string[]>([]);
  const [savingProfile, setSavingProfile] = useState(false);
  const [profileError, setProfileError] = useState("");
  const [editingProfile, setEditingProfile] = useState(false);
  const [scanSubnets, setScanSubnetsState] = useState<string[]>([]);
  const [selectedGroupId, setSelectedGroupId] = useState<string | null>(null);
  const [groups, setGroups] = useState<GroupInfo[]>([]);
  const [themeId, setThemeId] = useState<ThemeId>(() => getInitialTheme());
  const checkingUpdateRef = useRef(false);

  // ── notification sound ────────────────────────────────────────────────
  const audioCtxRef = useRef<AudioContext | null>(null);
  const prevUnreadTotalRef = useRef(0);
  const prevGroupUnreadRef = useRef(new Map<string, number>());
  const prevContactUnreadRef = useRef(new Map<string, number>());
  const trayLastTsRef = useRef(new Map<string, number>());
  const unreadInitRef = useRef(true);
  const groupUnreadInitRef = useRef(true);
  const onlineGraceUntilRef = useRef(new Map<string, number>());

  // Silent WAV (1 sample) — used only to unlock autoplay policy on first click
  const SILENT_WAV = "data:audio/wav;base64,UklGRiQAAABXQVZFZm10IBAAAAABAAEARKwAAIhYAQACABAAZGF0YQAAAAA=";

  // Unlock audio on first user click — create AudioContext SYNCHRONOUSLY
  // during the gesture so it starts in "running" state.
  useEffect(() => {
    const warmup = () => {
      // Create during user gesture — required for running state
      if (!audioCtxRef.current) {
        const Ctor = window.AudioContext || (window as any).webkitAudioContext;
        audioCtxRef.current = new Ctor();
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

    for (const uc of unreadCounts) {
      const key = `contact:${uc.peer_id}`;
      seen.add(key);
      const prev = prevContactUnreadRef.current.get(uc.peer_id) ?? 0;
      if (uc.count > prev || !lastTs.has(key)) {
        lastTs.set(key, now);
      }
      prevContactUnreadRef.current.set(uc.peer_id, uc.count);
      if (uc.count > 0) {
        items.push({
          kind: "contact",
          id: uc.peer_id,
          name: uc.username,
          count: uc.count,
          last_ts: lastTs.get(key) ?? now,
        });
      }
    }
    for (const peerId of Array.from(prevContactUnreadRef.current.keys())) {
      if (!unreadCounts.some((uc) => uc.peer_id === peerId)) {
        prevContactUnreadRef.current.delete(peerId);
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
        ip: item.ip,
        port: item.port,
        online: item.is_online || graceKey > now,
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
      if (existingId && existingId !== peer.id) {
        map.delete(existingId);
        onlineGraceUntilRef.current.delete(existingId);
      }
      endpointToId.set(endpointKey, peer.id);
      if (peer.online) {
        onlineGraceUntilRef.current.set(peer.id, now + onlineGraceMs);
      }
      map.set(peer.id, {
        ...peer,
        online: peer.online || (onlineGraceUntilRef.current.get(peer.id) ?? 0) > now,
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
      loadPeerState().catch(console.error);
    }, 2000);

    return () => {
      clearInterval(interval);
    };
  }, [appInfo?.initialized, loadPeerState]);

  // Poll contact messages
  useEffect(() => {
    if (!appInfo?.initialized || !selectedPeer) return;
    const interval = setInterval(() => {
      const activePeerId = selectedPeer.id;
      getConversation(activePeerId).then(setMessages).catch(console.error);
      markRead(activePeerId).catch(console.error);
    }, 1000);
    return () => clearInterval(interval);
  }, [appInfo?.initialized, selectedPeer]);

  // Poll group messages
  useEffect(() => {
    if (!appInfo?.initialized || !selectedGroupId) return;
    const interval = setInterval(() => {
      getGroupMessages(selectedGroupId).then(setMessages).catch(console.error);
      markGroupRead(selectedGroupId).catch(console.error);
    }, 1000);
    return () => clearInterval(interval);
  }, [appInfo?.initialized, selectedGroupId]);

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

  const handleSelectPeer = useCallback(async (peer: Peer) => {
    setSelectedGroupId(null);
    setSelectedPeer(peer);
    setMessages([]);
    try {
      const [conv] = await Promise.all([
        getConversation(peer.id),
        markRead(peer.id),
      ]);
      setMessages(conv);
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
    }
  }, []);

  const handleSelectGroup = useCallback(async (groupId: string) => {
    setSelectedPeer(null);
    setSelectedGroupId(groupId);
    setMessages([]);
    try {
      const [msgs] = await Promise.all([
        getGroupMessages(groupId),
        markGroupRead(groupId),
      ]);
      setMessages(msgs);
    } catch (err) {
      console.error("Failed to load group messages:", err);
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
    setMessages((prev) => [...prev, sent]);
    return sent;
  }, [selectedPeer]);

  const handleSendGroupMsg = useCallback(async (groupId: string, content: string, clientMsgId?: string) => {
    const msg = await sendGroupMessage(groupId, content, clientMsgId);
    setMessages((prev) => [...prev, msg]);
    return msg;
  }, []);

  const handleSendFile = useCallback(async (filePath: string, clientMsgId?: string) => {
    if (selectedGroupId) {
      return await sendGroupFile(selectedGroupId, filePath, clientMsgId);
    }
    if (!selectedPeer) throw new Error("未选择联系人");
    sendFile(selectedPeer.id, filePath, clientMsgId).catch(console.error);
  }, [selectedPeer, selectedGroupId]);

  const handleSendSticker = useCallback(async (filePath: string, clientMsgId?: string) => {
    if (selectedGroupId) {
      const sent = await sendGroupSticker(selectedGroupId, filePath, clientMsgId);
      setMessages((prev) => [...prev, sent]);
      return sent;
    }
    if (!selectedPeer) throw new Error("未选择联系人");
    return await sendSticker(selectedPeer.id, filePath, clientMsgId);
  }, [selectedPeer, selectedGroupId]);

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
      await saveProfile({ username: trimmedUser, department: trimmedDepartment });
      await loadMainData();
      setEditingProfile(false);
    } catch (err) {
      console.error(err);
      setProfileError("保存失败，请重试");
    } finally {
      setSavingProfile(false);
    }
  }, [username, department, loadMainData]);

  const openEditProfile = useCallback(() => {
    if (!appInfo) return;
    setUsername(appInfo.username);
    setDepartment(appInfo.department);
    setProfileError("");
    setEditingProfile(true);
  }, [appInfo]);

  const refreshDepartments = useCallback(async () => {
    try {
      const deps = await getDepartments();
      setDepartmentOptions(deps);
    } catch { /* keep existing */ }
  }, []);

  const handleThemeChange = useCallback((nextThemeId: ThemeId) => {
    setThemeId(nextThemeId);
    applyTheme(nextThemeId);
  }, []);

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
          <div className="space-y-2">
            <label className="text-sm text-gray-300">用户名</label>
            <input value={username} onChange={(e) => setUsername(e.target.value)} placeholder="例如：张三" className="w-full bg-gray-700 text-white text-sm rounded-lg px-3 py-2 outline-none focus:ring-2 focus:ring-indigo-500" />
          </div>
          <div className="space-y-2">
            <label className="text-sm text-gray-300">部门</label>
            <input list="department-options" value={department} onChange={(e) => setDepartment(e.target.value)} onFocus={refreshDepartments} placeholder="例如：研发部" className="w-full bg-gray-700 text-white text-sm rounded-lg px-3 py-2 outline-none focus:ring-2 focus:ring-indigo-500" />
            <datalist id="department-options">
              {departmentOptions.map((dep) => (
                <option key={dep} value={dep} />
              ))}
            </datalist>
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
        onSelectPeer={handleSelectPeer}
        onJumpToSearchHit={(peerId: string) => {
          const peer = peers.find((p) => p.id === peerId);
          if (peer) handleSelectPeer(peer);
        }}
        myId={appInfo.peer_id}
        myName={appInfo.username}
        myDepartment={appInfo.department}
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
      />
      <ChatWindow
        peer={selectedGroupId ? { id: selectedGroupId, username: groups.find(g => g.group_id === selectedGroupId)?.name || "群聊", department: "", ip: "", port: 0, online: true } : selectedPeer}
        messages={messages}
        myId={appInfo.peer_id}
        myName={appInfo.username}
        isGroup={!!selectedGroupId}
        groupId={selectedGroupId}
        groupInfo={selectedGroupId ? groups.find(g => g.group_id === selectedGroupId) ?? null : null}
        peers={peers}
        groups={groups}
        onSendMessage={selectedGroupId ? ((content: string) => handleSendGroupMsg(selectedGroupId!, content)) : handleSendMessage}
        onSendFile={handleSendFile}
        onSendSticker={handleSendSticker}
        onGroupUpdated={() => listGroups().then(setGroups).catch(() => {})}
      />
    </div>
  );
}

export default App;
