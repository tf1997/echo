import { useState, useRef, useEffect, useCallback } from "react";
import type { ChatMessage, Peer } from "../types";
import type { GroupInfo } from "../api";
import { MessageBubble, DateDivider } from "./MessageBubble";
import { HistorySearchView } from "./HistorySearchView";
import { Avatar } from "./Avatar";
import { AvatarPreviewTrigger } from "./AvatarPreview";
import { formatDateLabel, makeSearchHitId } from "./messageUtils";
import { saveTempFile, listEmojiFiles, addEmojiFile, deleteEmojiFile, sendMessage, sendMessageTyped, sendFile, sendSticker, sendGroupMessage, sendGroupMessageTyped, sendGroupFile, sendGroupSticker, renameGroup, leaveGroup, dissolveGroup, inviteToGroup, readFileBase64, pauseFileTransfer, resumeFileTransfer, cancelFileTransfer } from "../api";
import type { ForwardCardData } from "./MessageBubble";
import { ask, message as showDialogMessage, open } from "@tauri-apps/api/dialog";
import { convertFileSrc } from "@tauri-apps/api/tauri";

export interface PendingMessage {
  id: number;
  clientMsgId: string; // 必填，用于精确匹配数据库消息
  content: string;
  msg_type: string;
  file_name?: string;
  file_path?: string;
  file_size?: number;
  status: "sending" | "paused" | "failed" | "sent";
  error?: string;
  progress?: number; // 0-100
  speed?: number; // bytes/sec
  createdAt?: number;
}

// 生成唯一的客户端消息 ID
function generateClientMsgId(): string {
  return `${Date.now()}_${Math.random().toString(36).substr(2, 9)}`;
}

interface ChatWindowProps {
  peer: Peer | null;
  messages: ChatMessage[];
  myId: string;
  myName?: string;
  conversationResetKey: number;
  loadingMessages?: boolean;
  isGroup?: boolean;
  groupId?: string | null;
  groupInfo?: GroupInfo | null;
  peers?: Peer[];
  groups?: GroupInfo[];
  onSendMessage: (content: string, clientMsgId?: string) => Promise<ChatMessage>;
  onSendFile: (filePath: string, clientMsgId?: string) => Promise<void | ChatMessage>;
  onSendSticker: (filePath: string, clientMsgId?: string) => Promise<ChatMessage>;
  onGroupUpdated?: () => void;
  onLoadHistoryContext?: (messageId: number) => Promise<void>;
  historySearchRequest?: {
    query: string;
    messageId?: number | null;
    nonce: number;
  } | null;
}

let pendingId = Date.now();
const EMPTY_PENDING_MESSAGES: PendingMessage[] = [];
type PendingMessagesUpdate = PendingMessage[] | ((prev: PendingMessage[]) => PendingMessage[]);

interface TextSearchHit {
  id: string;
  messageId: number;
  occurrenceIndex: number;
}

function getTextSearchHits(messages: ChatMessage[], query: string): TextSearchHit[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return [];

  const hits: TextSearchHit[] = [];
  for (const message of messages) {
    if (message.msg_type !== "text") continue;
    const haystack = message.content.toLowerCase();
    let cursor = 0;
    let occurrenceIndex = 0;
    let matchIndex = haystack.indexOf(needle, cursor);
    while (matchIndex !== -1) {
      hits.push({
        id: makeSearchHitId(message.id, occurrenceIndex),
        messageId: message.id,
        occurrenceIndex,
      });
      occurrenceIndex += 1;
      cursor = matchIndex + needle.length;
      matchIndex = haystack.indexOf(needle, cursor);
    }
  }
  return hits;
}

async function readFileAndSave(file: File): Promise<string> {
  const buffer = await file.arrayBuffer();
  const data = Array.from(new Uint8Array(buffer));
  return await saveTempFile(data, file.name || "file");
}

const EMOJIS = [
  "😀","😃","😄","😁","😆","😂","🤣","😅","😊","🙂",
  "🙃","😉","😍","🥰","😘","😋","😜","🤪","😎","🤩",
  "🥳","🤭","🤫","🤔","🫡","😏","🙄","😬","😐","😑",
  "😶","🫥","🫣","🫢","😳","😮","😲","😵","😵‍💫","🤯",
  "🥺","😢","😭","😤","😡","🤬","😒","😔","😪","😴",
  "🥱","🤤","😷","🤒","🤧","🤮","😇","😈","🤡","🫠",
  "🤦","🙈","🙉","🙊","👍","👎","👌","🤌","👏","🙌",
  "🙏","🤝","💪","👊","✌️","🤟","👋","🤲","💅","👀",
  "🎉","🔥","❤️","🧡","💛","💚","💙","💜","💔","💯",
  "✅","❌","⭐","🌟","💡","🎵","🌹","☕","🍕","🚀",
  "🐶","🐕","🦮","🐾","🦴","🐱","🐭","🐹","🐰","🦊",
  "🐼","🐻","🐨","🐯","🦁","🐸","🐵","🐧","🐔","🐟",
  "📎","📁","🎂","🏆","🥇","💩"
];

function EmojiThumb({ path }: { path: string }) {
  const [failedPath, setFailedPath] = useState<string | null>(null);
  const src = convertFileSrc(path);
  const failed = failedPath === path;

  if (failed) return <div className="w-full h-full bg-gray-700 rounded" />;
  return <img src={src} alt="" className="w-full h-full object-contain" onError={() => setFailedPath(path)} />;
}

function formatSpeed(bytesPerSec: number | undefined): string {
  if (!bytesPerSec || bytesPerSec === 0) return "";
  if (bytesPerSec >= 1_000_000) return `${(bytesPerSec / 1_000_000).toFixed(1)} MB/s`;
  if (bytesPerSec >= 1_000) return `${(bytesPerSec / 1_000).toFixed(0)} KB/s`;
  return `${bytesPerSec} B/s`;
}

function getPendingStatusText(message: PendingMessage): string {
  if (message.status === "paused") {
    return message.progress !== undefined ? `已暂停 · ${message.progress}%` : "已暂停";
  }
  if (message.status === "failed") {
    return message.error ? `发送失败：${message.error}` : "发送失败";
  }
  if (message.msg_type === "file" && message.progress !== undefined) {
    const speed = formatSpeed(message.speed);
    return speed ? `${message.progress}% ${speed}` : `${message.progress}%`;
  }
  return "发送中...";
}

type ForwardMode = "individual" | "merged";

function isAttachmentMessage(message: ChatMessage): boolean {
  return message.msg_type === "file" || message.msg_type === "sticker";
}

function fallbackForwardText(message: ChatMessage): string {
  if (message.msg_type === "file") return `📎 ${message.file_name || message.content || "文件"}`;
  if (message.msg_type === "sticker") return "[表情]";
  if (message.msg_type === "forward_card") return "[聊天记录]";
  return message.content;
}

async function buildForwardCard(messages: ChatMessage[]): Promise<ForwardCardData> {
  const items = await Promise.all(messages.map(async (message) => {
    const item: ForwardCardData["items"][number] = {
      sender: message.sender_name,
      content: message.content,
      msg_type: message.msg_type,
      timestamp: message.timestamp,
      file_name: message.file_name,
      file_size: message.file_size,
    };

    if (isAttachmentMessage(message)) {
      item.content = fallbackForwardText(message);
      if (message.file_path) {
        try {
          const file = await readFileBase64(message.file_path);
          item.file_data = file.base64;
          item.mime = file.mime;
        } catch {
          item.attachment_error = "文件不可用";
        }
      } else {
        item.attachment_error = "文件不可用";
      }
    }

    return item;
  }));

  return {
    title: `${messages[0]?.sender_name ?? ""} 等人的聊天记录`,
    items,
  };
}

interface ForwardModalProps {
  messages: ChatMessage[];
  mode: ForwardMode;
  peers: Peer[];
  groups: GroupInfo[];
  myId: string;
  onClose: () => void;
}

function ForwardModal({ messages, mode, peers, groups, myId, onClose }: ForwardModalProps) {
  const [query, setQuery] = useState("");
  const [sending, setSending] = useState<string | null>(null);
  const [sent, setSent] = useState<Set<string>>(new Set());

  const filteredPeers = peers.filter((p) => p.id !== myId && p.username.toLowerCase().includes(query.toLowerCase()));
  const filteredGroups = groups.filter((g) => g.name.toLowerCase().includes(query.toLowerCase()));

  const forward = async (targetId: string, isGroup: boolean) => {
    if (sent.has(targetId) || sending) return;
    setSending(targetId);
    try {
      const sorted = [...messages].sort((a, b) => new Date(a.timestamp).getTime() - new Date(b.timestamp).getTime());
      if (mode === "merged") {
        const card = await buildForwardCard(sorted);
        const json = JSON.stringify(card);
        if (isGroup) await sendGroupMessageTyped(targetId, json, "forward_card");
        else await sendMessageTyped(targetId, json, "forward_card");
      } else {
        for (const m of sorted) {
          if (m.msg_type === "file" && m.file_path) {
            if (isGroup) await sendGroupFile(targetId, m.file_path, undefined, m.file_name);
            else await sendFile(targetId, m.file_path, undefined, m.file_name);
          } else if (m.msg_type === "sticker" && m.file_path) {
            if (isGroup) await sendGroupSticker(targetId, m.file_path, undefined, m.file_name);
            else await sendSticker(targetId, m.file_path, undefined, m.file_name);
          } else if (m.msg_type === "forward_card") {
            if (isGroup) await sendGroupMessageTyped(targetId, m.content, "forward_card");
            else await sendMessageTyped(targetId, m.content, "forward_card");
          } else {
            const content = isAttachmentMessage(m) ? fallbackForwardText(m) : m.content;
            if (isGroup) await sendGroupMessage(targetId, content);
            else await sendMessage(targetId, content);
          }
        }
      }
      setSent((prev) => new Set([...prev, targetId]));
    } catch {
      // ignore
    } finally {
      setSending(null);
    }
  };

  const modeLabel = mode === "merged" ? "合并转发" : "逐条转发";

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60" onClick={onClose}>
      <div className="bg-gray-800 border border-gray-600 rounded-2xl w-80 max-h-[480px] flex flex-col shadow-2xl" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center justify-between px-4 py-3 border-b border-gray-700">
          <div>
            <span className="text-sm font-semibold text-white">转发给</span>
            <span className="ml-2 text-xs text-gray-400">{modeLabel} · {messages.length} 条</span>
          </div>
          <button onClick={onClose} className="text-gray-400 hover:text-white text-lg leading-none">×</button>
        </div>
        <div className="px-3 py-2 border-b border-gray-700">
          <input autoFocus value={query} onChange={(e) => setQuery(e.target.value)}
            placeholder="搜索联系人或群聊..."
            className="w-full bg-gray-700 text-white text-sm rounded-lg px-3 py-1.5 outline-none focus:ring-1 focus:ring-indigo-500 placeholder-gray-400"
          />
        </div>
        <div className="overflow-y-auto flex-1 py-1">
          {filteredGroups.map((g) => (
            <button key={g.group_id} onClick={() => forward(g.group_id, true)} disabled={!!sending}
              className="w-full flex items-center gap-3 px-4 py-2 hover:bg-gray-700 text-left disabled:opacity-60">
              <div className="w-8 h-8 rounded-full bg-indigo-700 flex items-center justify-center text-sm flex-shrink-0">👥</div>
              <span className="text-sm text-gray-200 flex-1 truncate">{g.name}</span>
              {sent.has(g.group_id) ? <span className="text-xs text-green-400">已发送</span>
                : sending === g.group_id ? <span className="text-xs text-gray-400">发送中</span> : null}
            </button>
          ))}
          {filteredPeers.map((p) => (
            <button key={p.id} onClick={() => forward(p.id, false)} disabled={!!sending}
              className="w-full flex items-center gap-3 px-4 py-2 hover:bg-gray-700 text-left disabled:opacity-60">
              <Avatar name={p.username} src={p.avatar_path} size="sm" online={p.online} />
              <div className="flex-1 min-w-0">
                <p className="text-sm text-gray-200 truncate">{p.username}</p>
                {!p.online && <p className="text-[10px] text-gray-500">离线</p>}
              </div>
              {sent.has(p.id) ? <span className="text-xs text-green-400">已发送</span>
                : sending === p.id ? <span className="text-xs text-gray-400">发送中</span> : null}
            </button>
          ))}
          {filteredPeers.length === 0 && filteredGroups.length === 0 && (
            <p className="text-center text-gray-500 text-sm py-6">无匹配联系人</p>
          )}
        </div>
      </div>
    </div>
  );
}

export function ChatWindow({ peer, messages, myId, myName = "", conversationResetKey, loadingMessages = false, isGroup = false, groupId = null, groupInfo, peers = [], groups = [], onSendMessage, onSendFile, onSendSticker, onGroupUpdated, onLoadHistoryContext, historySearchRequest = null }: ChatWindowProps) {
  const peerId = peer?.id ?? null;
  const pendingConversationKey = isGroup
    ? groupId ? `group:${groupId}` : peerId ? `group:${peerId}` : ""
    : peer ? `contact:${peer.ip && peer.port ? `${peer.ip}:${peer.port}` : peer.id}` : "";
  const pendingConversationKeyRef = useRef(pendingConversationKey);
  pendingConversationKeyRef.current = pendingConversationKey;
  const [inputText, setInputText] = useState("");
  const [isDragging, setIsDragging] = useState(false);
  const [pendingByConversation, setPendingByConversation] = useState<Map<string, PendingMessage[]>>(() => new Map());
  const pendingMessages = pendingConversationKey ? pendingByConversation.get(pendingConversationKey) ?? EMPTY_PENDING_MESSAGES : EMPTY_PENDING_MESSAGES;
  const updatePendingMessagesForKey = useCallback((conversationKey: string, update: PendingMessagesUpdate) => {
    if (!conversationKey) return;
    setPendingByConversation((prev) => {
      const current = prev.get(conversationKey) ?? EMPTY_PENDING_MESSAGES;
      const next = typeof update === "function" ? update(current) : update;
      if (next === current) return prev;
      const nextMap = new Map(prev);
      if (next.length === 0) {
        nextMap.delete(conversationKey);
      } else {
        nextMap.set(conversationKey, next);
      }
      return nextMap;
    });
  }, []);
  const setPendingMessages = useCallback((update: PendingMessagesUpdate) => {
    updatePendingMessagesForKey(pendingConversationKeyRef.current, update);
  }, [updatePendingMessagesForKey]);
  const updatePendingMessagesEverywhere = useCallback((
    matches: (message: PendingMessage) => boolean,
    update: (message: PendingMessage) => PendingMessage,
  ) => {
    setPendingByConversation((prev) => {
      let changed = false;
      const nextMap = new Map(prev);
      for (const [conversationKey, list] of prev) {
        let listChanged = false;
        const nextList = list.map((message) => {
          if (!matches(message)) return message;
          const nextMessage = update(message);
          if (nextMessage !== message) listChanged = true;
          return nextMessage;
        });
        if (listChanged) {
          changed = true;
          nextMap.set(conversationKey, nextList);
        }
      }
      return changed ? nextMap : prev;
    });
  }, []);
  const removePendingMessagesEverywhere = useCallback((matches: (message: PendingMessage) => boolean) => {
    setPendingByConversation((prev) => {
      let changed = false;
      const nextMap = new Map(prev);
      for (const [conversationKey, list] of prev) {
        const nextList = list.filter((message) => !matches(message));
        if (nextList.length !== list.length) {
          changed = true;
          if (nextList.length === 0) {
            nextMap.delete(conversationKey);
          } else {
            nextMap.set(conversationKey, nextList);
          }
        }
      }
      return changed ? nextMap : prev;
    });
  }, []);
  const [showEmoji, setShowEmoji] = useState(false);
  const [emojiTab, setEmojiTab] = useState<"default" | "custom">("default");
  const [customEmojis, setCustomEmojis] = useState<string[]>([]);
  const [deletingEmoji, setDeletingEmoji] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState("");
  const [showSearch, setShowSearch] = useState(false);
  const [showHistory, setShowHistory] = useState(false);
  const [searchIndex, setSearchIndex] = useState(0);
  const [selectMode, setSelectMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());
  const [forwardModal, setForwardModal] = useState<{ messages: ChatMessage[]; mode: ForwardMode } | null>(null);
  const [showGroupPanel, setShowGroupPanel] = useState(false);
  const [groupNameEdit, setGroupNameEdit] = useState("");
  const [groupActionBusy, setGroupActionBusy] = useState("");
  const [groupPanelError, setGroupPanelError] = useState("");
  const [groupMemberQuery, setGroupMemberQuery] = useState("");
  const [groupInviteQuery, setGroupInviteQuery] = useState("");
  const [contextHighlightId, setContextHighlightId] = useState<number | null>(null);
  const messageRefs = useRef<Map<number, HTMLDivElement>>(new Map());
  const pendingJumpMessageIdRef = useRef<number | null>(null);
  const contextHighlightTimerRef = useRef<number | null>(null);

  const exitSelectMode = useCallback(() => { setSelectMode(false); setSelectedIds(new Set()); }, []);

  useEffect(() => {
    if (!historySearchRequest?.query.trim()) return;
    setForwardModal(null);
    exitSelectMode();
    setShowSearch(false);
    setSearchQuery("");
    setSearchIndex(0);
    setShowHistory(true);
  }, [exitSelectMode, historySearchRequest?.nonce, historySearchRequest?.query]);

  const handleStartForward = useCallback((msg: ChatMessage) => {
    setSelectMode(true);
    setSelectedIds(new Set([msg.id]));
  }, []);

  const handleToggleSelect = useCallback((msg: ChatMessage) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(msg.id)) next.delete(msg.id); else next.add(msg.id);
      return next;
    });
  }, []);

  const openForwardModal = useCallback((mode: ForwardMode) => {
    const selected = messages.filter((m) => selectedIds.has(m.id));
    if (selected.length === 0) return;
    setForwardModal({ messages: selected, mode });
  }, [messages, selectedIds]);

  // 核心去重逻辑：通过 client_msg_id 自动清理已保存的 pending 消息
  useEffect(() => {
    setPendingMessages((prev) => prev.filter((p) => {
      if (p.status === "failed") return true; // 保留失败的消息

      // 检查是否已经在数据库消息中（通过 client_msg_id 精确匹配）
      const exists = messages.some((m) =>
        m.client_msg_id && m.client_msg_id === p.clientMsgId
      );

      return !exists; // 如果已存在，移除 pending
    }));
  }, [messages, setPendingMessages]);

  // Load custom emojis
  useEffect(() => {
    listEmojiFiles().then(setCustomEmojis).catch(() => {});
  }, []);

  const handleAddEmoji = useCallback(async () => {
    const selected = await open({ multiple: true, filters: [{ name: "图片", extensions: ["png", "jpg", "jpeg", "gif", "webp"] }] });
    if (!selected) return;
    const paths = Array.isArray(selected) ? selected : [selected];
    for (const path of paths) {
      if (!path) continue;
      try {
        const saved = await addEmojiFile(path);
        setCustomEmojis((prev) => prev.includes(saved) ? prev : [...prev, saved]);
        setEmojiTab("custom");
      } catch (e) {
        console.error("Failed to add emoji:", e);
      }
    }
  }, []);

  const handleAddStickerFromMessage = useCallback(async (message: ChatMessage) => {
    if (message.msg_type !== "sticker" || !message.file_path) return;
    try {
      const saved = await addEmojiFile(message.file_path);
      setCustomEmojis((prev) => prev.includes(saved) ? prev : [...prev, saved]);
      setEmojiTab("custom");
    } catch (e) {
      console.error("Failed to add sticker from message:", e);
    }
  }, []);

  const handleDeleteEmoji = useCallback(async (path: string) => {
    if (deletingEmoji) return;
    setDeletingEmoji(path);
    try {
      await deleteEmojiFile(path);
      setCustomEmojis((prev) => prev.filter((item) => item !== path));
    } catch (e) {
      console.error("Failed to delete emoji:", e);
    } finally {
      setDeletingEmoji(null);
    }
  }, [deletingEmoji]);

  // Listen for file send progress
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let unlistenError: (() => void) | undefined;
    import("@tauri-apps/api/event").then(({ listen }) => {
      listen<{ fileName: string; clientMsgId?: string | null; sent: number; total: number; speed: number }>("file-progress", (event) => {
        const { fileName, clientMsgId, sent, total, speed } = event.payload;
        const pct = total > 0 ? Math.round((sent / total) * 100) : 0;
        const matchesFile = (message: PendingMessage) =>
          clientMsgId ? message.clientMsgId === clientMsgId : message.file_name === fileName && message.msg_type === "file";
        updatePendingMessagesEverywhere(
          matchesFile,
          (message) => ({ ...message, progress: pct, speed, status: pct >= 100 ? "sent" as const : message.status })
        );
        // Remove pending after 2s (real message will be in DB by then)
        if (pct >= 100) {
          setTimeout(() => {
            removePendingMessagesEverywhere(matchesFile);
          }, 2000);
        }
      }).then((fn) => { unlisten = fn; });

      listen<{ fileName: string; clientMsgId?: string | null; error: string }>("file-error", (event) => {
        const { fileName, clientMsgId, error } = event.payload;
        updatePendingMessagesEverywhere(
          (message) => clientMsgId ? message.clientMsgId === clientMsgId : message.file_name === fileName,
          (message) => ({ ...message, status: "failed", error })
        );
      }).then((fn) => { unlistenError = fn; });
    });
    return () => { unlisten?.(); unlistenError?.(); };
  }, [removePendingMessagesEverywhere, updatePendingMessagesEverywhere]);
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const messagesContainerRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const emojiPopoverRef = useRef<HTMLDivElement>(null);
  const dragResetTimerRef = useRef<number | null>(null);
  const nearBottomRef = useRef(true);

  const pendingScrollRef = useRef(false);

  const hideDragOverlay = useCallback(() => {
    if (dragResetTimerRef.current !== null) {
      window.clearTimeout(dragResetTimerRef.current);
      dragResetTimerRef.current = null;
    }
    setIsDragging(false);
  }, []);

  const showDragOverlay = useCallback(() => {
    setIsDragging(true);
    if (dragResetTimerRef.current !== null) {
      window.clearTimeout(dragResetTimerRef.current);
    }
    dragResetTimerRef.current = window.setTimeout(() => {
      dragResetTimerRef.current = null;
      setIsDragging(false);
    }, 2500);
  }, []);

  useEffect(() => {
    return () => {
      if (dragResetTimerRef.current !== null) {
        window.clearTimeout(dragResetTimerRef.current);
      }
    };
  }, []);

  useEffect(() => {
    window.addEventListener("blur", hideDragOverlay);
    window.addEventListener("focus", hideDragOverlay);
    document.addEventListener("dragend", hideDragOverlay);
    document.addEventListener("drop", hideDragOverlay);
    return () => {
      window.removeEventListener("blur", hideDragOverlay);
      window.removeEventListener("focus", hideDragOverlay);
      document.removeEventListener("dragend", hideDragOverlay);
      document.removeEventListener("drop", hideDragOverlay);
    };
  }, [hideDragOverlay]);

  const scrollToMessage = useCallback((messageId: number) => {
    requestAnimationFrame(() => {
      messageRefs.current.get(messageId)?.scrollIntoView({ behavior: "smooth", block: "center" });
    });
  }, []);

  const highlightContextMessage = useCallback((messageId: number) => {
    setContextHighlightId(messageId);
    if (contextHighlightTimerRef.current !== null) {
      window.clearTimeout(contextHighlightTimerRef.current);
    }
    contextHighlightTimerRef.current = window.setTimeout(() => {
      setContextHighlightId(null);
      contextHighlightTimerRef.current = null;
    }, 2600);
  }, []);

  const handleJumpToHistoryMessage = useCallback(async (messageId: number) => {
    pendingJumpMessageIdRef.current = messageId;
    setShowHistory(false);
    setShowSearch(false);
    setSearchQuery("");
    setSearchIndex(0);
    nearBottomRef.current = false;
    try {
      if (!messages.some((message) => message.id === messageId)) {
        await onLoadHistoryContext?.(messageId);
      }
    } catch (error) {
      pendingJumpMessageIdRef.current = null;
      console.error("Failed to load history context:", error);
      return;
    }
    highlightContextMessage(messageId);
    scrollToMessage(messageId);
  }, [highlightContextMessage, messages, onLoadHistoryContext, scrollToMessage]);

  useEffect(() => {
    const messageId = pendingJumpMessageIdRef.current;
    if (messageId === null || showHistory) return;
    if (!messageRefs.current.has(messageId)) return;
    pendingJumpMessageIdRef.current = null;
    highlightContextMessage(messageId);
    scrollToMessage(messageId);
  }, [highlightContextMessage, messages, scrollToMessage, showHistory]);

  useEffect(() => {
    return () => {
      if (contextHighlightTimerRef.current !== null) {
        window.clearTimeout(contextHighlightTimerRef.current);
      }
    };
  }, []);

  // Clear transient UI only when the app confirms an intentional conversation switch.
  // Background peer refreshes can change peer.id for the same endpoint and should not
  // close the history view.
  useEffect(() => {
    if (!historySearchRequest?.query.trim()) {
      setShowHistory(false);
    }
    setShowSearch(false);
    setSearchQuery("");
    setSearchIndex(0);
    setContextHighlightId(null);
    setGroupMemberQuery("");
    setGroupInviteQuery("");
    pendingJumpMessageIdRef.current = null;
    nearBottomRef.current = true;
    pendingScrollRef.current = true;
    requestAnimationFrame(() => inputRef.current?.focus());
  }, [conversationResetKey, historySearchRequest?.query]);

  const handleScroll = useCallback(() => {
    const el = messagesContainerRef.current;
    if (!el) return;
    nearBottomRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 100;
  }, []);

  useEffect(() => {
    if (pendingScrollRef.current) {
      pendingScrollRef.current = false;
      requestAnimationFrame(() => {
        messagesEndRef.current?.scrollIntoView({ behavior: "instant" });
      });
    } else if (nearBottomRef.current) {
      messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
    }
  }, [messages, pendingMessages]);

  useEffect(() => {
    if (!showEmoji) return;
    const handlePointerDown = (event: MouseEvent) => {
      if (!emojiPopoverRef.current?.contains(event.target as Node)) {
        setShowEmoji(false);
      }
    };
    document.addEventListener("mousedown", handlePointerDown);
    return () => document.removeEventListener("mousedown", handlePointerDown);
  }, [showEmoji]);

  const retryText = useCallback(async (pending: PendingMessage) => {
    const conversationKey = pendingConversationKeyRef.current;
    updatePendingMessagesForKey(conversationKey, (prev) => prev.filter((p) => p.id !== pending.id));
    const newClientMsgId = generateClientMsgId();
    try {
      await onSendMessage(pending.content, newClientMsgId);
    } catch {
      updatePendingMessagesForKey(conversationKey, (prev) => [...prev, {
        ...pending,
        id: ++pendingId,
        clientMsgId: newClientMsgId,
        status: "failed",
        error: "重试失败",
      }]);
    }
  }, [onSendMessage, updatePendingMessagesForKey]);

  const retryFile = useCallback(async (pending: PendingMessage) => {
    if (!pending.file_path) return;
    const conversationKey = pendingConversationKeyRef.current;
    updatePendingMessagesForKey(conversationKey, (prev) => prev.filter((p) => p.id !== pending.id));
    const newClientMsgId = generateClientMsgId();
    try {
      await onSendFile(pending.file_path, newClientMsgId);
    } catch {
      updatePendingMessagesForKey(conversationKey, (prev) => [...prev, {
        ...pending,
        id: ++pendingId,
        clientMsgId: newClientMsgId,
        status: "failed",
        error: "重试失败",
      }]);
    }
  }, [onSendFile, updatePendingMessagesForKey]);

  const handlePauseFileTransfer = useCallback(async (pending: PendingMessage) => {
    if (pending.msg_type !== "file" || pending.status !== "sending") return;
    const conversationKey = pendingConversationKeyRef.current;
    try {
      await pauseFileTransfer(pending.clientMsgId);
      updatePendingMessagesForKey(conversationKey, (prev) => prev.map((p) =>
        p.id === pending.id ? { ...p, status: "paused", speed: 0 } : p
      ));
    } catch (error) {
      updatePendingMessagesForKey(conversationKey, (prev) => prev.map((p) =>
        p.id === pending.id ? { ...p, status: "failed", error: String(error) } : p
      ));
    }
  }, [updatePendingMessagesForKey]);

  const handleResumeFileTransfer = useCallback(async (pending: PendingMessage) => {
    if (pending.msg_type !== "file" || pending.status !== "paused") return;
    const conversationKey = pendingConversationKeyRef.current;
    try {
      await resumeFileTransfer(pending.clientMsgId);
      updatePendingMessagesForKey(conversationKey, (prev) => prev.map((p) =>
        p.id === pending.id ? { ...p, status: "sending" } : p
      ));
    } catch (error) {
      updatePendingMessagesForKey(conversationKey, (prev) => prev.map((p) =>
        p.id === pending.id ? { ...p, status: "failed", error: String(error) } : p
      ));
    }
  }, [updatePendingMessagesForKey]);

  const handleCancelFileTransfer = useCallback(async (pending: PendingMessage) => {
    if (pending.msg_type !== "file" || (pending.status !== "sending" && pending.status !== "paused")) return;
    const conversationKey = pendingConversationKeyRef.current;
    updatePendingMessagesForKey(conversationKey, (prev) => prev.filter((p) => p.id !== pending.id));
    try {
      await cancelFileTransfer(pending.clientMsgId);
    } catch {
      // The background task may have completed between the click and command dispatch.
    }
  }, [updatePendingMessagesForKey]);

  const retrySticker = useCallback(async (pending: PendingMessage) => {
    if (!pending.file_path) return;
    const conversationKey = pendingConversationKeyRef.current;
    const name = pending.file_name || pending.file_path.replace(/\\/g, "/").split("/").pop() || "sticker";
    updatePendingMessagesForKey(conversationKey, (prev) => prev.filter((p) => p.id !== pending.id));
    const tempId = ++pendingId;
    const newClientMsgId = generateClientMsgId();
    updatePendingMessagesForKey(conversationKey, (prev) => [...prev, {
      ...pending,
      id: tempId,
      clientMsgId: newClientMsgId,
      file_name: name,
      createdAt: Date.now(),
      status: "sending",
      error: undefined,
    }]);
    if (!isGroup && !peer?.online) {
      updatePendingMessagesForKey(conversationKey, (prev) => prev.map((p) =>
        p.id === tempId ? { ...p, status: "failed", error: "对方离线" } : p
      ));
      return;
    }
    try {
      await onSendSticker(pending.file_path, newClientMsgId);
    } catch (e) {
      updatePendingMessagesForKey(conversationKey, (prev) => prev.map((p) =>
        p.id === tempId ? { ...p, status: "failed", error: String(e) } : p
      ));
    }
  }, [isGroup, peer?.online, onSendSticker, updatePendingMessagesForKey]);

  const sendSticker = useCallback(async (filePath: string) => {
    if (!peer) return;
    setShowEmoji(false);
    nearBottomRef.current = true;
    const conversationKey = pendingConversationKeyRef.current;
    const name = filePath.replace(/\\/g, "/").split("/").pop() || "sticker";
    const tempId = ++pendingId;
    const clientMsgId = generateClientMsgId();

    updatePendingMessagesForKey(conversationKey, (prev) => [...prev, {
      id: tempId,
      clientMsgId,
      content: "[表情]",
      msg_type: "sticker",
      file_name: name,
      file_path: filePath,
      createdAt: Date.now(),
      status: "sending",
    }]);
    if (!isGroup && !peer.online) {
      updatePendingMessagesForKey(conversationKey, (prev) => prev.map((p) =>
        p.id === tempId ? { ...p, status: "failed", error: "对方离线" } : p
      ));
      return;
    }
    try {
      await onSendSticker(filePath, clientMsgId);
    } catch (e) {
      updatePendingMessagesForKey(conversationKey, (prev) => prev.map((p) =>
        p.id === tempId ? { ...p, status: "failed", error: String(e) } : p
      ));
    }
  }, [peer, isGroup, onSendSticker, updatePendingMessagesForKey]);

  const sendText = useCallback(async () => {
    const trimmed = inputText.trim();
    if (!trimmed || !peer) return;
    setInputText("");
    nearBottomRef.current = true;
    if (inputRef.current) {
      inputRef.current.style.height = "auto";
    }
    const conversationKey = pendingConversationKeyRef.current;

    const tempId = ++pendingId;
    const clientMsgId = generateClientMsgId();

    const temp: PendingMessage = {
      id: tempId,
      clientMsgId,
      content: trimmed,
      msg_type: "text",
      status: "sending"
    };
    updatePendingMessagesForKey(conversationKey, (prev) => [...prev, temp]);

    try {
      await onSendMessage(trimmed, clientMsgId);
      // 不需要手动清理，useEffect 会自动处理
    } catch (e) {
      updatePendingMessagesForKey(conversationKey, (prev) => prev.map((p) =>
        p.id === tempId ? { ...p, status: "failed", error: String(e) } : p
      ));
    }
  }, [inputText, peer, onSendMessage, updatePendingMessagesForKey]);

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      sendText();
    }
  };

  const handleInputChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    setInputText(e.target.value);
    const el = e.target;
    el.style.height = "auto";
    el.style.height = Math.min(el.scrollHeight, 120) + "px";
  };

  const sendFileToPeer = useCallback(async (file: File) => {
    if (!peer) return;
    nearBottomRef.current = true;
    const conversationKey = pendingConversationKeyRef.current;

    const tempId = ++pendingId;
    const clientMsgId = generateClientMsgId();

    const temp: PendingMessage = {
      id: tempId,
      clientMsgId,
      content: `📎 ${file.name}`,
      msg_type: "file",
      file_name: file.name,
      status: "sending",
    };
    updatePendingMessagesForKey(conversationKey, (prev) => [...prev, temp]);

    try {
      // @ts-expect-error Tauri adds path property on drag events
      const filePath: string = file.path;
      if (filePath) {
        // attach the real path so retries can use it and so pending matches final message
        updatePendingMessagesForKey(conversationKey, (prev) => prev.map((p) => p.id === tempId ? { ...p, file_path: filePath } : p));
        onSendFile(filePath, clientMsgId).catch((e) => {
          updatePendingMessagesForKey(conversationKey, (prev) => prev.map((p) =>
            p.id === tempId ? { ...p, status: "failed", error: String(e) } : p
          ));
        });
        return;
      }
    } catch {
      // no path
    }

    try {
      const savedPath = await readFileAndSave(file);
      // Update pending entry to use the saved temp filename (it has a timestamp prefix)
      const savedName = savedPath.replace(/\\/g, "/").split("/").pop() || file.name;
      updatePendingMessagesForKey(conversationKey, (prev) => prev.map((p) => p.id === tempId ? { ...p, file_name: savedName, file_path: savedPath, file_size: file.size } : p));
      onSendFile(savedPath, clientMsgId).catch((e) => {
        updatePendingMessagesForKey(conversationKey, (prev) => prev.map((p) =>
          p.id === tempId ? { ...p, status: "failed", error: String(e) } : p
        ));
      });
    } catch (e) {
      updatePendingMessagesForKey(conversationKey, (prev) => prev.map((p) =>
        p.id === tempId ? { ...p, status: "failed", error: String(e) } : p
      ));
    }
  }, [peer, onSendFile, updatePendingMessagesForKey]);

  const handlePaste = useCallback(async (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
    const items = e.clipboardData.items;
    for (let i = 0; i < items.length; i++) {
      const item = items[i];
      if (item.kind === "file" && item.type.startsWith("image/")) {
        e.preventDefault();
        const file = item.getAsFile();
        if (file) sendFileToPeer(file);
        return;
      }
    }
  }, [sendFileToPeer]);

  // Tauri native file-drop events (HTML5 dataTransfer.files is empty in Tauri webview)
  useEffect(() => {
    if (!peerId) return;
    let unlistenHover: (() => void) | undefined;
    let unlistenDrop: (() => void) | undefined;
    let unlistenCancelled: (() => void) | undefined;

    import("@tauri-apps/api/event").then(({ listen }) => {
      listen("tauri://file-drop-hover", () => {
        showDragOverlay();
      }).then((fn) => { unlistenHover = fn; });

      listen("tauri://file-drop-cancelled", () => {
        hideDragOverlay();
      }).then((fn) => { unlistenCancelled = fn; });

      listen("tauri://file-drop", (event) => {
        hideDragOverlay();
        const paths = event.payload as string[];
        for (const filePath of paths) {
          const name = filePath.replace(/\\/g, "/").split("/").pop() || "file";
          nearBottomRef.current = true;
          const conversationKey = pendingConversationKeyRef.current;
          const tempId = ++pendingId;
          const clientMsgId = generateClientMsgId();
          updatePendingMessagesForKey(conversationKey, (prev) => [...prev, {
            id: tempId, clientMsgId, content: `📎 ${name}`, msg_type: "file", file_name: name, file_path: filePath, status: "sending",
          }]);
          onSendFile(filePath, clientMsgId).catch((e) => {
            updatePendingMessagesForKey(conversationKey, (prev) => prev.map((p) =>
              p.id === tempId ? { ...p, status: "failed", error: String(e) } : p
            ));
          });
        }
      }).then((fn) => { unlistenDrop = fn; });
    });

    return () => {
      unlistenHover?.();
      unlistenDrop?.();
      unlistenCancelled?.();
    };
  }, [hideDragOverlay, peerId, onSendFile, showDragOverlay, updatePendingMessagesForKey]);

  const handlePickFile = async () => {
    const selected = await open({ multiple: true });
    if (!selected) return;
    const paths = Array.isArray(selected) ? selected : [selected];
    for (const filePath of paths) {
      const name = filePath.replace(/\\/g, "/").split("/").pop() || "file";
      nearBottomRef.current = true;
      const conversationKey = pendingConversationKeyRef.current;
      const tempId = ++pendingId;
      const clientMsgId = generateClientMsgId();
      updatePendingMessagesForKey(conversationKey, (prev) => [...prev, {
        id: tempId, clientMsgId, content: `📎 ${name}`, msg_type: "file", file_name: name, file_path: filePath, status: "sending",
      }]);
      onSendFile(filePath, clientMsgId).catch((e) => {
        updatePendingMessagesForKey(conversationKey, (prev) => prev.map((p) =>
          p.id === tempId ? { ...p, status: "failed", error: String(e) } : p
        ));
      });
    }
  };

  const handleFileChange = useCallback((e: React.ChangeEvent<HTMLInputElement>) => {
    if (!e.target.files || !peer) return;
    for (let i = 0; i < e.target.files.length; i++) {
      sendFileToPeer(e.target.files[i]);
    }
    e.target.value = "";
  }, [peer, sendFileToPeer]);

  const reportGroupActionError = useCallback(async (error: unknown) => {
    const text = String(error || "操作失败，请重试");
    setGroupPanelError(text);
    await showDialogMessage(text, {
      title: "群操作失败",
      type: "error",
      okLabel: "确定",
    }).catch(() => {});
  }, []);

  const handleRenameGroup = useCallback(async () => {
    if (!groupInfo || groupActionBusy) return;
    const nextName = groupNameEdit.trim();
    if (!nextName) {
      setGroupPanelError("群名称不能为空");
      return;
    }
    if (nextName.length > 50) {
      setGroupPanelError("群名称不能超过50个字符");
      return;
    }
    if (nextName === groupInfo.name) return;

    setGroupActionBusy("rename");
    setGroupPanelError("");
    try {
      await renameGroup(groupInfo.group_id, nextName);
      onGroupUpdated?.();
    } catch (error) {
      await reportGroupActionError(error);
    } finally {
      setGroupActionBusy("");
    }
  }, [groupActionBusy, groupInfo, groupNameEdit, onGroupUpdated, reportGroupActionError]);

  const handleInviteMember = useCallback(async (peerId: string) => {
    if (!groupInfo || !peerId || groupActionBusy) return;
    setGroupActionBusy("invite");
    setGroupPanelError("");
    try {
      await inviteToGroup(groupInfo.group_id, [peerId]);
      setGroupInviteQuery("");
      onGroupUpdated?.();
    } catch (error) {
      await reportGroupActionError(error);
    } finally {
      setGroupActionBusy("");
    }
  }, [groupActionBusy, groupInfo, onGroupUpdated, reportGroupActionError]);

  const handleLeaveGroup = useCallback(async () => {
    if (!groupInfo || groupActionBusy) return;
    const confirmed = await ask(`确定退出「${groupInfo.name}」吗？`, {
      title: "退出群聊",
      type: "warning",
      okLabel: "退出",
      cancelLabel: "取消",
    });
    if (!confirmed) return;

    setGroupActionBusy("leave");
    setGroupPanelError("");
    try {
      await leaveGroup(groupInfo.group_id);
      onGroupUpdated?.();
      setShowGroupPanel(false);
    } catch (error) {
      await reportGroupActionError(error);
    } finally {
      setGroupActionBusy("");
    }
  }, [groupActionBusy, groupInfo, onGroupUpdated, reportGroupActionError]);

  const handleDissolveGroup = useCallback(async () => {
    if (!groupInfo || groupActionBusy) return;
    const confirmed = await ask(`确定解散「${groupInfo.name}」吗？该群会从所有成员列表中移除。`, {
      title: "解散群聊",
      type: "warning",
      okLabel: "解散",
      cancelLabel: "取消",
    });
    if (!confirmed) return;

    setGroupActionBusy("dissolve");
    setGroupPanelError("");
    try {
      await dissolveGroup(groupInfo.group_id);
      onGroupUpdated?.();
      setShowGroupPanel(false);
    } catch (error) {
      await reportGroupActionError(error);
    } finally {
      setGroupActionBusy("");
    }
  }, [groupActionBusy, groupInfo, onGroupUpdated, reportGroupActionError]);

  if (!peer) {
    return (
      <div className="flex-1 flex flex-col items-center justify-center bg-gray-800 text-gray-400">
        <svg className="w-20 h-20 mb-4 opacity-30" fill="none" viewBox="0 0 24 24" stroke="currentColor">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1} d="M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z" />
        </svg>
        <p className="text-lg font-medium">欢迎使用 Echo</p>
        <p className="text-sm mt-2">从左侧选择一个联系人开始聊天</p>
      </div>
    );
  }

  const allItems: (ChatMessage | PendingMessage)[] = [
    ...messages.filter((m) => m.msg_type !== "file_chunk" && m.msg_type !== "file_end"),
    ...pendingMessages,
  ].sort((a, b) => {
    const getTime = (item: ChatMessage | PendingMessage) => {
      if ("timestamp" in item) return new Date(item.timestamp).getTime();
      return Date.now();
    };
    return getTime(a) - getTime(b);
  });

  const searchHits = getTextSearchHits(messages, searchQuery);
  const totalSearchHits = searchHits.length;
  const clampedSearchIndex = totalSearchHits > 0 ? Math.min(searchIndex, totalSearchHits - 1) : 0;
  const currentSearchHit = searchHits[clampedSearchIndex];
  const searchMatchIds = new Set(searchHits.map((hit) => hit.messageId));
  const scrollToSearchHit = (idx: number) => {
    const hit = searchHits[idx];
    if (!hit) return;
    requestAnimationFrame(() => {
      const hitEl = messagesContainerRef.current?.querySelector<HTMLElement>(`[data-search-hit-id="${hit.id}"]`);
      if (hitEl) {
        hitEl.scrollIntoView({ behavior: "smooth", block: "center", inline: "nearest" });
        return;
      }
      messageRefs.current.get(hit.messageId)?.scrollIntoView({ behavior: "smooth", block: "center" });
    });
  };
  const groupMembers = groupInfo?.members ?? [];
  const memberQuery = groupMemberQuery.trim().toLowerCase();
  const visibleGroupMembers = memberQuery
    ? groupMembers.filter((member) => {
        const displayName = member.peer_id === myId ? (myName || member.username || "我") : (member.username || member.peer_id);
        return [
          displayName,
          member.username,
          member.department,
          member.peer_id,
          member.is_online ? "在线" : "离线",
          groupInfo?.creator_id === member.peer_id ? "群主" : "",
          member.peer_id === myId ? "我" : "",
        ].some((value) => value.toLowerCase().includes(memberQuery));
      })
    : groupMembers;
  const inviteCandidates = peers.filter((candidate) =>
    candidate.id !== myId && !groupMembers.some((member) => member.peer_id === candidate.id)
  );
  const inviteQuery = groupInviteQuery.trim().toLowerCase();
  const visibleInviteCandidates = inviteQuery
    ? inviteCandidates.filter((candidate) =>
        [
          candidate.username,
          candidate.department,
          candidate.id,
          candidate.ip,
          `${candidate.ip}:${candidate.port}`,
          candidate.online ? "在线" : "离线",
        ].some((value) => value.toLowerCase().includes(inviteQuery))
      )
    : inviteCandidates;
  const historyConversationKey = isGroup
    ? `group:${groupId ?? peer.id}`
    : `contact:${peer.ip && peer.port ? `${peer.ip}:${peer.port}` : peer.id}`;

  return (
    <div className="flex-1 flex h-full min-w-0">
    <div
      className="chat-surface flex-1 flex flex-col bg-gray-800 h-full relative min-w-0"
      onDragEnter={(e) => {
        e.preventDefault();
        showDragOverlay();
      }}
      onDragOver={(e) => {
        e.preventDefault();
        showDragOverlay();
      }}
      onDrop={(e) => {
        e.preventDefault();
        hideDragOverlay();
      }}
    >
      {isDragging && (
        <div className="pointer-events-none absolute inset-0 z-50 bg-indigo-600/20 border-2 border-dashed border-indigo-400 flex items-center justify-center backdrop-blur-sm">
          <div className="text-center">
            <svg className="w-16 h-16 mx-auto text-indigo-300 mb-3" fill="none" viewBox="0 0 24 24" stroke="currentColor">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M7 16a4 4 0 01-.88-7.903A5 5 0 1115.9 6L16 6a5 5 0 011 9.9M15 13l-3-3m0 0l-3 3m3-3v12" />
            </svg>
            <p className="text-white text-lg font-medium">拖放文件以发送</p>
            <p className="text-indigo-200 text-sm mt-1">发送给 {peer.username}</p>
          </div>
        </div>
      )}

      <div className="chat-header flex items-center gap-3 px-5 py-3 bg-gray-900/80 border-b border-gray-700 backdrop-blur">
        {isGroup ? (
          <div className="relative flex-shrink-0">
            <div className="w-9 h-9 rounded-full flex items-center justify-center text-base text-white bg-indigo-700">👥</div>
          </div>
        ) : (
          <AvatarPreviewTrigger name={peer.username} src={peer.avatar_path} size="md" online={peer.online} />
        )}
        <div className="flex-1 min-w-0">
          <p className="text-white text-sm font-semibold truncate">{peer.username}</p>
          <p className="text-xs text-gray-400">{isGroup ? "群聊" : (peer.online ? `${peer.ip}:${peer.port}` : "离线")}</p>
        </div>
        <button
          type="button"
          onClick={() => {
            setForwardModal(null);
            exitSelectMode();
            setShowSearch(false);
            setSearchQuery("");
            setSearchIndex(0);
            setShowHistory(true);
          }}
          className={`flex-shrink-0 h-8 px-2.5 rounded-lg flex items-center gap-1.5 text-xs hover:bg-gray-700 ${showHistory ? "bg-gray-700 text-white" : "text-gray-400 hover:text-white"}`}
          title="聊天记录"
        >
          <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01" />
          </svg>
          <span>聊天记录</span>
        </button>
        <button
          onClick={() => { setShowHistory(false); setShowSearch((v) => !v); setSearchQuery(""); setSearchIndex(0); }}
          className="flex-shrink-0 w-8 h-8 rounded-lg hover:bg-gray-700 flex items-center justify-center text-gray-400 hover:text-white"
          title="搜索消息"
        >
          <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
          </svg>
        </button>
        {isGroup && (
          <button
            onClick={() => {
              setShowGroupPanel((v) => {
                const next = !v;
                if (next) {
                  setGroupNameEdit(groupInfo?.name ?? "");
                  setGroupPanelError("");
                  setGroupMemberQuery("");
                  setGroupInviteQuery("");
                }
                return next;
              });
            }}
            className={`flex-shrink-0 w-8 h-8 rounded-lg flex items-center justify-center text-gray-400 hover:text-white hover:bg-gray-700 ${showGroupPanel ? "bg-gray-700 text-white" : ""}`}
            title="群信息"
          >
            <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M17 20h5v-2a3 3 0 00-5.356-1.857M17 20H7m10 0v-2c0-.656-.126-1.283-.356-1.857M7 20H2v-2a3 3 0 015.356-1.857M7 20v-2c0-.656.126-1.283.356-1.857m0 0a5.002 5.002 0 019.288 0M15 7a3 3 0 11-6 0 3 3 0 016 0z" />
            </svg>
          </button>
        )}
      </div>
      {showHistory ? (
        <HistorySearchView
          key={`${historyConversationKey}:${historySearchRequest?.nonce ?? "manual"}`}
          peer={peer}
          myId={myId}
          isGroup={isGroup}
          groupId={groupId}
          initialSearchRequest={historySearchRequest}
          onJumpToMessage={handleJumpToHistoryMessage}
          onClose={() => setShowHistory(false)}
        />
      ) : (
      <>
      {showSearch && (() => {
        return (
          <div className="flex items-center gap-2 px-4 py-2 bg-gray-900/60 border-b border-gray-700">
            <input
              autoFocus
              value={searchQuery}
              onChange={(e) => { setSearchQuery(e.target.value); setSearchIndex(0); }}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  if (totalSearchHits === 0) return;
                  const next = (clampedSearchIndex + 1) % totalSearchHits;
                  setSearchIndex(next);
                  scrollToSearchHit(next);
                }
                if (e.key === "Escape") { setShowSearch(false); setSearchQuery(""); }
              }}
              placeholder="搜索消息..."
              className="flex-1 bg-gray-700 text-white text-sm rounded-lg px-3 py-1.5 outline-none focus:ring-1 focus:ring-indigo-500 placeholder-gray-400"
            />
            {searchQuery && (
              <span className="text-xs text-gray-400 flex-shrink-0">{totalSearchHits > 0 ? `${clampedSearchIndex + 1}/${totalSearchHits}` : "无结果"}</span>
            )}
            <button disabled={totalSearchHits === 0} onClick={() => { const prev = (clampedSearchIndex - 1 + totalSearchHits) % totalSearchHits; setSearchIndex(prev); scrollToSearchHit(prev); }} className="text-gray-400 hover:text-white disabled:opacity-30">
              <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 15l7-7 7 7" /></svg>
            </button>
            <button disabled={totalSearchHits === 0} onClick={() => { const next = (clampedSearchIndex + 1) % totalSearchHits; setSearchIndex(next); scrollToSearchHit(next); }} className="text-gray-400 hover:text-white disabled:opacity-30">
              <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 9l-7 7-7-7" /></svg>
            </button>
          </div>
        );
      })()}

      <div ref={messagesContainerRef} onScroll={handleScroll} className="flex-1 overflow-y-auto py-4">
        {loadingMessages ? (
          <div className="flex flex-col items-center justify-center h-full text-gray-500">
            <div className="w-8 h-8 border-2 border-indigo-500 border-t-transparent rounded-full animate-spin mb-3" />
            <p className="text-sm">正在加载聊天记录...</p>
          </div>
        ) : allItems.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-full text-gray-500">
            <p className="text-sm">{isGroup ? "暂无群消息" : "暂无消息"}</p>
            <p className="text-xs mt-1">
              {isGroup
                ? "和群成员开始讨论吧"
                : peer.online
                  ? `向 ${peer.username} 发送第一条消息吧`
                  : `${peer.username} 当前离线，文本消息会在对方上线后继续尝试发送`}
            </p>
          </div>
        ) : (() => {
          const highlightedId = currentSearchHit?.messageId;
          let lastDateLabel = "";
          return allItems.map((item) => {
            const elements: React.ReactNode[] = [];
            if ("timestamp" in item) {
              const label = formatDateLabel(item.timestamp);
              if (label && label !== lastDateLabel) {
                lastDateLabel = label;
                elements.push(<DateDivider key={`date-${item.id}`} date={label} />);
              }
            }
            if ("status" in item) {
              const isPendingSticker = item.msg_type === "sticker" && !!item.file_path;
              const pendingStatusText = getPendingStatusText(item);
              elements.push(
                <div key={`pending-${item.id}`} className="message-row flex justify-end mb-3 px-4">
                  <div className="message-stack flex flex-col items-end">
                    <div className={`${isPendingSticker ? "overflow-hidden rounded-xl" : "message-bubble-shell rounded-2xl px-4 py-2.5 rounded-br-md"} ${
                      item.status === "failed"
                        ? isPendingSticker ? "ring-1 ring-red-500/70" : "bg-red-600/30 border border-red-500/50"
                        : isPendingSticker ? "" : "message-bubble-own bg-indigo-600/50"
                    } text-white`}>
                      {isPendingSticker ? (
                        <div className="w-32 h-32">
                          <EmojiThumb path={item.file_path!} />
                        </div>
                      ) : item.msg_type === "file" ? (
                        <div className="flex items-center gap-2">
                          <svg className="w-5 h-5 flex-shrink-0 opacity-80" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 10v6m0 0l-3-3m3 3l3-3m2 8H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z" />
                          </svg>
                          <p className="message-file-name truncate" title={item.file_name || "文件"}>{item.file_name || "文件"}</p>
                        </div>
                      ) : (
                        <p className="message-text">{item.content}</p>
                      )}
                    </div>
                    {item.msg_type === "file" && (item.status === "sending" || item.status === "paused") && item.progress !== undefined && (
                      <div className="w-full bg-gray-700 rounded-full h-1.5 mt-1.5">
                        <div className={`${item.status === "paused" ? "bg-yellow-400" : "bg-indigo-400"} h-1.5 rounded-full transition-all`} style={{ width: `${item.progress}%` }} />
                      </div>
                    )}
                    <div className="flex items-center gap-2 mt-1">
                      <span className="message-meta max-w-[22rem] truncate" title={item.error || pendingStatusText}>
                        {pendingStatusText}
                      </span>
                      {item.msg_type === "file" && item.status === "sending" && (
                        <button
                          onClick={() => handlePauseFileTransfer(item)}
                          className="text-[10px] text-yellow-300 hover:text-yellow-200"
                        >暂停</button>
                      )}
                      {item.msg_type === "file" && item.status === "paused" && (
                        <button
                          onClick={() => handleResumeFileTransfer(item)}
                          className="text-[10px] text-indigo-300 hover:text-indigo-200"
                        >继续</button>
                      )}
                      {item.msg_type === "file" && (item.status === "sending" || item.status === "paused") && (
                        <button
                          onClick={() => handleCancelFileTransfer(item)}
                          className="text-[10px] text-red-300 hover:text-red-200"
                        >取消</button>
                      )}
                      {item.status === "failed" && (
                        <button
                          onClick={() => { if (item.msg_type === "file") retryFile(item); else if (item.msg_type === "sticker") retrySticker(item); else retryText(item); }}
                          className="text-[10px] text-indigo-400 hover:text-indigo-300"
                        >重试</button>
                      )}
                    </div>
                  </div>
                </div>
              );
            } else {
              elements.push(
                <div key={item.id} ref={(el) => { if (el) messageRefs.current.set(item.id, el); else messageRefs.current.delete(item.id); }}>
                  <MessageBubble
                    message={item}
                    isOwn={item.sender_id === myId}
                    showSender={isGroup}
                    highlighted={(searchMatchIds.has(item.id) && item.id === highlightedId) || contextHighlightId === item.id}
                    searchQuery={searchMatchIds.has(item.id) ? searchQuery : ""}
                    activeSearchHitId={item.id === highlightedId ? currentSearchHit?.id : undefined}
                    selectMode={selectMode}
                    selected={selectedIds.has(item.id)}
                    onToggleSelect={handleToggleSelect}
                    onStartForward={handleStartForward}
                    onAddSticker={handleAddStickerFromMessage}
                  />
                </div>
              );
            }
            return elements;
          });
        })()}
        <div ref={messagesEndRef} />
      </div>
      {selectMode && (
        <div className="flex items-center gap-2 px-4 py-2.5 bg-gray-900 border-t border-gray-700">
          <span className="text-sm text-gray-300 flex-1">已选 {selectedIds.size} 条</span>
          <button onClick={exitSelectMode} className="px-3 py-1.5 text-sm text-gray-400 hover:text-white rounded-lg hover:bg-gray-700">取消</button>
          <button
            disabled={selectedIds.size === 0}
            onClick={() => openForwardModal("individual")}
            className="px-3 py-1.5 text-sm bg-gray-700 hover:bg-gray-600 text-white rounded-lg disabled:opacity-40"
          >逐条转发</button>
          <button
            disabled={selectedIds.size === 0}
            onClick={() => openForwardModal("merged")}
            className="px-3 py-1.5 text-sm bg-indigo-600 hover:bg-indigo-500 text-white rounded-lg disabled:opacity-40"
          >合并转发</button>
        </div>
      )}
      {forwardModal && (
        <ForwardModal
          messages={forwardModal.messages}
          mode={forwardModal.mode}
          peers={peers}
          groups={groups}
          myId={myId}
          onClose={() => { setForwardModal(null); exitSelectMode(); }}
        />
      )}

      <div className="chat-composer px-4 py-3 border-t border-gray-700 bg-gray-900/50">
        <div className="flex items-end gap-2">
          <div ref={emojiPopoverRef} className="relative flex-shrink-0">
            <button onClick={() => setShowEmoji(!showEmoji)} className="w-10 h-10 rounded-xl bg-gray-700 hover:bg-gray-600 transition-colors flex items-center justify-center" title="表情">
              <span className="text-lg">😀</span>
            </button>
            {showEmoji && (
              <div className="absolute bottom-full right-0 mb-2 bg-gray-800 border border-gray-600 rounded-xl shadow-2xl z-50 w-80 overflow-hidden">
                <div className="p-3">
                  {emojiTab === "custom" ? (
                    <>
                      <div className="grid grid-cols-5 gap-2 max-h-56 overflow-y-auto pr-1">
                        {customEmojis.map((path) => {
                          const name = path.replace(/\\/g, "/").split("/").pop() || "emoji";
                          return (
                            <div key={path} className="custom-emoji-tile group aspect-square">
                              <button
                                type="button"
                                onClick={() => sendSticker(path)}
                                className="w-full h-full rounded-lg hover:bg-gray-700 overflow-hidden border border-gray-700 bg-gray-900/60"
                                title={name}
                              >
                                <EmojiThumb path={path} />
                              </button>
                              <button
                                type="button"
                                disabled={deletingEmoji === path}
                                onClick={(e) => { e.stopPropagation(); handleDeleteEmoji(path); }}
                                className="custom-emoji-delete"
                                title="删除表情"
                                aria-label={`删除表情 ${name}`}
                              >
                                <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2.5} d="M6 6l12 12M18 6L6 18" />
                                </svg>
                              </button>
                            </div>
                          );
                        })}
                        <button
                          onClick={handleAddEmoji}
                          className="aspect-square flex items-center justify-center text-gray-400 hover:bg-gray-700 hover:text-white rounded-lg border border-dashed border-gray-600 text-2xl"
                          title="添加自定义表情"
                        >
                          +
                        </button>
                      </div>
                      {customEmojis.length === 0 && (
                        <button onClick={handleAddEmoji} className="mt-3 w-full py-2 text-xs rounded-lg bg-gray-700 hover:bg-gray-600 text-gray-200">
                          上传表情
                        </button>
                      )}
                    </>
                  ) : (
                    <div className="grid grid-cols-10 gap-1 max-h-56 overflow-y-auto">
                      {EMOJIS.map((emoji) => (
                      <button
                        key={emoji}
                        onClick={() => {
                          const el = inputRef.current;
                          if (el) {
                            const start = el.selectionStart ?? el.value.length;
                            const end = el.selectionEnd ?? el.value.length;
                            const before = el.value.slice(0, start);
                            const after = el.value.slice(end);
                            setInputText(before + emoji + after);
                            requestAnimationFrame(() => {
                              el.selectionStart = el.selectionEnd = start + emoji.length;
                              el.focus();
                            });
                          }
                          setShowEmoji(false);
                        }}
                        className="w-7 h-7 flex items-center justify-center text-base hover:bg-gray-600 rounded"
                      >
                        {emoji}
                      </button>
                      ))}
                    </div>
                  )}
                </div>
                <div className="flex border-t border-gray-700">
                  <button
                    onClick={() => setEmojiTab("default")}
                    className={`flex-1 py-2 text-xs font-medium ${emojiTab === "default" ? "text-indigo-300 border-t-2 border-indigo-400" : "text-gray-400 hover:text-gray-200 border-t-2 border-transparent"}`}
                  >
                    默认
                  </button>
                  <button
                    onClick={() => setEmojiTab("custom")}
                    className={`flex-1 py-2 text-xs font-medium ${emojiTab === "custom" ? "text-indigo-300 border-t-2 border-indigo-400" : "text-gray-400 hover:text-gray-200 border-t-2 border-transparent"}`}
                  >
                    自定义
                  </button>
                </div>
              </div>
            )}
          </div>
          <button onClick={handlePickFile} className="flex-shrink-0 w-10 h-10 rounded-xl bg-gray-700 hover:bg-gray-600 transition-colors flex items-center justify-center" title="发送文件">
            <svg className="w-5 h-5 text-gray-300" fill="none" viewBox="0 0 24 24" stroke="currentColor">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15.172 7l-6.586 6.586a2 2 0 102.828 2.828l6.414-6.586a4 4 0 00-5.656-5.656l-6.415 6.585a6 6 0 108.486 8.486L20.5 13" />
            </svg>
          </button>
          <input ref={fileInputRef} type="file" onChange={handleFileChange} style={{ position: "absolute", left: "-9999px", top: "-9999px" }} multiple />
          <textarea ref={inputRef} value={inputText} onChange={handleInputChange} onKeyDown={handleKeyDown} onPaste={handlePaste}
            placeholder={peer.online ? `发送消息给 ${peer.username}...` : "对方离线，消息将在上线后发送"}
            rows={1} className="flex-1 bg-gray-700 text-white text-sm rounded-xl px-4 py-2.5 outline-none focus:ring-2 focus:ring-indigo-500 placeholder-gray-400 resize-none max-h-[120px]"
          />
          <button onClick={sendText} disabled={!inputText.trim()}
            className="flex-shrink-0 w-10 h-10 rounded-xl bg-indigo-600 hover:bg-indigo-500 disabled:opacity-40 disabled:hover:bg-indigo-600 transition-colors flex items-center justify-center"
          >
            <svg className="w-5 h-5 text-white" fill="none" viewBox="0 0 24 24" stroke="currentColor">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 19l9 2-9-18-9 18 9-2zm0 0v-8" />
            </svg>
          </button>
        </div>
        <p className="text-[10px] text-gray-600 mt-1.5 ml-1">Enter 发送 · Shift+Enter 换行 · Ctrl+V 粘贴图片 · 拖拽/📎 发送文件</p>
      </div>
      </>
      )}
    </div>
    {/* Group info panel */}
    {isGroup && showGroupPanel && groupInfo && (
      <div className="w-72 flex-shrink-0 bg-gray-900 border-l border-gray-700 flex flex-col h-full overflow-y-auto">
        <div className="px-4 py-3 border-b border-gray-700 flex items-center justify-between">
          <span className="text-sm font-semibold text-white">群信息</span>
          <button onClick={() => setShowGroupPanel(false)} className="text-gray-500 hover:text-gray-300 text-lg leading-none">×</button>
        </div>
        <div className="px-4 py-3 space-y-4 flex-1">
          {groupPanelError ? (
            <div className="rounded-lg border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-200">
              {groupPanelError}
            </div>
          ) : null}
          {/* Group name */}
          <div>
            <p className="text-xs text-gray-400 mb-1">群名称</p>
            <div className="flex gap-1">
              <input
                value={groupNameEdit}
                maxLength={50}
                disabled={!!groupActionBusy}
                onChange={(e) => {
                  setGroupNameEdit(e.target.value);
                  if (groupPanelError) setGroupPanelError("");
                }}
                className="flex-1 bg-gray-800 border border-gray-600 rounded px-2 py-1 text-sm text-gray-200 outline-none focus:border-indigo-500"
              />
              <button
                onClick={handleRenameGroup}
                disabled={!!groupActionBusy || !groupNameEdit.trim() || groupNameEdit.trim() === groupInfo.name}
                className="px-2 py-1 text-xs bg-indigo-600 hover:bg-indigo-500 disabled:opacity-40 rounded text-white"
              >{groupActionBusy === "rename" ? "保存中" : "保存"}</button>
            </div>
            <p className="mt-1 text-[10px] text-gray-500">{groupNameEdit.length}/50</p>
          </div>
          {/* Members */}
          <div>
            <div className="flex items-center justify-between gap-2 mb-2">
              <p className="text-xs text-gray-400">成员 ({visibleGroupMembers.length}/{groupMembers.length}人)</p>
              {memberQuery ? (
                <button
                  type="button"
                  onClick={() => setGroupMemberQuery("")}
                  className="text-[10px] text-indigo-300 hover:text-indigo-200"
                >
                  清除
                </button>
              ) : null}
            </div>
            <div className="relative mb-2">
              <svg className="w-3.5 h-3.5 absolute left-2.5 top-1/2 -translate-y-1/2 text-gray-500" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
              </svg>
              <input
                value={groupMemberQuery}
                onChange={(e) => setGroupMemberQuery(e.target.value)}
                placeholder="搜索成员、部门或状态"
                className="w-full bg-gray-800 border border-gray-600 rounded-lg pl-8 pr-2 py-1.5 text-xs text-gray-200 outline-none focus:border-indigo-500 placeholder-gray-500"
              />
            </div>
            <div className="space-y-1.5 max-h-56 overflow-y-auto pr-1">
              {visibleGroupMembers.map((m) => {
                const displayName = m.peer_id === myId ? (myName || m.username || "我") : (m.username || m.peer_id);
                return (
                  <div key={m.peer_id} className="flex items-center gap-2 rounded-lg px-2 py-1.5 hover:bg-gray-800/70">
                    <AvatarPreviewTrigger name={displayName} src={m.avatar_path} size="xs" online={m.peer_id === myId || m.is_online} />
                    <div className="flex-1 min-w-0">
                      <p className="text-xs text-gray-200 truncate" title={displayName}>{displayName}{m.peer_id === myId ? " (我)" : ""}</p>
                      <p className="text-[10px] text-gray-500 truncate">
                        {groupInfo.creator_id === m.peer_id ? "群主" : (m.department || "成员")}
                      </p>
                    </div>
                  </div>
                );
              })}
              {visibleGroupMembers.length === 0 ? (
                <div className="rounded-lg border border-gray-700 bg-gray-800/60 px-3 py-4 text-center text-xs text-gray-500">
                  没有匹配成员
                </div>
              ) : null}
            </div>
          </div>
          {/* Invite */}
          <div>
            <div className="flex items-center justify-between gap-2 mb-2">
              <p className="text-xs text-gray-400">邀请成员</p>
              <span className="text-[10px] text-gray-500">{inviteCandidates.length} 个可邀请</span>
            </div>
            <div className="relative mb-2">
              <svg className="w-3.5 h-3.5 absolute left-2.5 top-1/2 -translate-y-1/2 text-gray-500" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
              </svg>
              <input
                value={groupInviteQuery}
                onChange={(e) => setGroupInviteQuery(e.target.value)}
                disabled={!!groupActionBusy || inviteCandidates.length === 0}
                placeholder={inviteCandidates.length === 0 ? "没有可邀请联系人" : "搜索联系人、部门或地址"}
                className="w-full bg-gray-800 border border-gray-600 rounded-lg pl-8 pr-2 py-1.5 text-xs text-gray-200 outline-none focus:border-indigo-500 placeholder-gray-500 disabled:opacity-50"
              />
            </div>
            <div className="max-h-44 overflow-y-auto space-y-1 pr-1">
              {visibleInviteCandidates.map((candidate) => (
                <button
                  key={candidate.id}
                  type="button"
                  onClick={() => handleInviteMember(candidate.id)}
                  disabled={!!groupActionBusy}
                  className="w-full flex items-center gap-2 rounded-lg px-2 py-1.5 text-left hover:bg-gray-800 disabled:opacity-50"
                >
                  <Avatar name={candidate.username} src={candidate.avatar_path} size="xs" online={candidate.online} />
                  <div className="flex-1 min-w-0">
                    <p className="text-xs text-gray-200 truncate" title={candidate.username}>{candidate.username}</p>
                    <p className="text-[10px] text-gray-500 truncate">{candidate.department || `${candidate.ip}:${candidate.port}`}</p>
                  </div>
                  <span className="text-[10px] text-indigo-300 flex-shrink-0">{groupActionBusy === "invite" ? "邀请中" : "邀请"}</span>
                </button>
              ))}
              {visibleInviteCandidates.length === 0 ? (
                <div className="rounded-lg border border-gray-700 bg-gray-800/60 px-3 py-4 text-center text-xs text-gray-500">
                  {inviteCandidates.length === 0 ? "联系人都已在群内" : "没有匹配联系人"}
                </div>
              ) : null}
            </div>
          </div>
        </div>
        {/* Leave / Dissolve */}
        <div className="px-4 py-3 border-t border-gray-700">
          {groupInfo.creator_id !== myId ? (
            <button
              onClick={handleLeaveGroup}
              disabled={!!groupActionBusy}
              className="w-full py-2 text-sm rounded-lg bg-yellow-700/60 hover:bg-yellow-700 disabled:opacity-50 text-yellow-200"
            >{groupActionBusy === "leave" ? "退出中..." : "退出群聊"}</button>
          ) : (
            <button
              onClick={handleDissolveGroup}
              disabled={!!groupActionBusy}
              className="w-full py-2 text-sm rounded-lg bg-red-700/60 hover:bg-red-700 disabled:opacity-50 text-red-200"
            >{groupActionBusy === "dissolve" ? "解散中..." : "解散群聊"}</button>
          )}
        </div>
      </div>
    )}
    </div>
  );
}
