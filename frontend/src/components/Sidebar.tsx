import { useState, useCallback, useEffect } from "react";
import type { Peer, UnreadCount, StoredPeer } from "../types";
import { searchMessages, discoverByIp, listRecentContacts, removeRecentContact, listGroups, createGroup, renameGroup, leaveGroup, inviteToGroup, dissolveGroup } from "../api";
import type { GroupInfo } from "../api";
import type { SearchResult } from "../api";

interface SidebarProps {
  peers: Peer[];
  selectedPeerId: string | null;
  onSelectPeer: (peer: Peer) => void;
  onJumpToSearchHit: (peerId: string) => void;
  myId: string;
  myName: string;
  myDepartment: string;
  myIp: string;
  myPort: number;
  onEditProfile: () => void;
  unreadCounts: UnreadCount[];
  scanSubnets: string[];
  onSaveScanSubnets: (subnets: string[]) => Promise<void>;
  selectedGroupId: string | null;
  onSelectGroup: (groupId: string) => void;
}

export function Sidebar({ peers, selectedPeerId, onSelectPeer, myId, myName, myDepartment, myIp, myPort, onEditProfile, unreadCounts, scanSubnets, onSaveScanSubnets, onJumpToSearchHit, selectedGroupId, onSelectGroup }: SidebarProps) {
  const [searchQuery, setSearchQuery] = useState("");
  const [searchResults, setSearchResults] = useState<SearchResult[]>([]);
  const [searching, setSearching] = useState(false);
  const [showSearch, setShowSearch] = useState(false);
  const [showProfile, setShowProfile] = useState(false);
  const [copied, setCopied] = useState("");
  const [subnetInput, setSubnetInput] = useState(scanSubnets.join(", "));
  const [savingSubnets, setSavingSubnets] = useState(false);
  const [tab, setTab] = useState<"recent" | "contacts" | "groups">("recent");
  const [recentContacts, setRecentContacts] = useState<StoredPeer[]>([]);
  const [groups, setGroups] = useState<GroupInfo[]>([]);
  const [showCreateGroup, setShowCreateGroup] = useState(false);
  const [newGroupName, setNewGroupName] = useState("");
  const [newGroupMembers, setNewGroupMembers] = useState<string[]>([]);
  const [infoGroup, setInfoGroup] = useState<GroupInfo | null>(null);

  useEffect(() => { listRecentContacts().then(setRecentContacts).catch(() => {}); }, [peers, tab]);
  useEffect(() => { listGroups().then(setGroups).catch(() => {}); }, [peers]);

  const handleRemoveRecent = useCallback(async (peerId: string) => {
    await removeRecentContact(peerId).catch(() => {});
    setRecentContacts((prev) => prev.filter((r) => r.peer_id !== peerId));
  }, []);

  const handleCreateGroup = useCallback(async () => {
    if (!newGroupName.trim()) return;
    try {
      await createGroup(newGroupName.trim(), newGroupMembers);
      setShowCreateGroup(false);
      setNewGroupName("");
      setNewGroupMembers([]);
      listGroups().then(setGroups).catch(() => {});
    } catch (e) {
      console.error("Failed to create group:", e);
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

  // Group peers by department for "contacts" tab
  const deptGroups = new Map<string, Peer[]>();
  for (const p of peers) {
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

  const handleSearchChange = useCallback(async (value: string) => {
    setSearchQuery(value);
    if (!value.trim()) {
      setSearchResults([]);
      setShowSearch(false);
      return;
    }
    setSearching(true);
    setShowSearch(true);
    try {
      const results = await searchMessages(value.trim());
      setSearchResults(results);
    } catch (e) {
      console.error(e);
    } finally {
      setSearching(false);
    }
  }, []);

  const handleJumpToHit = useCallback((peerId: string) => {
    setShowSearch(false);
    setSearchQuery("");
    setSearchResults([]);
    onJumpToSearchHit(peerId);
  }, [onJumpToSearchHit]);

  const [manualIp, setManualIp] = useState("");
  const [manualPort, setManualPort] = useState("9527");
  const [searchingIp, setSearchingIp] = useState(false);
  const [ipSearchMsg, setIpSearchMsg] = useState("");

  const handleDiscoverIp = useCallback(async () => {
    const ip = manualIp.trim();
    if (!ip) return;
    setSearchingIp(true);
    setIpSearchMsg("");
    try {
      const result = await discoverByIp(ip, parseInt(manualPort) || 9527);
      setIpSearchMsg(result.message);
      if (!result.online) {
        // Not found — still allow adding
      }
    } catch (e) {
      setIpSearchMsg(String(e));
    } finally {
      setSearchingIp(false);
    }
  }, [manualIp, manualPort]);

  return (
    <div className="flex flex-col w-72 bg-gray-900 text-white h-full border-r border-gray-700">
      <div className="p-4 border-b border-gray-700 relative">
        <div className="flex items-center gap-3">
          <button
            onClick={() => setShowProfile(!showProfile)}
            className="relative flex-shrink-0 w-10 h-10 rounded-full bg-indigo-500 hover:bg-indigo-400 transition-colors flex items-center justify-center text-lg font-bold cursor-pointer"
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
              <div className="w-12 h-12 rounded-full bg-indigo-500 flex items-center justify-center text-xl font-bold">
                {myName.charAt(0).toUpperCase()}
              </div>
              <div>
                <p className="text-sm font-semibold">{myName}</p>
                <p className="text-xs text-gray-400">{myDepartment}</p>
              </div>
            </div>
            <div className="space-y-1.5 text-xs">
              <div className="flex items-center gap-1">
                <span className="text-gray-400 w-14 flex-shrink-0">Peer ID</span>
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
                <span className="text-gray-400 w-14 flex-shrink-0">IP</span>
                <span className="text-gray-200 font-mono flex-1">{myIp}</span>
                <button onClick={() => copyToClipboard(`${myIp}:${myPort}`, "IP:端口")} className="flex-shrink-0 w-5 h-5 rounded hover:bg-white/10 flex items-center justify-center">
                  {copied === "IP:端口" ? (
                    <span className="text-[10px] text-green-400">✓</span>
                  ) : (
                    <svg className="w-3 h-3 text-gray-500" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z" /></svg>
                  )}
                </button>
              </div>
              <div className="flex items-center gap-1">
                <span className="text-gray-400 w-14 flex-shrink-0">端口</span>
                <span className="text-gray-200 font-mono flex-1">{myPort}</span>
                <div className="w-5 h-5" />
              </div>
              <div className="flex items-center gap-1">
                <span className="text-gray-400 w-14 flex-shrink-0">用户名</span>
                <span className="text-gray-200 flex-1">{myName}</span>
                <div className="w-5 h-5" />
              </div>
              <div className="flex items-center gap-1">
                <span className="text-gray-400 w-14 flex-shrink-0">部门</span>
                <span className="text-gray-200 flex-1">{myDepartment}</span>
                <div className="w-5 h-5" />
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
            placeholder="搜索聊天记录..."
            className="w-full bg-gray-800 text-sm text-gray-200 rounded-lg pl-8 pr-3 py-2 outline-none focus:ring-2 focus:ring-indigo-500 placeholder-gray-500"
          />
        </div>
      </div>

      {/* Manual IP search */}
      <div className="px-4 pb-2">
        <div className="flex gap-1">
          <input
            type="text"
            value={manualIp}
            onChange={(e) => setManualIp(e.target.value)}
            placeholder="IP 地址"
            className="flex-1 bg-gray-800 text-xs text-gray-200 rounded px-2 py-1.5 outline-none focus:ring-1 focus:ring-indigo-500 placeholder-gray-500"
            onKeyDown={(e) => { if (e.key === "Enter") handleDiscoverIp(); }}
          />
          <input
            type="text"
            value={manualPort}
            onChange={(e) => setManualPort(e.target.value)}
            placeholder="9527"
            className="w-14 bg-gray-800 text-xs text-gray-200 rounded px-2 py-1.5 outline-none focus:ring-1 focus:ring-indigo-500 placeholder-gray-500"
            onKeyDown={(e) => { if (e.key === "Enter") handleDiscoverIp(); }}
          />
          <button
            onClick={handleDiscoverIp}
            disabled={searchingIp}
            className="px-2 py-1.5 text-xs rounded bg-indigo-600 hover:bg-indigo-500 disabled:opacity-50 whitespace-nowrap"
          >
            {searchingIp ? "..." : "查找"}
          </button>
        </div>
        {ipSearchMsg && (
          <p className="text-[10px] text-gray-400 mt-1 truncate">{ipSearchMsg}</p>
        )}
      </div>

      {showSearch ? (
        <div className="flex-1 overflow-y-auto">
          <p className="px-4 py-2 text-xs text-gray-400 font-medium uppercase tracking-wider">搜索结果</p>
          {searching ? (
            <p className="px-4 py-3 text-xs text-gray-500">搜索中...</p>
          ) : searchResults.length === 0 ? (
            <p className="px-4 py-3 text-xs text-gray-500">无匹配结果</p>
          ) : (
            searchResults.map((result) => (
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
                    onClick={() => handleJumpToHit(result.peer_id)}
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
            ))
          )}
        </div>
      ) : (
        <div className="flex-1 flex flex-col min-h-0">
          {/* Tabs */}
          <div className="flex border-b border-gray-700">
            <button onClick={() => { setTab("recent"); listRecentContacts().then(setRecentContacts).catch(() => {}); }} className={`flex-1 py-2 text-xs font-medium ${tab === "recent" ? "text-indigo-400 border-b-2 border-indigo-400" : "text-gray-500 hover:text-gray-300"}`}>最近</button>
            <button onClick={() => setTab("contacts")} className={`flex-1 py-2 text-xs font-medium ${tab === "contacts" ? "text-indigo-400 border-b-2 border-indigo-400" : "text-gray-500 hover:text-gray-300"}`}>联系人</button>
            <button onClick={() => { setTab("groups"); listGroups().then(setGroups).catch(() => {}); }} className={`flex-1 py-2 text-xs font-medium ${tab === "groups" ? "text-indigo-400 border-b-2 border-indigo-400" : "text-gray-500 hover:text-gray-300"}`}>群组</button>
          </div>

          {/* Tab content */}
          <div className="flex-1 overflow-y-auto">
            {tab === "recent" ? (
              recentContacts.length === 0 ? (
                <p className="px-4 py-8 text-xs text-gray-500 text-center">暂无最近联系人</p>
              ) : (
                recentContacts.map(r => {
                  const peer: Peer = { id: r.peer_id, username: r.username, department: r.department, ip: r.ip, port: r.port, online: r.is_online, last_seen: r.last_seen_at ? new Date(r.last_seen_at).getTime() / 1000 : undefined };
                  return (
                    <div key={r.peer_id} className="group relative">
                      <PeerItem peer={peer} isSelected={selectedPeerId === r.peer_id} unread={unreadMap.get(r.peer_id) ?? 0} onClick={() => onSelectPeer(peer)} />
                      <button onClick={(e) => { e.stopPropagation(); handleRemoveRecent(r.peer_id); }} className="absolute right-2 top-3 hidden group-hover:flex w-5 h-5 rounded-full bg-gray-600 hover:bg-red-600 items-center justify-center text-[10px]" title="移除">×</button>
                    </div>
                  );
                })
              )
            ) : tab === "groups" ? (
              <>
                <div className="px-4 py-2">
                  <button onClick={() => setShowCreateGroup(true)} className="w-full py-1.5 text-xs rounded bg-indigo-600 hover:bg-indigo-500">+ 创建群组</button>
                </div>
                {groups.length === 0 ? (
                  <p className="px-4 py-8 text-xs text-gray-500 text-center">暂未加入群组</p>
                ) : (
                  groups.map((g) => (
                    <div key={g.group_id} className="flex items-center">
                      <button onClick={() => { onSelectGroup(g.group_id); }}
                        className={`flex-1 text-left px-4 py-2 hover:bg-gray-800 flex items-center gap-2 ${selectedGroupId === g.group_id ? "bg-indigo-600/20 border-l-2 border-indigo-400" : "border-l-2 border-transparent"}`}>
                        <span className="text-sm">👥</span>
                        <span className="text-sm text-gray-200 truncate">{g.name}</span>
                        <span className="text-[10px] text-gray-500 ml-auto">{g.members?.length || 0}人</span>
                      </button>
                      <button onClick={() => { listGroups().then(setGroups).catch(() => {}); setInfoGroup(g); }}
                        className="px-2 py-2 text-xs text-gray-500 hover:text-gray-300" title="群信息">ℹ️</button>
                    </div>
                  ))
                )}
              </>
            ) : (
              <>
                {sortedDepts.map(dept => (
                  <div key={dept}>
                    <p className="px-4 py-2 text-xs text-gray-400 font-medium uppercase tracking-wider">{dept} — {(deptGroups.get(dept) || []).length}</p>
                    {(deptGroups.get(dept) || []).map(peer => (
                      <PeerItem key={peer.id} peer={peer} isSelected={selectedPeerId === peer.id} unread={unreadMap.get(peer.id) ?? 0} onClick={() => onSelectPeer(peer)} />
                    ))}
                  </div>
                ))}
                {peers.length === 0 && <p className="px-4 py-8 text-xs text-gray-500 text-center">暂无联系人</p>}
              </>
            )}
          </div>
        </div>
      )}

      {/* Group info dialog */}
      {infoGroup && (
        <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/50" onClick={() => setInfoGroup(null)}>
          <div className="bg-gray-800 border border-gray-600 rounded-xl p-4 w-80 shadow-2xl" onClick={(e) => e.stopPropagation()}>
            <div className="flex items-center justify-between mb-3">
              <p className="text-sm font-semibold">群组信息</p>
              <button onClick={() => setInfoGroup(null)} className="text-gray-500 hover:text-gray-300">✕</button>
            </div>
            <div className="space-y-2 text-xs">
              <div className="flex items-center gap-2">
                <span className="text-gray-400">群名:</span>
                <input defaultValue={infoGroup.name} onBlur={async (e) => {
                  if (e.target.value.trim() && e.target.value !== infoGroup.name) {
                    await renameGroup(infoGroup.group_id, e.target.value.trim());
                    listGroups().then(setGroups).catch(() => {});
                  }
                }} className="flex-1 bg-gray-900 border border-gray-600 rounded px-2 py-1 text-gray-200 outline-none" />
              </div>
              <p className="text-gray-400">成员 ({infoGroup.members?.length || 0}人):</p>
              <div className="max-h-24 overflow-y-auto space-y-1">
                {infoGroup.members?.map((m) => (
                  <div key={m.peer_id} className="flex items-center gap-2 text-gray-300">
                    <span className={`w-2 h-2 rounded-full ${m.is_online ? "bg-green-400" : "bg-gray-500"}`} />
                    <span className="truncate">{m.username || m.peer_id}</span>
                  </div>
                ))}
              </div>
              <div className="flex gap-1 pt-2">
                <select multiple={false} onChange={async (e) => {
                  const pid = e.target.value;
                  if (pid && !infoGroup.members?.some((m) => m.peer_id === pid)) {
                    await inviteToGroup(infoGroup.group_id, [pid]);
                    listGroups().then(setGroups).catch(() => {});
                    setInfoGroup((prev) => prev ? {...prev, members: [...(prev.members||[]), {peer_id: pid, username: peers.find(p=>p.id===pid)?.username||pid, department:"", ip:"", port:0, is_online:false, first_seen_at:"", last_seen_at:""}]} : null);
                  }
                }} className="flex-1 bg-gray-900 border border-gray-600 rounded px-2 py-1 text-xs text-gray-200 outline-none">
                  <option value="">+ 邀请成员</option>
                  {peers.filter((p) => !infoGroup.members?.some((m) => m.peer_id === p.id)).map((p) => (
                    <option key={p.id} value={p.id}>{p.username}</option>
                  ))}
                </select>
              </div>
              <div className="flex gap-2 pt-2">
                <button onClick={async () => { await leaveGroup(infoGroup.group_id); setInfoGroup(null); listGroups().then(setGroups).catch(() => {}); }}
                  className="flex-1 py-1.5 text-xs rounded bg-yellow-700 hover:bg-yellow-600">退群</button>
                {infoGroup.creator_id === myId && (
                  <button onClick={async () => {
                    await dissolveGroup(infoGroup.group_id);
                    if (selectedGroupId === infoGroup.group_id) onSelectGroup("");
                    setInfoGroup(null);
                    listGroups().then(setGroups).catch(() => {});
                  }}
                    className="flex-1 py-1.5 text-xs rounded bg-red-700 hover:bg-red-600">解散群</button>
                )}
              </div>
            </div>
          </div>
        </div>
      )}

      {/* Create group dialog */}
      {showCreateGroup && (
        <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/50">
          <div className="bg-gray-800 border border-gray-600 rounded-xl p-4 w-80 shadow-2xl">
            <p className="text-sm font-semibold mb-3">创建群组</p>
            <input value={newGroupName} onChange={(e) => setNewGroupName(e.target.value)} placeholder="群组名称" className="w-full bg-gray-900 border border-gray-600 rounded px-3 py-2 text-sm text-gray-200 outline-none mb-3" />
            <p className="text-xs text-gray-400 mb-2">选择成员：</p>
            <div className="max-h-32 overflow-y-auto mb-3">
              {peers.map((p) => (
                <label key={p.id} className="flex items-center gap-2 py-1 cursor-pointer">
                  <input type="checkbox" checked={newGroupMembers.includes(p.id)} onChange={() => {
                    setNewGroupMembers((prev) => prev.includes(p.id) ? prev.filter((id) => id !== p.id) : [...prev, p.id]);
                  }} />
                  <span className="text-xs text-gray-300">{p.username}{p.department ? ` (${p.department})` : ""}</span>
                </label>
              ))}
            </div>
            <div className="flex gap-2">
              <button onClick={handleCreateGroup} className="flex-1 py-2 text-xs rounded bg-indigo-600 hover:bg-indigo-500">创建</button>
              <button onClick={() => setShowCreateGroup(false)} className="flex-1 py-2 text-xs rounded bg-gray-700 hover:bg-gray-600">取消</button>
            </div>
          </div>
        </div>
      )}

      <div className="p-3 border-t border-gray-700 text-xs text-gray-500 text-center">Echo P2P Chat · 局域网通信</div>
    </div>
  );
}

function PeerItem({ peer, isSelected, unread, onClick }: { peer: Peer; isSelected: boolean; unread: number; onClick: () => void }) {
  return (
    <button
      onClick={onClick}
      className={`w-full flex items-center gap-3 px-4 py-3 transition-colors ${
        isSelected ? "bg-indigo-600/30 border-l-2 border-indigo-400" : "hover:bg-gray-800 border-l-2 border-transparent"
      }`}
    >
      <div className="relative">
        <div className="w-9 h-9 rounded-full bg-gray-600 flex items-center justify-center text-sm font-medium">
          {peer.username.charAt(0).toUpperCase()}
        </div>
        <div className={`absolute -bottom-0.5 -right-0.5 w-3 h-3 rounded-full border-2 border-gray-900 ${peer.online ? "bg-green-400" : "bg-gray-500"}`} />
      </div>
      <div className="flex-1 min-w-0 text-left">
        <p className="text-sm font-medium truncate">{peer.username}</p>
        <p className="text-xs text-gray-400 truncate">{peer.department}</p>
        <p className="text-[10px] text-gray-500 truncate">{peer.online ? `${peer.ip}:${peer.port}` : "离线"}</p>
      </div>
      {unread > 0 && !isSelected && (
        <div className="flex-shrink-0 w-5 h-5 rounded-full bg-red-500 flex items-center justify-center text-[10px] font-bold">
          {unread > 99 ? "99+" : unread}
        </div>
      )}
    </button>
  );
}
