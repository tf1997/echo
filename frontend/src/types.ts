export interface Peer {
  id: string;
  username: string;
  department: string;
  ip: string;
  port: number;
  online: boolean;
}

export interface StoredPeer {
  peer_id: string;
  username: string;
  department: string;
  ip: string;
  port: number;
  is_online: boolean;
  first_seen_at: string;
  last_seen_at: string;
}

export interface ChatMessage {
  id: number;
  sender_id: string;
  sender_name: string;
  receiver_id: string;
  content: string;
  msg_type: string;
  file_path: string | null;
  file_name: string | null;
  file_size: number | null;
  timestamp: string;
  is_read: boolean;
}

export interface AppInfo {
  initialized: boolean;
  peer_id: string;
  username: string;
  department: string;
  listen_port: number;
}

export interface SaveProfilePayload {
  username: string;
  department: string;
}

export interface UnreadCount {
  peer_id: string;
  count: number;
}
