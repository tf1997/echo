import type { Peer, UnreadCount } from "../types";

interface SidebarProps {
  peers: Peer[];
  selectedPeerId: string | null;
  onSelectPeer: (peer: Peer) => void;
  myId: string;
  myName: string;
  myDepartment: string;
  onEditProfile: () => void;
  unreadCounts: UnreadCount[];
}

export function Sidebar({ peers, selectedPeerId, onSelectPeer, myId, myName, myDepartment, onEditProfile, unreadCounts }: SidebarProps) {
  const onlinePeers = peers.filter((p) => p.online);
  const offlinePeers = peers.filter((p) => !p.online);

  const unreadMap = new Map<string, number>();
  for (const uc of unreadCounts) {
    unreadMap.set(uc.peer_id, uc.count);
  }

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

      <div className="flex-1 overflow-y-auto">
        {onlinePeers.length > 0 && (
          <div>
            <p className="px-4 py-2 text-xs text-gray-400 font-medium uppercase tracking-wider">在线 — {onlinePeers.length}</p>
            {onlinePeers.map((peer) => (
              <PeerItem
                key={peer.id}
                peer={peer}
                isSelected={selectedPeerId === peer.id}
                unread={unreadMap.get(peer.id) ?? 0}
                onClick={() => onSelectPeer(peer)}
              />
            ))}
          </div>
        )}

        {offlinePeers.length > 0 && (
          <div>
            <p className="px-4 py-2 text-xs text-gray-400 font-medium uppercase tracking-wider">离线/历史 — {offlinePeers.length}</p>
            {offlinePeers.map((peer) => (
              <PeerItem
                key={peer.id}
                peer={peer}
                isSelected={selectedPeerId === peer.id}
                unread={unreadMap.get(peer.id) ?? 0}
                onClick={() => onSelectPeer(peer)}
              />
            ))}
          </div>
        )}
      </div>

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
