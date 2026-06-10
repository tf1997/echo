import { invoke } from "@tauri-apps/api/tauri";
import type { Peer, ChatMessage, AppInfo, SaveProfilePayload, StoredPeer, UnreadCount, UpdateCheckResult, DownloadUpdateResult, AvatarInfo } from "./types";

export async function getAppInfo(): Promise<AppInfo> {
  return await invoke("get_app_info");
}

export async function getDepartments(): Promise<string[]> {
  return await invoke("get_departments");
}

export async function saveProfile(payload: SaveProfilePayload): Promise<void> {
  await invoke("save_profile", { payload });
}

export async function setProfileAvatar(sourcePath: string): Promise<AvatarInfo> {
  return await invoke("set_profile_avatar", { sourcePath });
}

export async function clearProfileAvatar(): Promise<AvatarInfo> {
  return await invoke("clear_profile_avatar");
}

export async function requestPeerAvatar(peerId: string): Promise<StoredPeer | null> {
  return await invoke("request_peer_avatar", { peerId });
}

export async function listStoredPeers(): Promise<StoredPeer[]> {
  return await invoke("list_stored_peers");
}

export async function refreshPeerProfile(peerId: string, ip: string, port: number): Promise<StoredPeer | null> {
  return await invoke("refresh_peer_profile", { peerId, ip, port });
}

export async function getPeers(): Promise<Peer[]> {
  return await invoke("get_peers");
}

export async function sendMessage(peerId: string, content: string, clientMsgId?: string): Promise<ChatMessage> {
  return await invoke("send_message", { peerId, content, clientMsgId });
}

export async function sendMessageTyped(peerId: string, content: string, msgType: string, clientMsgId?: string): Promise<ChatMessage> {
  return await invoke("send_message_typed", { peerId, content, msgType, clientMsgId });
}

export async function sendFile(peerId: string, filePath: string, clientMsgId?: string, fileName?: string | null): Promise<ChatMessage> {
  return await invoke("send_file", { peerId, filePath, clientMsgId, fileName });
}

export async function sendSticker(peerId: string, filePath: string, clientMsgId?: string, fileName?: string | null): Promise<ChatMessage> {
  return await invoke("send_sticker", { peerId, filePath, clientMsgId, fileName });
}

export async function pauseFileTransfer(clientMsgId: string): Promise<void> {
  await invoke("pause_file_transfer", { clientMsgId });
}

export async function resumeFileTransfer(clientMsgId: string): Promise<void> {
  await invoke("resume_file_transfer", { clientMsgId });
}

export async function cancelFileTransfer(clientMsgId: string): Promise<void> {
  await invoke("cancel_file_transfer", { clientMsgId });
}

export async function getConversation(peerId: string, limit?: number): Promise<ChatMessage[]> {
  const args = limit === undefined ? { peerId } : { peerId, limit };
  return await invoke("get_conversation", args);
}

export async function markRead(peerId: string): Promise<void> {
  await invoke("mark_read", { peerId });
}

export async function getUnreadCounts(): Promise<UnreadCount[]> {
  return await invoke("get_unread_counts");
}

export interface TrayUnreadItem {
  kind: "contact" | "group";
  id: string;
  name: string;
  count: number;
  last_ts: number;
}

export async function updateTrayUnread(items: TrayUnreadItem[]): Promise<void> {
  await invoke("update_tray_unread", { items });
}

export async function getScanSubnets(): Promise<string[]> {
  return await invoke("get_scan_subnets");
}

export async function setScanSubnets(subnets: string[]): Promise<void> {
  await invoke("set_scan_subnets", { subnets });
}

export async function discoverByIp(ip: string, port: number): Promise<{ online: boolean; message: string }> {
  return await invoke("discover_by_ip", { ip, port });
}

export async function listEmojiFiles(): Promise<string[]> {
  return await invoke("list_emoji_files");
}

export async function addEmojiFile(sourcePath: string): Promise<string> {
  return await invoke("add_emoji_file", { sourcePath });
}

export async function deleteEmojiFile(filePath: string): Promise<void> {
  await invoke("delete_emoji_file", { filePath });
}

export async function listRecentContacts(): Promise<StoredPeer[]> {
  return await invoke("list_recent_contacts");
}

export async function removeRecentContact(peerId: string): Promise<void> {
  await invoke("remove_recent_contact", { peerId });
}

// Group APIs
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

export async function createGroup(name: string, members: string[]): Promise<GroupInfo> {
  return await invoke("create_group", { payload: { name, members } });
}

export async function listGroups(): Promise<GroupInfo[]> {
  return await invoke("list_groups");
}

export async function sendGroupMessage(groupId: string, content: string, clientMsgId?: string): Promise<ChatMessage> {
  return await invoke("send_group_message", { groupId, content, clientMsgId });
}

export async function sendGroupMessageTyped(groupId: string, content: string, msgType: string, clientMsgId?: string): Promise<ChatMessage> {
  return await invoke("send_group_message_typed", { groupId, content, msgType, clientMsgId });
}

export async function sendGroupFile(groupId: string, filePath: string, clientMsgId?: string, fileName?: string | null): Promise<ChatMessage> {
  return await invoke("send_group_file", { groupId, filePath, clientMsgId, fileName });
}

export async function sendGroupSticker(groupId: string, filePath: string, clientMsgId?: string, fileName?: string | null): Promise<ChatMessage> {
  return await invoke("send_group_sticker", { groupId, filePath, clientMsgId, fileName });
}

export async function getGroupMessages(groupId: string, limit?: number): Promise<ChatMessage[]> {
  const args = limit === undefined ? { groupId } : { groupId, limit };
  return await invoke("get_group_messages", args);
}

export async function renameGroup(groupId: string, newName: string): Promise<void> {
  await invoke("rename_group", { groupId, newName });
}

export async function leaveGroup(groupId: string): Promise<void> {
  await invoke("leave_group", { groupId });
}

export async function inviteToGroup(groupId: string, members: string[]): Promise<void> {
  await invoke("invite_to_group", { groupId, members });
}

export async function dissolveGroup(groupId: string): Promise<void> {
  await invoke("dissolve_group", { groupId });
}

export async function getGroupUnreadCounts(): Promise<GroupUnread[]> {
  return await invoke("get_group_unread_counts");
}

export async function markGroupRead(groupId: string): Promise<void> {
  await invoke("mark_group_read", { groupId });
}

export async function saveTempFile(data: number[], filename: string): Promise<string> {
  return await invoke("save_temp_file", { data, filename });
}

export interface ScreenshotData {
  base64: string;
  mime: string;
  width: number;
  height: number;
  x: number;
  y: number;
}

export async function captureScreenshotNative(): Promise<ScreenshotData> {
  return await invoke("capture_screenshot");
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

export type HistoryFilter = "all" | "file" | "image";

export async function searchMessages(query: string): Promise<SearchResult[]> {
  return await invoke("search_messages", { query });
}

export async function getConversationHistory(peerId: string, beforeId?: number, limit?: number, filter?: HistoryFilter, dayStart?: string, dayEnd?: string): Promise<ChatMessage[]> {
  return await invoke("get_conversation_history", {
    peerId,
    beforeId,
    limit,
    filter: filter === "all" ? undefined : filter,
    dayStart,
    dayEnd,
  });
}

export async function getGroupHistory(groupId: string, beforeId?: number, limit?: number, filter?: HistoryFilter, dayStart?: string, dayEnd?: string): Promise<ChatMessage[]> {
  return await invoke("get_group_history", {
    groupId,
    beforeId,
    limit,
    filter: filter === "all" ? undefined : filter,
    dayStart,
    dayEnd,
  });
}

export async function deleteChatMessages(messageIds: number[]): Promise<number> {
  return await invoke("delete_chat_messages", { messageIds });
}

export async function searchConversationMessages(peerId: string, query: string, limit?: number, filter?: HistoryFilter, dayStart?: string, dayEnd?: string): Promise<ChatMessage[]> {
  const args = limit === undefined ? { peerId, query, dayStart, dayEnd } : { peerId, query, limit, dayStart, dayEnd };
  if (filter && filter !== "all") return await invoke("search_conversation_messages", { ...args, filter });
  return await invoke("search_conversation_messages", args);
}

export async function searchGroupChatMessages(groupId: string, query: string, limit?: number, filter?: HistoryFilter, dayStart?: string, dayEnd?: string): Promise<ChatMessage[]> {
  const args = limit === undefined ? { groupId, query, dayStart, dayEnd } : { groupId, query, limit, dayStart, dayEnd };
  if (filter && filter !== "all") return await invoke("search_group_messages", { ...args, filter });
  return await invoke("search_group_messages", args);
}

export async function checkPeerOnline(peerId: string, ip: string, port: number): Promise<boolean> {
  return await invoke("check_peer_online", { peerId, ip, port });
}

export async function checkForUpdates(): Promise<UpdateCheckResult> {
  return await invoke("check_for_updates_command");
}

export async function downloadUpdate(): Promise<DownloadUpdateResult> {
  return await invoke("download_update_command");
}

export async function restartAfterUpdate(): Promise<void> {
  await invoke("restart_after_update_command");
}
