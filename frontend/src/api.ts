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


