export interface Peer {
  id: string;
  username: string;
  department: string;
  software_version?: string;
  mac_address?: string;
  ip: string;
  port: number;
  online: boolean;
  last_seen?: number;
}

export interface StoredPeer {
  peer_id: string;
  username: string;
  department: string;
  software_version?: string;
  mac_address?: string;
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
  client_msg_id?: string | null;
}

export interface AppInfo {
  initialized: boolean;
  peer_id: string;
  username: string;
  department: string;
  software_version: string;
  mac_address: string;
  listen_port: number;
  my_ip: string;
}

export interface SaveProfilePayload {
  username: string;
  department: string;
}

export interface UnreadCount {
  peer_id: string;
  count: number;
  username: string;
}

export interface UpdatePackage {
  target: "portable" | "installer";
  platform: string;
  arch: string;
  url: string;
  sha256: string;
  signature?: string | null;
  size?: number | null;
}

export interface UpdateCheckResult {
  current_version: string;
  latest_version?: string | null;
  available: boolean;
  distribution: string;
  platform: string;
  arch: string;
  package?: UpdatePackage | null;
  notes?: string | null;
  message?: string | null;
}

export interface DownloadUpdateResult {
  version: string;
  target: "portable" | "installer";
  path: string;
  ready_to_restart: boolean;
  message: string;
}

export interface GroupInfo {
  group_id: string;
  name: string;
  creator_id: string;
  created_at: string;
  members: StoredPeer[];
  last_message?: string | null;
  last_message_at?: string | null;
  last_message_sender?: string | null;
  unread_count?: number;
}

export interface GroupUnread {
  group_id: string;
  count: number;
}
