import { useState, useEffect, useCallback } from "react";
import type { Peer, ChatMessage, AppInfo, StoredPeer, UnreadCount } from "./types";
import { Sidebar } from "./components/Sidebar";
import { ChatWindow } from "./components/ChatWindow";
import {
  getAppInfo,
  getPeers,
  getConversation,
  sendMessage,
  sendFile,
  markRead,
  getDepartments,
  saveProfile,
  listStoredPeers,
  getUnreadCounts,
} from "./api";

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

  const mergePeers = useCallback((onlinePeers: Peer[], stored: StoredPeer[]): Peer[] => {
    const map = new Map<string, Peer>();
    const endpointToId = new Map<string, string>();

    for (const item of stored) {
      const peer: Peer = {
        id: item.peer_id,
        username: item.username,
        department: item.department,
        ip: item.ip,
        port: item.port,
        online: item.is_online,
      };
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
      }
      endpointToId.set(endpointKey, peer.id);
      map.set(peer.id, peer);
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

  useEffect(() => {
    if (!appInfo?.initialized || !selectedPeer) return;

    const interval = setInterval(() => {
      const activePeerId = selectedPeer.id;
      getConversation(activePeerId)
        .then(setMessages)
        .catch(console.error);
      markRead(activePeerId).catch(console.error);
    }, 1000);

    return () => clearInterval(interval);
  }, [appInfo?.initialized, selectedPeer]);

  const handleSelectPeer = useCallback(async (peer: Peer) => {
    setSelectedPeer(peer);
    try {
      const conv = await getConversation(peer.id);
      setMessages(conv);
      await markRead(peer.id);
    } catch (err) {
      console.error("Failed to load conversation:", err);
      setMessages([]);
    }
  }, []);

  const handleSendMessage = useCallback(async (content: string) => {
    if (!selectedPeer) return;
    try {
      const sent = await sendMessage(selectedPeer.id, content);
      setMessages((prev) => [...prev, sent]);
    } catch (err) {
      console.error("Failed to send message:", err);
    }
  }, [selectedPeer]);

  const handleSendFile = useCallback(async (filePath: string) => {
    if (!selectedPeer) return;
    try {
      const sent = await sendFile(selectedPeer.id, filePath);
      setMessages((prev) => [...prev, sent]);
    } catch (err) {
      console.error("Failed to send file:", err);
    }
  }, [selectedPeer]);

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
            <input list="department-options" value={department} onChange={(e) => setDepartment(e.target.value)} placeholder="例如：研发部" className="w-full bg-gray-700 text-white text-sm rounded-lg px-3 py-2 outline-none focus:ring-2 focus:ring-indigo-500" />
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
        myId={appInfo.peer_id}
        myName={appInfo.username}
        myDepartment={appInfo.department}
        onEditProfile={openEditProfile}
        unreadCounts={unreadCounts}
      />
      <ChatWindow
        peer={selectedPeer}
        messages={messages}
        myId={appInfo.peer_id}
        onSendMessage={handleSendMessage}
        onSendFile={handleSendFile}
      />
    </div>
  );
}

export default App;
