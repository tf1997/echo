import { useState, useCallback, useEffect, useMemo, useRef } from "react";
import type { ChatMessage, Peer, UnreadCount, StoredPeer } from "../types";
import { searchMessages, searchGroupChatMessages, discoverByIp, listRecentContacts, removeRecentContact, createGroup, refreshPeerProfile } from "../api";
import { THEMES } from "../theme";
import type { ThemeId } from "../theme";
import type { GroupInfo, SearchResult } from "../api";

interface SidebarProps {
  peers: Peer[];
  selectedPeerId: string | null;
  onSelectPeer: (peer: Peer) => void;
  onJumpToSearchHit: (peerId: string, query: string, messageId?: number) => void;
  onJumpToGroupSearchHit: (groupId: string, query: string, messageId?: number) => void;
  myId: string;
  myName: string;
  myDepartment: string;
  mySoftwareVersion: string;
  myMacAddress: string;
  myIp: string;
  myPort: number;
  onEditProfile: () => void;
  unreadCounts: UnreadCount[];
  scanSubnets: string[];
  onSaveScanSubnets: (subnets: string[]) => Promise<void>;
  selectedGroupId: string | null;
  onSelectGroup: (groupId: string) => void;
  groups: GroupInfo[];
  themeId: ThemeId;
  onThemeChange: (themeId: ThemeId) => void;
}

export function Sidebar({ peers, selectedPeerId, onSelectPeer, myId, myName, myDepartment, mySoftwareVersion, myMacAddress, myIp, myPort, onEditProfile, unreadCounts, scanSubnets, onSaveScanSubnets, onJumpToSearchHit, onJumpToGroupSearchHit, selectedGroupId, onSelectGroup, groups, themeId, onThemeChange }: SidebarProps) {
  const [searchQuery, setSearchQuery] = useState("");
  const [searchResults, setSearchResults] = useState<SearchResult[]>([]);
  const [groupSearchResults, setGroupSearchResults] = useState<{ group: GroupInfo; messages: ChatMessage[] }[]>([]);
  const [searching, setSearching] = useState(false);
  const [searchError, setSearchError] = useState("");
  const [showSearch, setShowSearch] = useState(false);
  const [showProfile, setShowProfile] = useState(false);
  const [profilePeer, setProfilePeer] = useState<Peer | null>(null);
  const [refreshingProfileId, setRefreshingProfileId] = useState("");
  const [showThemeMenu, setShowThemeMenu] = useState(false);
  const [copied, setCopied] = useState("");
  const [subnetInput, setSubnetInput] = useState(scanSubnets.join(", "));
  const [savingSubnets, setSavingSubnets] = useState(false);
  const [tab, setTab] = useState<"recent" | "contacts" | "groups">("recent");
  const [contactQuery, setContactQuery] = useState("");
  const [recentContacts, setRecentContacts] = useState<StoredPeer[]>([]);
  const [showCreateGroup, setShowCreateGroup] = useState(false);
  const [newGroupName, setNewGroupName] = useState("");
  const [newGroupMembers, setNewGroupMembers] = useState<string[]>([]);
  const [newGroupMemberQuery, setNewGroupMemberQuery] = useState("");
  const [expandedDepts, setExpandedDepts] = useState<Set<string>>(new Set());
  const [groupNameError, setGroupNameError] = useState("");
  const themeMenuRef = useRef<HTMLDivElement>(null);
  const searchTimerRef = useRef<number | null>(null);
  const searchSeqRef = useRef(0);

  const toggleDept = useCallback((dept: string) => {
    setExpandedDepts((prev) => {
      const next = new Set(prev);
      if (next.has(dept)) next.delete(dept);
      else next.add(dept);
      return next;
    });
  }, []);

  useEffect(() => { listRecentContacts().then(setRecentContacts).catch(() => {}); }, [peers, tab]);

  useEffect(() => {
    return () => {
      if (searchTimerRef.current !== null) {
        window.clearTimeout(searchTimerRef.current);
      }
    };
  }, []);

  useEffect(() => {
    if (!profilePeer) return;
    const peer = profilePeer;
    const missingMetadata = !peer.software_version || !peer.mac_address;
    if (!missingMetadata || !peer.ip || !peer.port) return;

    let cancelled = false;
    queueMicrotask(() => {
      if (!cancelled) setRefreshingProfileId(peer.id);
    });
    refreshPeerProfile(peer.id, peer.ip, peer.port)
      .then((stored) => {
        if (cancelled || !stored) return;
        setProfilePeer((current) => {
          if (!current || current.id !== peer.id) return current;
          return {
            ...current,
            id: stored.peer_id,
            username: stored.username,
            department: stored.department,
            software_version: stored.software_version ?? "",
            mac_address: stored.mac_address ?? "",
            ip: stored.ip,
            port: stored.port,
            online: stored.is_online,
            last_seen: stored.last_seen_at ? new Date(stored.last_seen_at).getTime() / 1000 : current.last_seen,
          };
        });
      })
      .catch(console.error)
      .finally(() => {
        if (!cancelled) setRefreshingProfileId("");
      });

    return () => {
      cancelled = true;
    };
  }, [profilePeer]);

  // Close theme menu when clicking outside
  useEffect(() => {
    if (!showThemeMenu) return;
    const handleClickOutside = (e: MouseEvent) => {
      if (themeMenuRef.current && !themeMenuRef.current.contains(e.target as Node)) {
        setShowThemeMenu(false);
      }
    };
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [showThemeMenu]);

  const handleRemoveRecent = useCallback(async (peerId: string) => {
    await removeRecentContact(peerId).catch(() => {});
    setRecentContacts((prev) => prev.filter((r) => r.peer_id !== peerId));
  }, []);

  const handleCreateGroup = useCallback(async () => {
    const trimmedName = newGroupName.trim();
    if (!trimmedName) {
      setGroupNameError("群组名称不能为空");
      return;
    }
    if (trimmedName.length > 50) {
      setGroupNameError("群组名称不能超过50个字符");
      return;
    }
    if (newGroupMembers.length === 0) {
      setGroupNameError("请至少选择一个成员");
      return;
    }
    try {
      await createGroup(trimmedName, newGroupMembers);
      setShowCreateGroup(false);
      setNewGroupName("");
      setNewGroupMembers([]);
      setNewGroupMemberQuery("");
      setGroupNameError("");
    } catch (e) {
      console.error("Failed to create group:", e);
      setGroupNameError("创建群组失败，请重试");
    }
  }, [newGroupName, newGroupMembers]);

  const handleSaveSubnets = useCallback(async () => {
    setSavingSubnets(true);
    const list = subnetInput
      .split(/[,，\s]+/)
      .map((s) => s.trim())
      .filter((s) => s.length > 0);
    try {
      await onSaveScanSubnets(list);
    } finally {
      setSavingSubnets(false);
    }
  }, [subnetInput, onSaveScanSubnets]);

  const copyToClipboard = useCallback(async (text: string, label: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(label);
      setTimeout(() => setCopied(""), 1500);
    } catch {
      // fallback
    }
  }, []);

  const contactFilter = contactQuery.trim().toLowerCase();
  const visiblePeers = useMemo(() => {
    if (!contactFilter) return peers;

    return peers.filter((peer) => {
      const fields = [
        peer.username,
        peer.department,
        peer.software_version ?? "",
        peer.mac_address ?? "",
        peer.ip,
        `${peer.ip}:${peer.port}`,
        peer.online ? "在线" : "离线",
      ];

      return fields.some((field) => field.toLowerCase().includes(contactFilter));
    });
  }, [contactFilter, peers]);

  const groupMemberFilter = newGroupMemberQuery.trim().toLowerCase();
  const selectableGroupPeers = useMemo(() => {
    if (!groupMemberFilter) return peers;
    return peers.filter((peer) => {
      const fields = [
        peer.username,
        peer.department,
        peer.ip,
        `${peer.ip}:${peer.port}`,
        peer.online ? "在线" : "离线",
      ];
      return fields.some((field) => field.toLowerCase().includes(groupMemberFilter));
    });
  }, [groupMemberFilter, peers]);

  // Group peers by department for "contacts" tab
  const deptGroups = new Map<string, Peer[]>();
  for (const p of visiblePeers) {
    const dept = p.department || "未分组";
    if (!deptGroups.has(dept)) deptGroups.set(dept, []);
    deptGroups.get(dept)!.push(p);
  }
  const sortedDepts = [...deptGroups.keys()].sort((a, b) => {
    if (a === "未分组") return 1;
    if (b === "未分组") return -1;
    return a.localeCompare(b);
  });

  const unreadMap = new Map<string, number>();
  for (const uc of unreadCounts) {
    unreadMap.set(uc.peer_id, uc.count);
  }
  const peerById = new Map(peers.map((peer) => [peer.id, peer]));
  const recentGroups = groups
    .filter((group) => !!group.last_message_at || (group.unread_count || 0) > 0)
    .sort((a, b) => {
      const aTime = a.last_message_at ? new Date(a.last_message_at).getTime() : 0;
      const bTime = b.last_message_at ? new Date(b.last_message_at).getTime() : 0;
      return bTime - aTime;
    });

  const recentTotalUnread =
    recentContacts.reduce((sum, contact) => sum + (unreadMap.get(contact.peer_id) ?? 0), 0) +
    recentGroups.reduce((sum, group) => sum + (group.unread_count || 0), 0);
  const groupsTotalUnread = groups.reduce((sum, g) => sum + (g.unread_count || 0), 0);
  const currentTheme = THEMES.find((theme) => theme.id === themeId) ?? THEMES[0];

  const handleSearchChange = useCallback((value: string) => {
    setSearchQuery(value);
    searchSeqRef.current += 1;
    const seq = searchSeqRef.current;

    if (searchTimerRef.current !== null) {
      window.clearTimeout(searchTimerRef.current);
      searchTimerRef.current = null;
    }

    if (!value.trim()) {
      setSearchResults([]);
      setGroupSearchResults([]);
      setSearchError("");
      setSearching(false);
      setShowSearch(false);
      return;
    }

    setSearching(true);
    setSearchError("");
    setShowSearch(true);

    searchTimerRef.current = window.setTimeout(() => {
      searchTimerRef.current = null;
      const term = value.trim();
      Promise.all([
        searchMessages(term),
        Promise.all(groups.map(async (group) => ({
          group,
          messages: await searchGroupChatMessages(group.group_id, term, 5).catch(() => []),
        }))),
      ])
        .then(([results, groupResults]) => {
          if (searchSeqRef.current !== seq) return;
          setSearchResults(results);
          setGroupSearchResults(groupResults.filter((result) => result.messages.length > 0));
        })
        .catch((e) => {
          if (searchSeqRef.current !== seq) return;
          console.error(e);
          setSearchResults([]);
          setGroupSearchResults([]);
          setSearchError("搜索失败，请稍后重试");
        })
        .finally(() => {
          if (searchSeqRef.current === seq) setSearching(false);
        });
    }, 250);
  }, [groups]);

  const clearGlobalSearch = useCallback(() => {
    setShowSearch(false);
    setSearchQuery("");
    setSearchResults([]);
    setGroupSearchResults([]);
    setSearchError("");
    setSearching(false);
    searchSeqRef.current += 1;
    if (searchTimerRef.current !== null) {
      window.clearTimeout(searchTimerRef.current);
      searchTimerRef.current = null;
    }
  }, []);

  const handleJumpToHit = useCallback((peerId: string, messageId?: number) => {
    const term = searchQuery.trim();
    clearGlobalSearch();
    onJumpToSearchHit(peerId, term, messageId);
  }, [clearGlobalSearch, onJumpToSearchHit, searchQuery]);

  const handleJumpToGroupHit = useCallback((groupId: string, messageId?: number) => {
    const term = searchQuery.trim();
    clearGlobalSearch();
    onJumpToGroupSearchHit(groupId, term, messageId);
  }, [onJumpToGroupSearchHit, searchQuery, clearGlobalSearch]);

  const [manualIp, setManualIp] = useState("");
  const [manualPort, setManualPort] = useState("9527");
  const [searchingIp, setSearchingIp] = useState(false);
  const [ipSearchMsg, setIpSearchMsg] = useState("");
  const [ipSearchStatus, setIpSearchStatus] = useState<"idle" | "success" | "error">("idle");

  const handleDiscoverIp = useCallback(async () => {
    const ip = manualIp.trim();
    const port = Number.parseInt(manualPort.trim(), 10);
    const validIpv4 = /^(25[0-5]|2[0-4]\d|1?\d?\d)(\.(25[0-5]|2[0-4]\d|1?\d?\d)){3}$/.test(ip);
    if (!ip) {
      setIpSearchStatus("error");
      setIpSearchMsg("请输入 IP 地址");
      return;
    }
    if (!validIpv4) {
      setIpSearchStatus("error");
      setIpSearchMsg("请输入有效 IPv4 地址");
      return;
    }
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      setIpSearchStatus("error");
      setIpSearchMsg("端口范围应为 1-65535");
      return;
    }
    setSearchingIp(true);
    setIpSearchMsg("");
    setIpSearchStatus("idle");
    try {
      const result = await discoverByIp(ip, port);
      setIpSearchMsg(result.message);
      setIpSearchStatus(result.online ? "success" : "error");
    } catch (e) {
      setIpSearchMsg(String(e));
      setIpSearchStatus("error");
    } finally {
      setSearchingIp(false);
    }
  }, [manualIp, manualPort]);

  const ipSearchMessageClass =
    ipSearchStatus === "success"
      ? "text-green-400"
      : ipSearchStatus === "error"
        ? "text-red-400"
        : "text-gray-400";

  return (
    <div className="app-sidebar relative flex flex-col w-72 bg-gray-900 text-white h-full border-r border-gray-700">
      <div className="p-4 border-b border-gray-700 relative">
        <div className="flex items-center gap-3">
          <button
            onClick={() => setShowProfile(!showProfile)}
            className="own-profile-avatar relative flex-shrink-0 w-10 h-10 rounded-full bg-indigo-500 hover:bg-indigo-400 transition-colors flex items-center justify-center text-lg font-bold cursor-pointer"
            title="查看个人信息"
          >
            {myName.charAt(0).toUpperCase()}
          </button>
          <button
            onClick={() => setShowProfile(!showProfile)}
            className="flex-1 min-w-0 text-left cursor-pointer hover:opacity-80"
          >
            <p className="text-sm font-semibold truncate">{myName}</p>
            <p className="text-xs text-gray-400 truncate">{myDepartment}</p>
            <p className="text-[10px] text-gray-500 truncate">ID: {myId.slice(0, 8)}...</p>
          </button>
          <button
            onClick={onEditProfile}
            className="text-xs px-2 py-1 rounded bg-gray-700 hover:bg-gray-600"
          >
            编辑
          </button>
        </div>

        {showProfile && (
          <div className="absolute top-full left-2 right-2 mt-1 z-50 bg-gray-800 border border-gray-600 rounded-xl p-4 shadow-2xl">
            <div className="flex items-center gap-3 mb-3">
              <div className="own-profile-avatar w-12 h-12 rounded-full bg-indigo-500 flex items-center justify-center text-xl font-bold">
                {myName.charAt(0).toUpperCase()}
              </div>
              <div>
                <p className="text-sm font-semibold">{myName}</p>
                <p className="text-xs text-gray-400">{myDepartment}</p>
              </div>
            </div>
            <div className="space-y-1.5 text-xs">
              <div className="flex items-center gap-1">
                <span className="text-gray-400 w-16 flex-shrink-0">Peer ID</span>
                <span className="text-gray-200 font-mono text-[10px] truncate flex-1" title={myId}>{myId.slice(0, 16)}...</span>
                <button onClick={() => copyToClipboard(myId, "Peer ID")} className="flex-shrink-0 w-5 h-5 rounded hover:bg-white/10 flex items-center justify-center">
                  {copied === "Peer ID" ? (
                    <span className="text-[10px] text-green-400">✓</span>
                  ) : (
                    <svg className="w-3 h-3 text-gray-500" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z" /></svg>
                  )}
                </button>
              </div>
              <div className="flex items-center gap-1">
                <span className="text-gray-400 w-16 flex-shrink-0">IP</span>
                <span className="text-gray-200 font-mono flex-1">{myIp}</span>
                <button onClick={() => copyToClipboard(myIp, "IP")} className="flex-shrink-0 w-5 h-5 rounded hover:bg-white/10 flex items-center justify-center">
                  {copied === "IP" ? (
                    <span className="text-[10px] text-green-400">✓</span>
                  ) : (
                    <svg className="w-3 h-3 text-gray-500" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z" /></svg>
                  )}
                </button>
              </div>
              <div className="flex items-center gap-1">
                <span className="text-gray-400 w-16 flex-shrink-0">端口</span>
                <span className="text-gray-200 font-mono flex-1">{myPort}</span>
                <div className="w-5 h-5" />
              </div>
              <div className="flex items-center gap-1">
                <span className="text-gray-400 w-16 flex-shrink-0">用户名</span>
                <span className="text-gray-200 flex-1">{myName}</span>
                <div className="w-5 h-5" />
              </div>
              <div className="flex items-center gap-1">
                <span className="text-gray-400 w-16 flex-shrink-0">部门</span>
                <span className="text-gray-200 flex-1">{myDepartment}</span>
                <div className="w-5 h-5" />
              </div>
              <div className="flex items-center gap-1">
                <span className="text-gray-400 w-16 flex-shrink-0">软件版本</span>
                <span className="text-gray-200 font-mono flex-1 truncate">{mySoftwareVersion || "未知"}</span>
                <div className="w-5 h-5" />
              </div>
              <div className="flex items-center gap-1">
                <span className="text-gray-400 w-16 flex-shrink-0">MAC地址</span>
                <span className="text-gray-200 font-mono text-[10px] truncate flex-1" title={myMacAddress || "未知"}>{myMacAddress || "未知"}</span>
                <button onClick={() => copyToClipboard(myMacAddress, "MAC地址")} disabled={!myMacAddress} className="flex-shrink-0 w-5 h-5 rounded hover:bg-white/10 disabled:opacity-30 flex items-center justify-center">
                  {copied === "MAC地址" ? (
                    <span className="text-[10px] text-green-400">✓</span>
                  ) : (
                    <svg className="w-3 h-3 text-gray-500" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z" /></svg>
                  )}
                </button>
              </div>
            </div>
            <div className="border-t border-gray-700 my-3 pt-3">
              <p className="text-xs text-gray-400 mb-2">网段扫描（跨子网发现）</p>
              <input
                type="text"
                value={subnetInput}
                onChange={(e) => setSubnetInput(e.target.value)}
                placeholder="例: 10.100.0, 10.101.0"
                className="w-full bg-gray-900 border border-gray-600 rounded px-2 py-1.5 text-xs text-gray-200 outline-none focus:border-indigo-500"
              />
              <button
                onClick={handleSaveSubnets}
                disabled={savingSubnets}
                className="mt-2 w-full text-center text-xs py-1 rounded bg-indigo-600 hover:bg-indigo-500 transition-colors disabled:opacity-50"
              >
                {savingSubnets ? "保存中..." : "保存网段"}
              </button>
              <p className="text-[10px] text-gray-500 mt-1">
                留空则不扫描 · 5 分钟间隔 · IP 随机
              </p>
            </div>
            <div className="border-t border-gray-700 my-3 pt-3">
              <p className="text-xs text-gray-400 mb-2">皮肤</p>
              <div className="flex items-center gap-1.5">
                {THEMES.map((theme) => (
                  <button
                    key={theme.id}
                    type="button"
                    onClick={() => onThemeChange(theme.id)}
                    className={`theme-swatch ${themeId === theme.id ? "theme-swatch-active" : ""}`}
                    title={theme.name}
                    aria-label={`切换到${theme.name}皮肤`}
                  >
                    <span style={{ background: theme.preview[0] }} />
                    <span style={{ background: theme.preview[1] }} />
                    <span style={{ background: theme.preview[2] }} />
                  </button>
                ))}
              </div>
            </div>
            <button
              onClick={() => setShowProfile(false)}
              className="mt-1 w-full text-center text-xs py-1.5 rounded-lg bg-gray-700 hover:bg-gray-600 transition-colors"
            >
              关闭
            </button>
          </div>
        )}

      </div>

      {/* Search input */}
      <div className="px-4 py-3">
        <div className="relative">
          <svg className="w-4 h-4 absolute left-2.5 top-1/2 -translate-y-1/2 text-gray-500" fill="none" viewBox="0 0 24 24" stroke="currentColor">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
          </svg>
          <input
            type="text"
            value={searchQuery}
            onChange={(e) => handleSearchChange(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Escape") clearGlobalSearch(); }}
            placeholder="搜索聊天记录..."
            className="w-full bg-gray-800 text-sm text-gray-200 rounded-lg pl-8 pr-8 py-2 outline-none focus:ring-2 focus:ring-indigo-500 placeholder-gray-500"
          />
          {searchQuery ? (
            <button
              type="button"
              onClick={clearGlobalSearch}
              className="absolute right-2 top-1/2 -translate-y-1/2 w-5 h-5 rounded-full text-gray-500 hover:text-gray-300 hover:bg-gray-700 flex items-center justify-center"
              aria-label="清空聊天记录搜索"
            >
              ×
            </button>
          ) : null}
        </div>
      </div>

      {/* Manual IP search */}
      <div className="px-4 pb-2">
        <div className="grid grid-cols-[minmax(0,1fr)_4.25rem_auto] gap-2">
          <input
            type="text"
            value={manualIp}
            onChange={(e) => {
              setManualIp(e.target.value);
              setIpSearchMsg("");
              setIpSearchStatus("idle");
            }}
            placeholder="IP 地址"
            className="min-w-0 bg-gray-800 text-xs text-gray-200 rounded px-2 py-1.5 outline-none focus:ring-1 focus:ring-indigo-500 placeholder-gray-500"
            onKeyDown={(e) => { if (e.key === "Enter") handleDiscoverIp(); }}
          />
          <input
            type="text"
            value={manualPort}
            onChange={(e) => {
              setManualPort(e.target.value);
              setIpSearchMsg("");
              setIpSearchStatus("idle");
            }}
            placeholder="9527"
            className="min-w-0 bg-gray-800 text-xs text-gray-200 rounded px-2 py-1.5 outline-none focus:ring-1 focus:ring-indigo-500 placeholder-gray-500"
            onKeyDown={(e) => { if (e.key === "Enter") handleDiscoverIp(); }}
          />
          <button
            onClick={handleDiscoverIp}
            disabled={searchingIp}
            className="px-3 py-1.5 text-xs rounded bg-indigo-600 hover:bg-indigo-500 disabled:opacity-50 whitespace-nowrap"
          >
            {searchingIp ? "..." : "查找"}
          </button>
        </div>
        {ipSearchMsg && (
          <p className={`text-[10px] mt-1 truncate ${ipSearchMessageClass}`}>{ipSearchMsg}</p>
        )}
      </div>

      {showSearch ? (
        <div className="flex-1 overflow-y-auto">
          <p className="px-4 py-2 text-xs text-gray-400 font-medium uppercase tracking-wider">搜索结果</p>
          {searching ? (
            <SidebarEmptyState title="正在搜索" detail="正在匹配联系人、群聊和聊天记录。" />
          ) : searchError ? (
            <SidebarEmptyState title="搜索失败" detail={searchError} tone="error" />
          ) : searchResults.length === 0 && groupSearchResults.length === 0 ? (
            <SidebarEmptyState title="没有匹配结果" detail={`未找到与“${searchQuery.trim()}”相关的聊天记录。`} />
          ) : (
            <>
              {searchResults.map((result) => (
                <div key={result.peer_id} className="mb-1">
                  <button
                    onClick={() => handleJumpToHit(result.peer_id)}
                    className="w-full text-left px-4 py-2 hover:bg-gray-800"
                  >
                    <p className="text-xs font-medium text-indigo-400 truncate">{result.peer_name}</p>
                  </button>
                  {result.messages.slice(0, 5).map((hit) => (
                    <button
                      key={hit.id}
                      onClick={() => handleJumpToHit(result.peer_id, hit.id)}
                      className="w-full text-left px-4 py-1.5 pl-6 hover:bg-gray-800"
                    >
                      <p className="text-xs text-gray-300 truncate">
                        {hit.msg_type === "file" ? `📎 ${hit.file_name || "文件"}` : hit.content.slice(0, 60)}
                      </p>
                      <p className="text-[10px] text-gray-500 mt-0.5">
                        {hit.sender_name} · {new Date(hit.timestamp).toLocaleString("zh-CN", { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" })}
                      </p>
                    </button>
                  ))}
                </div>
              ))}
              {groupSearchResults.map((result) => (
                <div key={result.group.group_id} className="mb-1">
                  <button
                    onClick={() => handleJumpToGroupHit(result.group.group_id)}
                    className="w-full text-left px-4 py-2 hover:bg-gray-800"
                  >
                    <p className="text-xs font-medium text-indigo-400 truncate">群聊 · {result.group.name}</p>
                  </button>
                  {result.messages.slice(0, 5).map((hit) => (
                    <button
                      key={hit.id}
                      onClick={() => handleJumpToGroupHit(result.group.group_id, hit.id)}
                      className="w-full text-left px-4 py-1.5 pl-6 hover:bg-gray-800"
                    >
                      <p className="text-xs text-gray-300 truncate">
                        {hit.msg_type === "file" ? `📎 ${hit.file_name || "文件"}` : hit.msg_type === "sticker" ? "[表情]" : hit.content.slice(0, 60)}
                      </p>
                      <p className="text-[10px] text-gray-500 mt-0.5">
                        {hit.sender_name} · {new Date(hit.timestamp).toLocaleString("zh-CN", { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" })}
                      </p>
                    </button>
                  ))}
                </div>
              ))}
            </>
          )}
        </div>
      ) : (
        <div className="flex-1 flex flex-col min-h-0">
          {/* Tabs */}
          <div className="flex border-b border-gray-700">
            <button onClick={() => { setTab("recent"); listRecentContacts().then(setRecentContacts).catch(() => {}); }} className={`flex-1 py-2 text-xs font-medium relative ${tab === "recent" ? "text-indigo-400 border-b-2 border-indigo-400" : "text-gray-500 hover:text-gray-300"}`}>
              最近
              {recentTotalUnread > 0 && (
                <span className="absolute top-1 right-2 min-w-[16px] h-4 px-1 rounded-full bg-red-500 text-white text-[10px] font-bold flex items-center justify-center">{recentTotalUnread > 99 ? "99+" : recentTotalUnread}</span>
              )}
            </button>
            <button onClick={() => setTab("contacts")} className={`flex-1 py-2 text-xs font-medium ${tab === "contacts" ? "text-indigo-400 border-b-2 border-indigo-400" : "text-gray-500 hover:text-gray-300"}`}>联系人</button>
            <button onClick={() => setTab("groups")} className={`flex-1 py-2 text-xs font-medium relative ${tab === "groups" ? "text-indigo-400 border-b-2 border-indigo-400" : "text-gray-500 hover:text-gray-300"}`}>
              群组
              {groupsTotalUnread > 0 && (
                <span className="absolute top-1 right-2 min-w-[16px] h-4 px-1 rounded-full bg-red-500 text-white text-[10px] font-bold flex items-center justify-center">{groupsTotalUnread > 99 ? "99+" : groupsTotalUnread}</span>
              )}
            </button>
          </div>

          {/* Tab content */}
          <div className="flex-1 overflow-y-auto">
            {tab === "recent" ? (
              recentContacts.length === 0 && recentGroups.length === 0 ? (
                <SidebarEmptyState
                  title="暂无最近会话"
                  detail="联系人或群聊产生消息后会显示在这里。"
                  actionLabel={peers.length > 0 ? "查看联系人" : groups.length > 0 ? "查看群组" : undefined}
                  onAction={peers.length > 0 ? () => setTab("contacts") : groups.length > 0 ? () => setTab("groups") : undefined}
                />
              ) : (
                <>
                  {recentGroups.length > 0 ? (
                    <div className="pb-1">
                      <p className="px-4 pt-3 pb-1 text-[10px] font-semibold uppercase tracking-wider text-gray-500">最近群聊</p>
                      {recentGroups.map((group) => (
                        <GroupItem
                          key={group.group_id}
                          group={group}
                          isSelected={selectedGroupId === group.group_id}
                          onSelect={() => onSelectGroup(group.group_id)}
                        />
                      ))}
                    </div>
                  ) : null}
                  {recentContacts.length > 0 ? (
                    <div className="pb-1">
                      <p className="px-4 pt-3 pb-1 text-[10px] font-semibold uppercase tracking-wider text-gray-500">最近联系人</p>
                      {recentContacts.map(r => {
                        const livePeer = peerById.get(r.peer_id);
                        const peer: Peer = livePeer ?? {
                          id: r.peer_id,
                          username: r.username,
                          department: r.department,
                          software_version: r.software_version ?? "",
                          mac_address: r.mac_address ?? "",
                          ip: r.ip,
                          port: r.port,
                          online: r.is_online,
                          last_seen: r.last_seen_at ? new Date(r.last_seen_at).getTime() / 1000 : undefined,
                        };
                        return (
                          <div key={r.peer_id} className="group relative">
                            <PeerItem peer={peer} isSelected={selectedPeerId === r.peer_id} unread={unreadMap.get(r.peer_id) ?? 0} onClick={() => onSelectPeer(peer)} onAvatarClick={() => setProfilePeer(peer)} />
                            <button onClick={(e) => { e.stopPropagation(); handleRemoveRecent(r.peer_id); }} className="absolute right-2 top-3 hidden group-hover:flex w-5 h-5 rounded-full bg-gray-600 hover:bg-red-600 items-center justify-center text-[10px]" title="移除">×</button>
                          </div>
                        );
                      })}
                    </div>
                  ) : null}
                </>
              )
            ) : tab === "groups" ? (
              <>
                <div className="px-4 py-2">
                  <button onClick={() => setShowCreateGroup(true)} className="w-full py-1.5 text-xs rounded bg-indigo-600 hover:bg-indigo-500">+ 创建群组</button>
                </div>
                {groups.length === 0 ? (
                  <SidebarEmptyState
                    title="暂未加入群组"
                    detail="创建一个群组，把常联系的人放在一起。"
                    actionLabel="创建群组"
                    onAction={() => setShowCreateGroup(true)}
                  />
                ) : (
                  groups.map((g) => (
                    <GroupItem
                      key={g.group_id}
                      group={g}
                      isSelected={selectedGroupId === g.group_id}
                      onSelect={() => onSelectGroup(g.group_id)}
                    />
                  ))
                )}
              </>
            ) : (
              <>
                <div className="sticky top-0 z-10 border-b border-gray-700 bg-gray-900 px-4 py-2">
                  <div className="relative">
                    <svg className="w-3.5 h-3.5 absolute left-2.5 top-1/2 -translate-y-1/2 text-gray-500" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
                    </svg>
                    <input
                      type="text"
                      value={contactQuery}
                      onChange={(e) => setContactQuery(e.target.value)}
                      onKeyDown={(e) => { if (e.key === "Escape") setContactQuery(""); }}
                      placeholder="搜索联系人..."
                      className="w-full bg-gray-800 text-xs text-gray-200 rounded-lg pl-8 pr-8 py-2 outline-none focus:ring-1 focus:ring-indigo-500 placeholder-gray-500"
                    />
                    {contactQuery ? (
                      <button
                        type="button"
                        onClick={() => setContactQuery("")}
                        className="absolute right-2 top-1/2 -translate-y-1/2 w-5 h-5 rounded-full text-gray-500 hover:text-gray-300 hover:bg-gray-700 flex items-center justify-center"
                        aria-label="清空联系人搜索"
                      >
                        ×
                      </button>
                    ) : null}
                  </div>
                </div>
                {sortedDepts.map(dept => {
                  const expanded = contactFilter ? true : expandedDepts.has(dept);
                  const deptPeers = deptGroups.get(dept) || [];
                  const onlineCount = deptPeers.filter(p => p.online).length;
                  return (
                    <div key={dept}>
                      <button
                        onClick={() => { if (!contactFilter) toggleDept(dept); }}
                        className="w-full flex items-center gap-2 px-4 py-2 text-xs text-gray-400 font-medium hover:bg-gray-800 transition-colors"
                      >
                        <svg
                          className={`w-3 h-3 text-gray-500 transition-transform flex-shrink-0 ${expanded ? "rotate-90" : ""}`}
                          fill="none" viewBox="0 0 24 24" stroke="currentColor"
                        >
                          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9 5l7 7-7 7" />
                        </svg>
                        <span className="uppercase tracking-wider truncate">{dept}</span>
                        <span className="text-gray-500 normal-case text-[10px] flex-shrink-0">
                          {onlineCount}/{deptPeers.length}
                        </span>
                      </button>
                      {expanded && deptPeers.map(peer => (
                        <PeerItem key={peer.id} peer={peer} isSelected={selectedPeerId === peer.id} unread={unreadMap.get(peer.id) ?? 0} onClick={() => onSelectPeer(peer)} onAvatarClick={() => setProfilePeer(peer)} />
                      ))}
                    </div>
                  );
                })}
                {peers.length === 0 && (
                  <SidebarEmptyState title="暂无联系人" detail="同一局域网内的 Echo 用户会显示在这里。" />
                )}
                {peers.length > 0 && visiblePeers.length === 0 && (
                  <SidebarEmptyState title="没有匹配联系人" detail={`未找到与“${contactQuery.trim()}”匹配的联系人。`} />
                )}
              </>
            )}
          </div>
        </div>
      )}

      {profilePeer ? (
        <PeerProfileCard
          peer={profilePeer}
          copied={copied}
          refreshing={refreshingProfileId === profilePeer.id}
          onCopy={copyToClipboard}
          onClose={() => setProfilePeer(null)}
        />
      ) : null}

      {/* Create group dialog */}
      {showCreateGroup && (
        <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/50">
          <div className="bg-gray-800 border border-gray-600 rounded-xl p-5 w-96 shadow-2xl">
            <h3 className="text-base font-semibold mb-4">创建群组</h3>

            <div className="mb-4">
              <label className="block text-xs text-gray-400 mb-1.5">
                群组名称 <span className="text-red-400">*</span>
              </label>
              <input
                value={newGroupName}
                onChange={(e) => {
                  setNewGroupName(e.target.value);
                  if (groupNameError) setGroupNameError("");
                }}
                placeholder="请输入群组名称"
                maxLength={50}
                className={`w-full bg-gray-900 border ${groupNameError ? "border-red-500" : "border-gray-600"} rounded px-3 py-2 text-sm text-gray-200 outline-none focus:border-indigo-500 transition-colors`}
                autoFocus
              />
              {groupNameError && (
                <p className="text-xs text-red-400 mt-1">{groupNameError}</p>
              )}
              <p className="text-xs text-gray-500 mt-1">{newGroupName.length}/50</p>
            </div>

            <div className="mb-4">
              <label className="block text-xs text-gray-400 mb-1.5">
                选择成员 <span className="text-red-400">*</span>
                <span className="text-gray-500 ml-1">({newGroupMembers.length} 人)</span>
              </label>
              <div className="relative mb-2">
                <svg className="w-3.5 h-3.5 absolute left-2.5 top-1/2 -translate-y-1/2 text-gray-500" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
                </svg>
                <input
                  value={newGroupMemberQuery}
                  onChange={(e) => setNewGroupMemberQuery(e.target.value)}
                  placeholder="搜索成员..."
                  className="w-full bg-gray-900 border border-gray-600 rounded px-8 py-1.5 text-xs text-gray-200 outline-none focus:border-indigo-500"
                />
                {newGroupMemberQuery ? (
                  <button
                    type="button"
                    onClick={() => setNewGroupMemberQuery("")}
                    className="absolute right-2 top-1/2 -translate-y-1/2 w-5 h-5 rounded-full text-gray-500 hover:text-gray-300 hover:bg-gray-700 flex items-center justify-center"
                    aria-label="清空成员搜索"
                  >
                    ×
                  </button>
                ) : null}
              </div>
              <div className="max-h-40 overflow-y-auto bg-gray-900 border border-gray-600 rounded p-2">
                {peers.length === 0 ? (
                  <p className="text-xs text-gray-500 text-center py-4">暂无可选成员</p>
                ) : selectableGroupPeers.length === 0 ? (
                  <p className="text-xs text-gray-500 text-center py-4">无匹配成员</p>
                ) : (
                  selectableGroupPeers.map((p) => (
                    <label key={p.id} className="flex items-center gap-2 py-1.5 px-2 hover:bg-gray-800 rounded cursor-pointer transition-colors">
                      <input
                        type="checkbox"
                        checked={newGroupMembers.includes(p.id)}
                        onChange={() => {
                          setNewGroupMembers((prev) => prev.includes(p.id) ? prev.filter((id) => id !== p.id) : [...prev, p.id]);
                          if (groupNameError) setGroupNameError("");
                        }}
                        className="w-4 h-4 accent-indigo-600"
                      />
                      <div className="flex-1 min-w-0">
                        <span className="text-xs text-gray-300">{p.username}</span>
                        {p.department && <span className="text-xs text-gray-500 ml-1">({p.department})</span>}
                      </div>
                      <span className={`text-[10px] ${p.online ? "text-green-400" : "text-gray-500"}`}>
                        {p.online ? "在线" : "离线"}
                      </span>
                    </label>
                  ))
                )}
              </div>
            </div>

            <div className="flex gap-2">
              <button
                onClick={handleCreateGroup}
                disabled={!newGroupName.trim() || newGroupMembers.length === 0}
                className="create-group-submit flex-1 py-2.5 text-sm font-medium rounded border border-transparent bg-indigo-600 hover:bg-indigo-500 disabled:cursor-not-allowed transition-colors"
              >
                创建群组
              </button>
              <button
                onClick={() => {
                  setShowCreateGroup(false);
                  setNewGroupName("");
                  setNewGroupMembers([]);
                  setNewGroupMemberQuery("");
                  setGroupNameError("");
                }}
                className="flex-1 py-2.5 text-sm font-medium rounded bg-gray-700 hover:bg-gray-600 transition-colors"
              >
                取消
              </button>
            </div>
          </div>
        </div>
      )}

      <div className="relative p-3 border-t border-gray-700 text-xs text-gray-500 text-center">
        Echo P2P Chat · 局域网通信
        <div ref={themeMenuRef} className="absolute right-2 top-1/2 -translate-y-1/2">
          <button
            type="button"
            onClick={() => setShowThemeMenu((value) => !value)}
            className="w-7 h-7 rounded bg-gray-700 hover:bg-gray-600 transition-colors flex items-center justify-center"
            title="切换皮肤"
            aria-label="切换皮肤"
          >
            <span className="flex items-center gap-0.5">
              {currentTheme.preview.map((color) => (
                <span key={color} className="w-1.5 h-3 rounded-sm" style={{ background: color }} />
              ))}
            </span>
          </button>
          {showThemeMenu && (
            <div className="absolute bottom-full right-0 mb-2 z-50 w-44 rounded-lg bg-gray-800 border border-gray-600 p-2 shadow-2xl text-left">
              {THEMES.map((theme) => (
                <button
                  key={theme.id}
                  type="button"
                  onClick={() => {
                    onThemeChange(theme.id);
                    setShowThemeMenu(false);
                  }}
                  className={`w-full flex items-center gap-2 px-2 py-1.5 rounded text-xs hover:bg-gray-700 ${themeId === theme.id ? "text-indigo-400" : "text-gray-300"}`}
                >
                  <span className={`theme-swatch ${themeId === theme.id ? "theme-swatch-active" : ""}`}>
                    <span style={{ background: theme.preview[0] }} />
                    <span style={{ background: theme.preview[1] }} />
                    <span style={{ background: theme.preview[2] }} />
                  </span>
                  <span>{theme.name}</span>
                </button>
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function SidebarEmptyState({
  title,
  detail,
  tone = "muted",
  actionLabel,
  onAction,
}: {
  title: string;
  detail: string;
  tone?: "muted" | "error";
  actionLabel?: string;
  onAction?: () => void;
}) {
  const titleClass = tone === "error" ? "text-red-300" : "text-gray-300";
  const iconClass = tone === "error" ? "text-red-400 bg-red-500/10 border-red-500/30" : "text-gray-500 bg-gray-800/70 border-gray-700";

  return (
    <div className="px-5 py-10 text-center">
      <div className={`mx-auto mb-3 flex h-10 w-10 items-center justify-center rounded-lg border ${iconClass}`}>
        {tone === "error" ? (
          <svg className="h-5 w-5" fill="none" viewBox="0 0 24 24" stroke="currentColor">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 9v4m0 4h.01M10.29 3.86L1.82 18a2 2 0 001.71 3h16.94a2 2 0 001.71-3L13.71 3.86a2 2 0 00-3.42 0z" />
          </svg>
        ) : (
          <svg className="h-5 w-5" fill="none" viewBox="0 0 24 24" stroke="currentColor">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M8 10h8M8 14h5M7 20l-4 2V5a3 3 0 013-3h12a3 3 0 013 3v10a3 3 0 01-3 3H7v2z" />
          </svg>
        )}
      </div>
      <p className={`text-sm font-medium ${titleClass}`}>{title}</p>
      <p className="mt-1 text-xs leading-relaxed text-gray-500">{detail}</p>
      {actionLabel && onAction ? (
        <button
          type="button"
          onClick={onAction}
          className="mt-4 rounded-lg bg-gray-700 px-3 py-1.5 text-xs text-gray-200 hover:bg-gray-600"
        >
          {actionLabel}
        </button>
      ) : null}
    </div>
  );
}

function PeerItem({ peer, isSelected, unread, onClick, onAvatarClick }: { peer: Peer; isSelected: boolean; unread: number; onClick: () => void; onAvatarClick: () => void }) {
  return (
    <div
      className={`sidebar-list-item w-full flex items-center gap-3 px-4 py-3 transition-colors ${
        isSelected ? "sidebar-list-item-active bg-indigo-600/30 border-l-2 border-indigo-400" : "hover:bg-gray-800 border-l-2 border-transparent"
      }`}
    >
      <button
        type="button"
        onClick={(event) => {
          event.stopPropagation();
          onAvatarClick();
        }}
        className="relative flex-shrink-0 rounded-full focus:outline-none focus:ring-2 focus:ring-indigo-400"
        title="查看个人信息"
        aria-label={`查看${peer.username}的个人信息`}
      >
        <div className="w-9 h-9 rounded-full bg-gray-600 flex items-center justify-center text-sm font-medium">
          {peer.username.charAt(0).toUpperCase()}
        </div>
        <div className={`absolute -bottom-0.5 -right-0.5 w-3 h-3 rounded-full border-2 border-gray-900 ${peer.online ? "bg-green-400" : "bg-gray-500"}`} />
      </button>
      <button
        type="button"
        onClick={onClick}
        className="flex-1 min-w-0 text-left"
      >
        <p className="text-sm font-medium truncate">{peer.username}</p>
        <p className="text-xs text-gray-400 truncate">{peer.department}</p>
        <p className="text-[10px] text-gray-500 truncate">{peer.online ? `${peer.ip}:${peer.port}` : "离线"}</p>
      </button>
      {unread > 0 && !isSelected && (
        <div className="flex-shrink-0 w-5 h-5 rounded-full bg-red-500 flex items-center justify-center text-[10px] font-bold">
          {unread > 99 ? "99+" : unread}
        </div>
      )}
    </div>
  );
}

function formatPeerLastSeen(peer: Peer) {
  if (peer.online) return "在线";
  if (!peer.last_seen) return "未知";
  return new Date(peer.last_seen * 1000).toLocaleString("zh-CN", {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function PeerProfileCard({ peer, copied, refreshing, onCopy, onClose }: { peer: Peer; copied: string; refreshing: boolean; onCopy: (text: string, label: string) => void; onClose: () => void }) {
  const endpoint = peer.ip && peer.port ? `${peer.ip}:${peer.port}` : "";
  const rows = [
    { label: "Peer ID", value: peer.id, copyLabel: "好友Peer ID", mono: true },
    { label: "用户名", value: peer.username },
    { label: "部门", value: peer.department || "未分组" },
    { label: "软件版本", value: peer.software_version || (refreshing ? "获取中..." : "未知"), mono: true },
    { label: "MAC地址", value: peer.mac_address || (refreshing ? "获取中..." : "未知"), copyLabel: peer.mac_address ? "好友MAC地址" : undefined, mono: true },
    { label: "IP:端口", value: endpoint || "未知", copyLabel: endpoint ? "好友IP:端口" : undefined, mono: true },
    { label: "状态", value: formatPeerLastSeen(peer) },
  ];

  return (
    <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/50 px-4">
      <div className="w-full rounded-xl border border-gray-600 bg-gray-800 p-4 shadow-2xl">
        <div className="mb-4 flex items-center gap-3">
          <div className="relative flex-shrink-0">
            <div className="w-12 h-12 rounded-full bg-gray-600 flex items-center justify-center text-lg font-semibold">
              {peer.username.charAt(0).toUpperCase()}
            </div>
            <div className={`absolute -bottom-0.5 -right-0.5 w-3.5 h-3.5 rounded-full border-2 border-gray-800 ${peer.online ? "bg-green-400" : "bg-gray-500"}`} />
          </div>
          <div className="min-w-0 flex-1">
            <p className="truncate text-sm font-semibold text-gray-100">{peer.username}</p>
            <p className="truncate text-xs text-gray-400">{peer.department || "未分组"}</p>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="flex-shrink-0 w-7 h-7 rounded bg-gray-700 hover:bg-gray-600 text-gray-300 flex items-center justify-center"
            aria-label="关闭"
          >
            ×
          </button>
        </div>
        <div className="space-y-2 text-xs">
          {rows.map((row) => (
            <div key={row.label} className="flex items-center gap-2">
              <span className="w-16 flex-shrink-0 text-gray-400">{row.label}</span>
              <span
                className={`min-w-0 flex-1 truncate text-gray-200 ${row.mono ? "font-mono text-[10px]" : ""}`}
                title={row.value}
              >
                {row.value}
              </span>
              {row.copyLabel && row.value !== "未知" ? (
                <button
                  type="button"
                  onClick={() => onCopy(row.value, row.copyLabel!)}
                  className="flex-shrink-0 w-6 h-6 rounded hover:bg-white/10 flex items-center justify-center"
                  title={`复制${row.label}`}
                >
                  {copied === row.copyLabel ? (
                    <span className="text-[10px] text-green-400">✓</span>
                  ) : (
                    <svg className="w-3.5 h-3.5 text-gray-500" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z" /></svg>
                  )}
                </button>
              ) : (
                <div className="w-6 h-6 flex-shrink-0" />
              )}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

function GroupItem({ group, isSelected, onSelect }: { group: GroupInfo; isSelected: boolean; onSelect: () => void }) {
  const unread = group.unread_count || 0;
  const preview = group.last_message || "暂无消息";
  const sender = group.last_message_sender;
  const time = group.last_message_at ? new Date(group.last_message_at).toLocaleString("zh-CN", { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" }) : "";
  return (
    <div className={`sidebar-list-item flex items-stretch ${isSelected ? "sidebar-list-item-active bg-indigo-600/20 border-l-2 border-indigo-400" : "border-l-2 border-transparent hover:bg-gray-800"}`}>
      <button onClick={onSelect} className="flex-1 min-w-0 text-left px-4 py-3 flex items-center gap-3">
        <div className="relative flex-shrink-0">
          <div className="w-9 h-9 rounded-full bg-indigo-700 flex items-center justify-center text-base">👥</div>
        </div>
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2">
            <p className="text-sm font-medium truncate flex-1">{group.name}</p>
            <span className="text-[10px] text-gray-500 flex-shrink-0">{time}</span>
          </div>
          <p className="text-xs text-gray-400 truncate">
            {sender ? <span className="text-gray-500">{sender}: </span> : null}
            {preview}
          </p>
          <p className="text-[10px] text-gray-500 truncate">{group.members?.length || 0} 人</p>
        </div>
        {unread > 0 && !isSelected && (
          <div className="flex-shrink-0 w-5 h-5 rounded-full bg-red-500 flex items-center justify-center text-[10px] font-bold text-white">
            {unread > 99 ? "99+" : unread}
          </div>
        )}
      </button>
    </div>
  );
}
