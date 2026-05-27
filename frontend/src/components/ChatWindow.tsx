import { useState, useRef, useEffect, useCallback } from "react";
import type { ChatMessage, Peer } from "../types";
import type { GroupInfo } from "../api";
import { MessageBubble, DateDivider, formatDateLabel, makeSearchHitId } from "./MessageBubble";
import { saveTempFile, listEmojiFiles, addEmojiFile, deleteEmojiFile, readFileBase64, sendMessage, sendMessageTyped, sendGroupMessage, sendGroupMessageTyped, renameGroup, leaveGroup, dissolveGroup, inviteToGroup } from "../api";
import type { ForwardCardData } from "./MessageBubble";
import { open } from "@tauri-apps/api/dialog";

export interface PendingMessage {
  id: number;
  content: string;
  msg_type: string;
  file_name?: string;
  file_path?: string;
  file_size?: number;
  status: "sending" | "failed" | "sent";
  error?: string;
  progress?: number; // 0-100
  speed?: number; // bytes/sec
  createdAt?: number;
}

interface ChatWindowProps {
  peer: Peer | null;
  messages: ChatMessage[];
  myId: string;
  myName?: string;
  isGroup?: boolean;
  groupId?: string | null;
  groupInfo?: GroupInfo | null;
  peers?: Peer[];
  groups?: GroupInfo[];
  onSendMessage: (content: string) => Promise<ChatMessage>;
  onSendFile: (filePath: string) => Promise<void | ChatMessage>;
  onSendSticker: (filePath: string) => Promise<ChatMessage>;
  onGroupUpdated?: () => void;
}

let pendingId = Date.now();

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

const EMOJIS = ["😀","😂","🤣","😍","🥰","😘","😜","🤪","😎","🤩","😢","😭","😤","😡","🤬","👍","👎","👏","🙌","💪","🎉","🔥","❤️","💔","💯","✅","❌","⭐","🌟","📎","📁","💡","🎵","🌹","🍕","☕","🚀","🐱","🐶","🦊","🐼","👋","🤝","🙏","💀","👻","🤖","🎂","🏆","🥇","💩"];

function EmojiThumb({ path }: { path: string }) {
  const [src, setSrc] = useState<string>("");
  useEffect(() => {
    readFileBase64(path).then((d) => setSrc(`data:${d.mime};base64,${d.base64}`)).catch(() => {});
  }, [path]);
  if (!src) return <div className="w-full h-full bg-gray-700 rounded" />;
  return <img src={src} alt="" className="w-full h-full object-contain" />;
}

function formatSpeed(bytesPerSec: number | undefined): string {
  if (!bytesPerSec || bytesPerSec === 0) return "";
  if (bytesPerSec >= 1_000_000) return `${(bytesPerSec / 1_000_000).toFixed(1)} MB/s`;
  if (bytesPerSec >= 1_000) return `${(bytesPerSec / 1_000).toFixed(0)} KB/s`;
  return `${bytesPerSec} B/s`;
}

type ForwardMode = "individual" | "merged";

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
        const card: ForwardCardData = {
          title: `${sorted[0]?.sender_name ?? ""} 等人的聊天记录`,
          items: sorted.map((m) => ({ sender: m.sender_name, content: m.content, msg_type: m.msg_type, timestamp: m.timestamp })),
        };
        const json = JSON.stringify(card);
        if (isGroup) await sendGroupMessageTyped(targetId, json, "forward_card");
        else await sendMessageTyped(targetId, json, "forward_card");
      } else {
        for (const m of sorted) {
          if (isGroup) await sendGroupMessage(targetId, m.content);
          else await sendMessage(targetId, m.content);
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
              <div className="w-8 h-8 rounded-full bg-gray-600 flex items-center justify-center text-sm font-medium text-white flex-shrink-0">
                {p.username.charAt(0).toUpperCase()}
              </div>
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

export function ChatWindow({ peer, messages, myId, myName = "", isGroup = false, groupInfo, peers = [], groups = [], onSendMessage, onSendFile, onSendSticker, onGroupUpdated }: ChatWindowProps) {
  const [inputText, setInputText] = useState("");
  const [isDragging, setIsDragging] = useState(false);
  const [pendingMessages, setPendingMessages] = useState<PendingMessage[]>([]);
  const [showEmoji, setShowEmoji] = useState(false);
  const [emojiTab, setEmojiTab] = useState<"default" | "custom">("default");
  const [customEmojis, setCustomEmojis] = useState<string[]>([]);
  const [deletingEmoji, setDeletingEmoji] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState("");
  const [showSearch, setShowSearch] = useState(false);
  const [searchIndex, setSearchIndex] = useState(0);
  const [selectMode, setSelectMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());
  const [forwardModal, setForwardModal] = useState<{ messages: ChatMessage[]; mode: ForwardMode } | null>(null);
  const [showGroupPanel, setShowGroupPanel] = useState(false);
  const [groupNameEdit, setGroupNameEdit] = useState("");
  const messageRefs = useRef<Map<number, HTMLDivElement>>(new Map());

  const exitSelectMode = useCallback(() => { setSelectMode(false); setSelectedIds(new Set()); }, []);

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
      listen<{ fileName: string; sent: number; total: number; speed: number }>("file-progress", (event) => {
        const { fileName, sent, total, speed } = event.payload;
        const pct = total > 0 ? Math.round((sent / total) * 100) : 0;
        setPendingMessages((prev) =>
          prev.map((p) =>
            p.file_name === fileName && p.msg_type === "file"
              ? { ...p, progress: pct, speed, status: pct >= 100 ? "sent" as const : p.status }
              : p
          )
        );
        // Remove pending after 2s (real message will be in DB by then)
        if (pct >= 100) {
          setTimeout(() => {
            setPendingMessages((prev) => prev.filter((p) => p.file_name !== fileName || p.msg_type !== "file"));
          }, 2000);
        }
      }).then((fn) => { unlisten = fn; });

      listen<{ fileName: string; error: string }>("file-error", (event) => {
        const { fileName, error } = event.payload;
        setPendingMessages((prev) =>
          prev.map((p) =>
            p.file_name === fileName
              ? { ...p, status: "failed", error }
              : p
          )
        );
      }).then((fn) => { unlistenError = fn; });
    });
    return () => { unlisten?.(); unlistenError?.(); };
  }, []);
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const messagesContainerRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const emojiPopoverRef = useRef<HTMLDivElement>(null);
  const nearBottomRef = useRef(true);

  const pendingScrollRef = useRef(false);

  // Clear pending when switching peers + focus input + mark pending scroll
  useEffect(() => {
    setPendingMessages([]);
    nearBottomRef.current = true;
    pendingScrollRef.current = true;
    if (peer) {
      requestAnimationFrame(() => inputRef.current?.focus());
    }
  }, [peer?.id]);

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

  // Remove pending bubbles when the real message arrives to avoid duplicates
  useEffect(() => {
    setPendingMessages((prev) => prev.filter((p) => {
      if (p.status === "failed") return true;
      return !messages.some((m) => {
        if (m.sender_id !== myId || m.msg_type !== p.msg_type) return false;
        if (p.msg_type === "file" || p.msg_type === "sticker") {
          if (p.createdAt) {
            const messageTime = new Date(m.timestamp).getTime();
            if (!Number.isNaN(messageTime) && messageTime + 500 < p.createdAt) return false;
          }
          return (
            (p.file_path && m.file_path && p.file_path === m.file_path) ||
            (p.file_name && m.file_name && p.file_name === m.file_name)
          );
        }
        return m.content === p.content;
      });
    }));
  }, [messages, myId]);

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
    setPendingMessages((prev) => prev.filter((p) => p.id !== pending.id));
    try {
      await onSendMessage(pending.content);
    } catch {
      setPendingMessages((prev) => [...prev, {
        ...pending,
        id: ++pendingId,
        status: "failed",
        error: "重试失败",
      }]);
    }
  }, [onSendMessage]);

  const retryFile = useCallback(async (pending: PendingMessage) => {
    if (!pending.file_path) return;
    setPendingMessages((prev) => prev.filter((p) => p.id !== pending.id));
    try {
      await onSendFile(pending.file_path);
    } catch {
      setPendingMessages((prev) => [...prev, {
        ...pending,
        id: ++pendingId,
        status: "failed",
        error: "重试失败",
      }]);
    }
  }, [onSendFile]);

  const retrySticker = useCallback(async (pending: PendingMessage) => {
    if (!pending.file_path) return;
    const name = pending.file_name || pending.file_path.replace(/\\/g, "/").split("/").pop() || "sticker";
    setPendingMessages((prev) => prev.filter((p) => p.id !== pending.id));
    const tempId = ++pendingId;
    setPendingMessages((prev) => [...prev, {
      ...pending,
      id: tempId,
      file_name: name,
      createdAt: Date.now(),
      status: "sending",
      error: undefined,
    }]);
    if (!isGroup && !peer?.online) {
      setPendingMessages((prev) => prev.map((p) =>
        p.id === tempId ? { ...p, status: "failed", error: "对方离线" } : p
      ));
      return;
    }
    try {
      await onSendSticker(pending.file_path);
    } catch (e) {
      setPendingMessages((prev) => prev.map((p) =>
        p.id === tempId ? { ...p, status: "failed", error: String(e) } : p
      ));
    }
  }, [isGroup, peer?.online, onSendSticker]);

  const sendSticker = useCallback(async (filePath: string) => {
    if (!peer) return;
    setShowEmoji(false);
    nearBottomRef.current = true;
    const name = filePath.replace(/\\/g, "/").split("/").pop() || "sticker";
    const tempId = ++pendingId;
    setPendingMessages((prev) => [...prev, {
      id: tempId,
      content: "[表情]",
      msg_type: "sticker",
      file_name: name,
      file_path: filePath,
      createdAt: Date.now(),
      status: "sending",
    }]);
    if (!isGroup && !peer.online) {
      setPendingMessages((prev) => prev.map((p) =>
        p.id === tempId ? { ...p, status: "failed", error: "对方离线" } : p
      ));
      return;
    }
    try {
      await onSendSticker(filePath);
    } catch (e) {
      setPendingMessages((prev) => prev.map((p) =>
        p.id === tempId ? { ...p, status: "failed", error: String(e) } : p
      ));
    }
  }, [peer, isGroup, onSendSticker]);

  const sendText = useCallback(async () => {
    const trimmed = inputText.trim();
    if (!trimmed || !peer) return;
    setInputText("");
    nearBottomRef.current = true;
    if (inputRef.current) {
      inputRef.current.style.height = "auto";
    }

    const tempId = ++pendingId;
    const temp: PendingMessage = { id: tempId, content: trimmed, msg_type: "text", status: "sending" };
    setPendingMessages((prev) => [...prev, temp]);

    try {
      await onSendMessage(trimmed);
      setPendingMessages((prev) => prev.filter((p) => p.id !== tempId));
    } catch (e) {
      setPendingMessages((prev) => prev.map((p) =>
        p.id === tempId ? { ...p, status: "failed", error: String(e) } : p
      ));
    }
  }, [inputText, peer, onSendMessage]);

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

    const tempId = ++pendingId;
    const temp: PendingMessage = {
      id: tempId,
      content: `📎 ${file.name}`,
      msg_type: "file",
      file_name: file.name,
      status: "sending",
    };
    setPendingMessages((prev) => [...prev, temp]);

    try {
      // @ts-expect-error Tauri adds path property on drag events
      const filePath: string = file.path;
      if (filePath) {
        // attach the real path so retries can use it and so pending matches final message
        setPendingMessages((prev) => prev.map((p) => p.id === tempId ? { ...p, file_path: filePath } : p));
        onSendFile(filePath).catch((e) => {
          setPendingMessages((prev) => prev.map((p) =>
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
      setPendingMessages((prev) => prev.map((p) => p.id === tempId ? { ...p, file_name: savedName, file_path: savedPath, file_size: file.size } : p));
      onSendFile(savedPath).catch((e) => {
        setPendingMessages((prev) => prev.map((p) =>
          p.id === tempId ? { ...p, status: "failed", error: String(e) } : p
        ));
      });
    } catch (e) {
      setPendingMessages((prev) => prev.map((p) =>
        p.id === tempId ? { ...p, status: "failed", error: String(e) } : p
      ));
    }
  }, [peer, onSendFile]);

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
    if (!peer) return;
    let unlistenHover: (() => void) | undefined;
    let unlistenDrop: (() => void) | undefined;
    let unlistenCancelled: (() => void) | undefined;

    import("@tauri-apps/api/event").then(({ listen }) => {
      listen("tauri://file-drop-hover", () => {
        setIsDragging(true);
      }).then((fn) => { unlistenHover = fn; });

      listen("tauri://file-drop-cancelled", () => {
        setIsDragging(false);
      }).then((fn) => { unlistenCancelled = fn; });

      listen("tauri://file-drop", (event) => {
        setIsDragging(false);
        const paths = event.payload as string[];
        for (const filePath of paths) {
          const name = filePath.replace(/\\/g, "/").split("/").pop() || "file";
          nearBottomRef.current = true;
          const tempId = ++pendingId;
          setPendingMessages((prev) => [...prev, {
            id: tempId, content: `📎 ${name}`, msg_type: "file", file_name: name, status: "sending",
          }]);
          onSendFile(filePath).catch((e) => {
            setPendingMessages((prev) => prev.map((p) =>
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
  }, [peer?.id, onSendFile]);

  const handlePickFile = async () => {
    const selected = await open({ multiple: true });
    if (!selected) return;
    const paths = Array.isArray(selected) ? selected : [selected];
    for (const filePath of paths) {
      const name = filePath.replace(/\\/g, "/").split("/").pop() || "file";
      nearBottomRef.current = true;
      const tempId = ++pendingId;
      setPendingMessages((prev) => [...prev, {
        id: tempId, content: `📎 ${name}`, msg_type: "file", file_name: name, status: "sending",
      }]);
      onSendFile(filePath).catch((e) => {
        setPendingMessages((prev) => prev.map((p) =>
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

  return (
    <div className="flex-1 flex h-full min-w-0">
    <div
      className="chat-surface flex-1 flex flex-col bg-gray-800 h-full relative min-w-0"
      onDragOver={(e) => e.preventDefault()}
    >
      {isDragging && (
        <div className="absolute inset-0 z-50 bg-indigo-600/20 border-2 border-dashed border-indigo-400 flex items-center justify-center backdrop-blur-sm">
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
        <div className="relative">
          <div className={`w-9 h-9 rounded-full flex items-center justify-center text-sm font-medium text-white ${isGroup ? "bg-indigo-700 text-base" : "bg-gray-600"}`}>
            {isGroup ? "👥" : peer.username.charAt(0).toUpperCase()}
          </div>
          {!isGroup && (
            <div className={`absolute -bottom-0.5 -right-0.5 w-2.5 h-2.5 rounded-full border-2 border-gray-900 ${peer.online ? "bg-green-400" : "bg-gray-500"}`} />
          )}
        </div>
        <div className="flex-1 min-w-0">
          <p className="text-white text-sm font-semibold truncate">{peer.username}</p>
          <p className="text-xs text-gray-400">{isGroup ? "群聊" : (peer.online ? `${peer.ip}:${peer.port}` : "离线")}</p>
        </div>
        <button
          onClick={() => { setShowSearch((v) => !v); setSearchQuery(""); setSearchIndex(0); }}
          className="flex-shrink-0 w-8 h-8 rounded-lg hover:bg-gray-700 flex items-center justify-center text-gray-400 hover:text-white"
          title="搜索消息"
        >
          <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
          </svg>
        </button>
        {isGroup && (
          <button
            onClick={() => { setShowGroupPanel((v) => !v); setGroupNameEdit(groupInfo?.name ?? ""); }}
            className={`flex-shrink-0 w-8 h-8 rounded-lg flex items-center justify-center text-gray-400 hover:text-white hover:bg-gray-700 ${showGroupPanel ? "bg-gray-700 text-white" : ""}`}
            title="群信息"
          >
            <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M17 20h5v-2a3 3 0 00-5.356-1.857M17 20H7m10 0v-2c0-.656-.126-1.283-.356-1.857M7 20H2v-2a3 3 0 015.356-1.857M7 20v-2c0-.656.126-1.283.356-1.857m0 0a5.002 5.002 0 019.288 0M15 7a3 3 0 11-6 0 3 3 0 016 0z" />
            </svg>
          </button>
        )}
      </div>
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
        {allItems.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-full text-gray-500">
            <p className="text-sm">暂无消息</p>
            <p className="text-xs mt-1">向 {peer.username} 发送第一条消息吧</p>
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
              elements.push(
                <div key={`pending-${item.id}`} className="message-row flex justify-end mb-3 px-4">
                  <div className="max-w-[70%] flex flex-col items-end">
                    <div className={`${isPendingSticker ? "overflow-hidden rounded-xl" : "rounded-2xl px-4 py-2.5 rounded-br-md"} ${
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
                          <p className="text-sm truncate">{item.file_name || "文件"}</p>
                        </div>
                      ) : (
                        <p className="text-sm whitespace-pre-wrap break-words">{item.content}</p>
                      )}
                    </div>
                    {item.msg_type === "file" && item.status === "sending" && item.progress !== undefined && (
                      <div className="w-full bg-gray-700 rounded-full h-1.5 mt-1.5">
                        <div className="bg-indigo-400 h-1.5 rounded-full transition-all" style={{ width: `${item.progress}%` }} />
                      </div>
                    )}
                    <div className="flex items-center gap-2 mt-1">
                      <span className="text-[10px] text-gray-500">
                        {item.status === "failed" ? "发送失败" : item.msg_type === "file" && item.progress !== undefined ? `${item.progress}% ${formatSpeed(item.speed)}` : "发送中..."}
                      </span>
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
                    highlighted={searchMatchIds.has(item.id) && item.id === highlightedId}
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
                <div className="flex border-b border-gray-700">
                  <button
                    onClick={() => setEmojiTab("default")}
                    className={`flex-1 py-2 text-xs font-medium ${emojiTab === "default" ? "text-indigo-300 border-b-2 border-indigo-400" : "text-gray-400 hover:text-gray-200"}`}
                  >
                    默认
                  </button>
                  <button
                    onClick={() => setEmojiTab("custom")}
                    className={`flex-1 py-2 text-xs font-medium ${emojiTab === "custom" ? "text-indigo-300 border-b-2 border-indigo-400" : "text-gray-400 hover:text-gray-200"}`}
                  >
                    自定义
                  </button>
                </div>
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
                                <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor">
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
    </div>
    {/* Group info panel */}
    {isGroup && showGroupPanel && groupInfo && (
      <div className="w-64 flex-shrink-0 bg-gray-900 border-l border-gray-700 flex flex-col h-full overflow-y-auto">
        <div className="px-4 py-3 border-b border-gray-700 flex items-center justify-between">
          <span className="text-sm font-semibold text-white">群信息</span>
          <button onClick={() => setShowGroupPanel(false)} className="text-gray-500 hover:text-gray-300 text-lg leading-none">×</button>
        </div>
        <div className="px-4 py-3 space-y-4 flex-1">
          {/* Group name */}
          <div>
            <p className="text-xs text-gray-400 mb-1">群名称</p>
            <div className="flex gap-1">
              <input
                value={groupNameEdit}
                onChange={(e) => setGroupNameEdit(e.target.value)}
                className="flex-1 bg-gray-800 border border-gray-600 rounded px-2 py-1 text-sm text-gray-200 outline-none focus:border-indigo-500"
              />
              <button
                onClick={async () => {
                  if (groupNameEdit.trim() && groupNameEdit !== groupInfo.name) {
                    await renameGroup(groupInfo.group_id, groupNameEdit.trim());
                    onGroupUpdated?.();
                  }
                }}
                className="px-2 py-1 text-xs bg-indigo-600 hover:bg-indigo-500 rounded text-white"
              >保存</button>
            </div>
          </div>
          {/* Members */}
          <div>
            <p className="text-xs text-gray-400 mb-2">成员 ({groupInfo.members?.length || 0}人)</p>
            <div className="space-y-1.5">
              {groupInfo.members?.map((m) => {
                const displayName = m.peer_id === myId ? (myName || m.username || "我") : (m.username || m.peer_id);
                return (
                <div key={m.peer_id} className="flex items-center gap-2">
                  <div className="w-7 h-7 rounded-full bg-gray-600 flex items-center justify-center text-xs font-medium text-white flex-shrink-0">
                    {displayName.charAt(0).toUpperCase()}
                  </div>
                  <div className="flex-1 min-w-0">
                    <p className="text-xs text-gray-200 truncate">{displayName}{m.peer_id === myId ? " (我)" : ""}</p>
                    {groupInfo.creator_id === m.peer_id && <p className="text-[10px] text-indigo-400">群主</p>}
                  </div>
                  <span className={`w-2 h-2 rounded-full flex-shrink-0 ${m.peer_id === myId ? "bg-green-400" : m.is_online ? "bg-green-400" : "bg-gray-600"}`} />
                </div>
                );
              })}
            </div>
          </div>
          {/* Invite */}
          <div>
            <p className="text-xs text-gray-400 mb-1">邀请成员</p>
            <select
              onChange={async (e) => {
                const pid = e.target.value;
                if (pid) { await inviteToGroup(groupInfo.group_id, [pid]); onGroupUpdated?.(); }
                e.target.value = "";
              }}
              className="w-full bg-gray-800 border border-gray-600 rounded px-2 py-1.5 text-xs text-gray-200 outline-none"
            >
              <option value="">选择联系人...</option>
              {peers.filter((p) => p.id !== myId && !groupInfo.members?.some((m) => m.peer_id === p.id)).map((p) => (
                <option key={p.id} value={p.id}>{p.username}</option>
              ))}
            </select>
          </div>
        </div>
        {/* Leave / Dissolve */}
        <div className="px-4 py-3 border-t border-gray-700">
          {groupInfo.creator_id !== myId ? (
            <button
              onClick={async () => { await leaveGroup(groupInfo.group_id); onGroupUpdated?.(); setShowGroupPanel(false); }}
              className="w-full py-2 text-sm rounded-lg bg-yellow-700/60 hover:bg-yellow-700 text-yellow-200"
            >退出群聊</button>
          ) : (
            <button
              onClick={async () => { await dissolveGroup(groupInfo.group_id); onGroupUpdated?.(); setShowGroupPanel(false); }}
              className="w-full py-2 text-sm rounded-lg bg-red-700/60 hover:bg-red-700 text-red-200"
            >解散群聊</button>
          )}
        </div>
      </div>
    )}
    </div>
  );
}
