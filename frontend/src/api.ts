import { invoke } from "@tauri-apps/api/core";
import type { Peer, ChatMessage, AppInfo, SaveProfilePayload, StoredPeer, UnreadCount } from "./types";

export async function getAppInfo(): Promise<AppInfo> {
  return await invoke("get_app_info");
}

export async function getDepartments(): Promise<string[]> {
  return await invoke("get_departments");
}

export async function saveProfile(payload: SaveProfilePayload): Promise<void> {
  await invoke("save_profile", { payload });
}

export async function listStoredPeers(): Promise<StoredPeer[]> {
  return await invoke("list_stored_peers");
}

export async function getPeers(): Promise<Peer[]> {
  return await invoke("get_peers");
}

export async function sendMessage(peerId: string, content: string): Promise<ChatMessage> {
  return await invoke("send_message", { peerId, content });
}

export async function sendFile(peerId: string, filePath: string): Promise<ChatMessage> {
  return await invoke("send_file", { peerId, filePath });
}

export async function getConversation(peerId: string): Promise<ChatMessage[]> {
  return await invoke("get_conversation", { peerId });
}

export async function markRead(peerId: string): Promise<void> {
  await invoke("mark_read", { peerId });
}

export async function getUnreadCounts(): Promise<UnreadCount[]> {
  return await invoke("get_unread_counts");
}

export async function saveTempFile(data: number[], filename: string): Promise<string> {
  return await invoke("save_temp_file", { data, filename });
}

interface FileData {
  base64: string;
  mime: string;
}

export async function readFileBase64(filePath: string): Promise<FileData> {
  return await invoke("read_file_base64", { filePath });
}

export async function openFile(filePath: string): Promise<void> {
  return await invoke("open_file", { path: filePath });
}

export async function openFolder(filePath: string): Promise<void> {
  return await invoke("open_folder", { path: filePath });
}

export interface SearchHit {
  id: number;
  sender_id: string;
  sender_name: string;
  receiver_id: string;
  content: string;
  msg_type: string;
  file_name: string | null;
  file_path: string | null;
  timestamp: string;
}

export interface SearchResult {
  peer_id: string;
  peer_name: string;
  messages: SearchHit[];
}

export async function searchMessages(query: string): Promise<SearchResult[]> {
  return await invoke("search_messages", { query });
}

export async function checkPeerOnline(ip: string, port: number): Promise<boolean> {
  return await invoke("check_peer_online", { ip, port });
}




