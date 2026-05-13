import { useState, useCallback } from "react";
import type { Peer, UnreadCount } from "../types";
import { searchMessages } from "../api";
import type { SearchResult } from "../api";

interface SidebarProps {
  peers: Peer[];
  selectedPeerId: string | null;
  onSelectPeer: (peer: Peer) => void;
  onJumpToSearchHit: (peerId: string) => void;
  myId: string;
  myName: string;
  myDepartment: string;
  onEditProfile: () => void;
  unreadCounts: UnreadCount[];
}

export function Sidebar({ peers, selectedPeerId, onSelectPeer, myId, myName, myDepartment, onEditProfile, unreadCounts, onJumpToSearchHit }: SidebarProps) {
  const [searchQuery, setSearchQuery] = useState("");
  const [searchResults, setSearchResults] = useState<SearchResult[]>([]);
  const [searching, setSearching] = useState(false);
  const [showSearch, setShowSearch] = useState(false);

  const onlinePeers = peers.filter((p) => p.online);
  const offlinePeers = peers.filter((p) => !p.online);

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

  return (
    <div className="flex flex-col w-72 bg-gray-900 text-white h-full border-r border-gray-700">
      <div className="p-4 border-b border-gray-700">
        <div className="flex items-center gap-3">
          <div className="w-10 h-10 rounded-full bg-indigo-500 flex items-center justify-center text-lg font-bold">
            {myName.charAt(0).toUpperCase()}
          </div>
          <div className="flex-1 min-w-0">
            <p className="text-sm font-semibold truncate">{myName}</p>
            <p className="text-xs text-gray-400 truncate">{myDepartment}</p>
            <p className="text-[10px] text-gray-500 truncate">ID: {myId.slice(0, 8)}...</p>
          </div>
          <button
            onClick={onEditProfile}
            className="text-xs px-2 py-1 rounded bg-gray-700 hover:bg-gray-600"
          >
            编辑
          </button>
        </div>
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
        <div className="flex-1 overflow-y-auto">
          {onlinePeers.length > 0 && (
            <div>
              <p className="px-4 py-2 text-xs text-gray-400 font-medium uppercase tracking-wider">在线 — {onlinePeers.length}</p>
              {onlinePeers.map((peer) => (
                <PeerItem key={peer.id} peer={peer} isSelected={selectedPeerId === peer.id} unread={unreadMap.get(peer.id) ?? 0} onClick={() => onSelectPeer(peer)} />
              ))}
            </div>
          )}

          {offlinePeers.length > 0 && (
            <div>
              <p className="px-4 py-2 text-xs text-gray-400 font-medium uppercase tracking-wider">离线/历史 — {offlinePeers.length}</p>
              {offlinePeers.map((peer) => (
                <PeerItem key={peer.id} peer={peer} isSelected={selectedPeerId === peer.id} unread={unreadMap.get(peer.id) ?? 0} onClick={() => onSelectPeer(peer)} />
              ))}
            </div>
          )}
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
