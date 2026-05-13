import { invoke } from "@tauri-apps/api/core";
import type { Peer, ChatMessage, AppInfo, SaveProfilePayload, StoredPeer } from "./types";

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
