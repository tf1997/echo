import { useState, useEffect, useLayoutEffect, useRef, useCallback } from "react";
import type { ReactNode } from "react";
import type { ChatMessage } from "../types";
import { openFile, openFolder, saveTempFile } from "../api";
import { WebviewWindow } from "@tauri-apps/api/window";
import { convertFileSrc } from "@tauri-apps/api/tauri";
import { makeSearchHitId } from "./messageUtils";
import { decodeEchoEmojiTokens, emojiAssetSrc, splitInlineEmojis } from "./emojiCatalog";
import { MESSAGE_TYPE_NUDGE, MESSAGE_TYPE_RPS, getRpsMoveLabel, parseRpsMoveFromContent } from "../messageTypes";
import type { RpsMove } from "../messageTypes";

const MAX_PREVIEW_BYTES = 20 * 1024 * 1024;
const COLLAPSED_TEXT_CHARS = 520;
const COLLAPSED_TEXT_LINES = 8;

export interface ForwardCardData {
  title: string;
  items: ForwardCardItem[];
}

export interface ForwardCardItem {
  sender: string;
  content: string;
  msg_type: string;
  timestamp: string;
  file_name?: string | null;
  file_size?: number | null;
  file_data?: string;
  mime?: string | null;
  attachment_error?: string | null;
}

interface MessageBubbleProps {
  message: ChatMessage;
  isOwn: boolean;
  showSender?: boolean;
  highlighted?: boolean;
  searchQuery?: string;
  activeSearchHitId?: string;
  selectMode?: boolean;
  selected?: boolean;
  onToggleSelect?: (message: ChatMessage) => void;
  onStartForward?: (message: ChatMessage) => void;
  onAddSticker?: (message: ChatMessage) => Promise<void> | void;
}

export function DateDivider({ date }: { date: string }) {
  return (
    <div className="message-row date-divider flex items-center gap-3 px-4 my-2">
      <div className="flex-1 h-px bg-gray-700" />
      <span className="text-[11px] text-gray-500 select-none">{date}</span>
      <div className="flex-1 h-px bg-gray-700" />
    </div>
  );
}

function formatTime(ts: string): string {
  try {
    const date = new Date(ts);
    return date.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" });
  } catch {
    return "";
  }
}

function isImageFile(name: string | null): boolean {
  if (!name) return false;
  const ext = name.split(".").pop()?.toLowerCase() ?? "";
  return ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico", "tiff"].includes(ext);
}

function getCopyableBubbleText(message: ChatMessage): string {
  return message.msg_type === "text" ? decodeEchoEmojiTokens(message.content) : "";
}

function getNudgeDisplayText(message: ChatMessage, isOwn: boolean): string {
  return isOwn ? "你发送了一个抖一抖" : `${message.sender_name || "对方"} 发送了一个抖一抖`;
}

function RpsHandGesture({ move }: { move: RpsMove }) {
  return (
    <svg className="rps-sticker-hand" viewBox="0 0 96 96" aria-hidden="true">
      {move === "rock" ? (
        <g>
          <path className="rps-hand-cuff" d="M34 74h32c6 0 11 5 11 11v5H24v-5c0-6 4-11 10-11z" />
          <path className="rps-hand-fill" d="M25 39c0-6 5-11 11-11h27c8 0 15 7 15 15v18c0 9-7 16-16 16H41c-10 0-18-8-18-18V44c0-2 1-4 2-5z" />
          <rect className="rps-hand-fill" x="29" y="26" width="14" height="29" rx="7" />
          <rect className="rps-hand-fill" x="42" y="23" width="14" height="31" rx="7" />
          <rect className="rps-hand-fill" x="55" y="25" width="14" height="29" rx="7" />
          <path className="rps-hand-line" d="M33 48h33M36 61h28" />
        </g>
      ) : move === "scissors" ? (
        <g>
          <path className="rps-hand-cuff" d="M36 73h28c6 0 11 5 11 11v6H27v-6c0-6 4-11 9-11z" />
          <rect className="rps-hand-fill" x="40" y="11" width="14" height="56" rx="7" transform="rotate(-19 47 39)" />
          <rect className="rps-hand-fill" x="57" y="14" width="14" height="53" rx="7" transform="rotate(19 64 41)" />
          <path className="rps-hand-fill" d="M35 44c7-6 18-5 25 2l7 7c8 8 8 21 0 29l-2 2H38c-10 0-18-8-18-18v-5c0-5 6-7 9-3l7 8z" />
          <rect className="rps-hand-fill" x="21" y="48" width="18" height="34" rx="9" transform="rotate(-33 30 65)" />
          <path className="rps-hand-line" d="M44 54c6 3 10 8 11 16M52 43l7 8" />
        </g>
      ) : (
        <g>
          <path className="rps-hand-cuff" d="M33 73h32c6 0 11 5 11 11v6H24v-6c0-6 4-11 9-11z" />
          <rect className="rps-hand-fill" x="23" y="17" width="12" height="51" rx="6" />
          <rect className="rps-hand-fill" x="37" y="10" width="12" height="58" rx="6" />
          <rect className="rps-hand-fill" x="51" y="13" width="12" height="56" rx="6" />
          <rect className="rps-hand-fill" x="65" y="23" width="12" height="45" rx="6" />
          <path className="rps-hand-fill" d="M27 53c0-9 7-16 16-16h18c8 0 15 7 15 15v13c0 10-8 18-18 18H42c-9 0-15-7-15-16z" />
          <rect className="rps-hand-fill" x="17" y="47" width="17" height="34" rx="8.5" transform="rotate(-31 25.5 64)" />
          <path className="rps-hand-line" d="M36 44v25M49 41v29M62 45v24" />
        </g>
      )}
    </svg>
  );
}

function RpsSticker({ message }: { message: ChatMessage }) {
  const move = parseRpsMoveFromContent(message.content);
  if (!move) {
    return <p className="message-text">{message.content}</p>;
  }

  const label = getRpsMoveLabel(move);

  return (
    <div className="rps-sticker" role="img" aria-label={`猜拳：${label}`}>
      <div className={`rps-sticker-face rps-sticker-${move}`}>
        <span className="rps-sticker-spark rps-sticker-spark-left" aria-hidden="true" />
        <span className="rps-sticker-spark rps-sticker-spark-right" aria-hidden="true" />
        <RpsHandGesture move={move} />
      </div>
    </div>
  );
}

function getFileExtension(name: string | null): string {
  if (!name) return "FILE";
  const ext = name.split(".").pop()?.trim();
  return ext ? ext.slice(0, 4).toUpperCase() : "FILE";
}

function isLongText(text: string): boolean {
  return text.length > COLLAPSED_TEXT_CHARS || text.split(/\r?\n/).length > COLLAPSED_TEXT_LINES;
}

function getCollapsedText(text: string): string {
  const lines = text.split(/\r?\n/);
  const byLine = lines.length > COLLAPSED_TEXT_LINES
    ? lines.slice(0, COLLAPSED_TEXT_LINES).join("\n")
    : text;
  const byLength = byLine.length > COLLAPSED_TEXT_CHARS
    ? byLine.slice(0, COLLAPSED_TEXT_CHARS)
    : byLine;
  return byLength.trimEnd() + "...";
}

function looksLikeCodeText(text: string): boolean {
  if (text.includes("```")) return true;
  const lines = text.split(/\r?\n/).filter((line) => line.trim().length > 0);
  if (lines.length < 2) return false;
  const codeLikeLines = lines.filter((line) => {
    const trimmed = line.trim();
    return (
      /^\s{2,}\S/.test(line) ||
      /[{};]$/.test(trimmed) ||
      /^(const|let|var|function|class|import|export|pub|fn|struct|enum|impl)\b/.test(trimmed) ||
      trimmed.includes("=>")
    );
  });
  return codeLikeLines.length >= Math.min(3, lines.length);
}

function renderTextWithLinks(text: string): ReactNode {
  const pattern = /(https?:\/\/[^\s<]+|www\.[^\s<]+)/gi;
  const parts: ReactNode[] = [];
  let cursor = 0;
  let index = 0;
  let match = pattern.exec(text);

  while (match) {
    if (match.index > cursor) {
      parts.push(...renderInlineEmojis(text.slice(cursor, match.index), `text-${cursor}`));
    }
    parts.push(
      <span key={`url-${index++}`} className="message-url">
        {match[0]}
      </span>
    );
    cursor = match.index + match[0].length;
    match = pattern.exec(text);
  }

  if (cursor < text.length) {
    parts.push(...renderInlineEmojis(text.slice(cursor), `text-${cursor}`));
  }
  return parts.length > 0 ? parts : text;
}

function renderInlineEmojis(text: string, keyPrefix: string): ReactNode[] {
  const segments = splitInlineEmojis(text);
  return segments.map((segment, index) => {
    if (segment.type === "text") return segment.text;
    return (
      <img
        key={`${keyPrefix}-emoji-${segment.id}-${index}`}
        className="inline-emoji"
        src={emojiAssetSrc(segment.id)}
        alt={segment.emoji}
        title={segment.emoji}
        draggable={false}
      />
    );
  });
}

function renderTextWithSearchHighlights(text: string, query: string, messageId: number, activeSearchHitId?: string): ReactNode {
  const needle = query.trim();
  if (!needle) return renderInlineEmojis(text, "search-empty");

  const lowerText = text.toLowerCase();
  const lowerNeedle = needle.toLowerCase();
  const parts: ReactNode[] = [];
  let cursor = 0;
  let matchIndex = lowerText.indexOf(lowerNeedle, cursor);
  let occurrenceIndex = 0;

  while (matchIndex !== -1) {
    if (matchIndex > cursor) {
      parts.push(...renderInlineEmojis(text.slice(cursor, matchIndex), `search-${cursor}`));
    }
    const endIndex = matchIndex + needle.length;
    const hitId = makeSearchHitId(messageId, occurrenceIndex);
    parts.push(
      <mark
        key={hitId}
        className={`message-search-hit ${hitId === activeSearchHitId ? "message-search-hit-current" : ""}`}
        data-search-hit-id={hitId}
      >
        {renderInlineEmojis(text.slice(matchIndex, endIndex), `hit-${hitId}`)}
      </mark>
    );
    occurrenceIndex += 1;
    cursor = endIndex;
    matchIndex = lowerText.indexOf(lowerNeedle, cursor);
  }

  if (cursor < text.length) {
    parts.push(...renderInlineEmojis(text.slice(cursor), `search-${cursor}`));
  }

  return parts.length > 0 ? parts : text;
}

function handleOpenFile(filePath: string | null) {
  if (filePath) openFile(filePath).catch(console.error);
}

function handleOpenFolder(filePath: string | null) {
  if (filePath) openFolder(filePath).catch(console.error);
}

function getForwardItemText(item: ForwardCardItem): string {
  if (item.msg_type === "file") return item.file_name ? `📎 ${item.file_name}` : "[文件]";
  if (item.msg_type === "sticker") return item.file_name ? `[图片] ${item.file_name}` : "[图片]";
  if (item.msg_type === "forward_card") return "[聊天记录]";
  if (item.msg_type === MESSAGE_TYPE_NUDGE) return "[抖一抖]";
  if (item.msg_type === MESSAGE_TYPE_RPS) return item.content || "[猜拳]";
  return decodeEchoEmojiTokens(item.content);
}

function isForwardAttachment(item: ForwardCardItem): boolean {
  return item.msg_type === "file" || item.msg_type === "sticker";
}

function isForwardImage(item: ForwardCardItem): boolean {
  if (item.mime?.startsWith("image/")) return true;
  return isImageFile(item.file_name ?? null);
}

function base64ToBytes(base64: string): number[] {
  const binary = atob(base64);
  const bytes = new Array<number>(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

async function copyTextToClipboard(text: string) {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return;
    }
  } catch {
    // Fall back to a temporary textarea below.
  }

  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.setAttribute("readonly", "");
  textarea.style.position = "fixed";
  textarea.style.left = "-9999px";
  textarea.style.opacity = "0";
  document.body.appendChild(textarea);
  textarea.select();
  try {
    document.execCommand("copy");
  } finally {
    document.body.removeChild(textarea);
  }
}

function ImagePreview({ filePath, fileSize }: { filePath: string; fileSize: number | null }) {
  const [failedPath, setFailedPath] = useState<string | null>(null);
  const src = convertFileSrc(filePath);
  const failed = failedPath === filePath;

  const openPreview = useCallback(() => {
    const fileName = filePath.replace(/\\/g, "/").split("/").pop() || "image";
    new WebviewWindow(`image-preview-${Date.now()}`, {
      url: convertFileSrc(filePath),
      title: fileName,
      width: 960,
      height: 720,
      center: true,
      resizable: true,
    });
  }, [filePath]);

  if (failed || (fileSize !== null && fileSize > MAX_PREVIEW_BYTES)) return null;
  return (
    <button
      type="button"
      title="点击预览图片"
      className="message-image-preview"
      onClick={openPreview}
    >
      <img
        src={src}
        alt=""
        className="w-full max-h-[320px] object-contain rounded"
        onError={() => setFailedPath(filePath)}
      />
      <span className="message-image-preview-badge">预览</span>
    </button>
  );
}

function ForwardCard({ data, isOwn }: { data: ForwardCardData; isOwn: boolean }) {
  const [expanded, setExpanded] = useState(false);
  const [savingIndex, setSavingIndex] = useState<number | null>(null);
  const [savedPaths, setSavedPaths] = useState<Record<number, string>>({});
  const preview = data.items.slice(0, 3);

  const downloadForwardAttachment = useCallback(async (item: ForwardCardItem, index: number) => {
    const savedPath = savedPaths[index];
    if (savedPath) {
      handleOpenFile(savedPath);
      return;
    }
    if (!item.file_data || savingIndex !== null) return;

    setSavingIndex(index);
    try {
      const fileName = item.file_name || "file";
      const path = await saveTempFile(base64ToBytes(item.file_data), fileName);
      setSavedPaths((prev) => ({ ...prev, [index]: path }));
    } catch (error) {
      console.error("Failed to save forwarded attachment:", error);
    } finally {
      setSavingIndex(null);
    }
  }, [savedPaths, savingIndex]);

  return (
    <div
      className={`w-56 cursor-pointer rounded-xl overflow-hidden border ${isOwn ? "border-indigo-400/30 bg-indigo-700/60" : "border-gray-600 bg-gray-600"}`}
      onClick={() => setExpanded(true)}
    >
      <div className="px-3 pt-3 pb-2">
        <p className="text-xs font-semibold mb-1.5 opacity-90">{data.title}</p>
        {preview.map((item, i) => (
          <p key={i} className="text-xs opacity-70 truncate leading-5">
            <span className="font-medium">{item.sender}：</span>
            {renderInlineEmojis(getForwardItemText(item), `forward-preview-${i}`)}
          </p>
        ))}
        {data.items.length > 3 && <p className="text-xs opacity-50 mt-0.5">…</p>}
      </div>
      <div className={`px-3 py-1.5 text-[10px] opacity-60 border-t ${isOwn ? "border-indigo-400/20" : "border-gray-500"}`}>
        聊天记录
      </div>
      {expanded && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/60"
          onClick={(e) => { e.stopPropagation(); setExpanded(false); }}
        >
          <div className="bg-gray-800 border border-gray-600 rounded-2xl w-96 max-h-[70vh] flex flex-col shadow-2xl" onClick={(e) => e.stopPropagation()}>
            <div className="flex items-center justify-between px-4 py-3 border-b border-gray-700">
              <span className="text-sm font-semibold text-white">{data.title}</span>
              <button onClick={() => setExpanded(false)} className="text-gray-400 hover:text-white text-lg leading-none">×</button>
            </div>
            <div className="overflow-y-auto flex-1 px-4 py-3 space-y-3">
              {data.items.map((item, i) => (
                <div key={i}>
                  <div className="flex items-baseline gap-2 mb-0.5">
                    <p className="text-xs text-indigo-300">{item.sender}</p>
                    <p className="text-[10px] text-gray-500">{formatTime(item.timestamp)}</p>
                  </div>
                  {isForwardAttachment(item) ? (
                    <div className="rounded-lg border border-gray-700 bg-gray-900/40 overflow-hidden">
                      {item.file_data && isForwardImage(item) ? (
                        <img
                          src={`data:${item.mime || "image/*"};base64,${item.file_data}`}
                          alt=""
                          className="w-full max-h-56 object-contain bg-black/20"
                        />
                      ) : null}
                      <div className="flex items-center gap-2 px-3 py-2">
                        <div className="flex-1 min-w-0">
                          <p className="text-sm text-gray-200 truncate">{item.file_name || (item.msg_type === "sticker" ? "图片" : "文件")}</p>
                          {item.file_size ? <p className="text-xs text-gray-500">{formatFileSize(item.file_size)}</p> : null}
                          {!item.file_data ? <p className="text-xs text-red-300">{item.attachment_error || "文件不可下载"}</p> : null}
                        </div>
                        <button
                          disabled={!item.file_data || savingIndex !== null}
                          onClick={() => downloadForwardAttachment(item, i)}
                          className="flex-shrink-0 rounded-lg bg-indigo-600 hover:bg-indigo-500 disabled:bg-gray-700 disabled:text-gray-500 px-3 py-1.5 text-xs text-white"
                        >
                          {savedPaths[i] ? "打开" : savingIndex === i ? "保存中" : "下载"}
                        </button>
                        {savedPaths[i] ? (
                          <button
                            onClick={() => handleOpenFolder(savedPaths[i])}
                            className="flex-shrink-0 w-7 h-7 rounded-lg hover:bg-white/10 flex items-center justify-center"
                            title="在文件夹中显示"
                          >
                            <svg className="w-4 h-4 text-gray-300" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z" /></svg>
                          </button>
                        ) : null}
                      </div>
                    </div>
                  ) : (
                    <p className="text-sm text-gray-200 whitespace-pre-wrap break-words">
                      {renderInlineEmojis(getForwardItemText(item), `forward-item-${i}`)}
                    </p>
                  )}
                </div>
              ))}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

export function MessageBubble({ message, isOwn, showSender = false, highlighted = false, searchQuery = "", activeSearchHitId, selectMode = false, selected = false, onToggleSelect, onStartForward, onAddSticker }: MessageBubbleProps) {
  const isNudge = message.msg_type === MESSAGE_TYPE_NUDGE;
  const isRps = message.msg_type === MESSAGE_TYPE_RPS;
  const isSticker = message.msg_type === "sticker";
  const isFile = message.msg_type === "file";
  const showPreview = isFile && !!message.file_path && (isImageFile(message.file_name) || isImageFile(message.file_path));
  const isMediaBubble = isSticker || showPreview || isRps;
  const [showMenu, setShowMenu] = useState(false);
  const [menuPosition, setMenuPosition] = useState<{ x: number; y: number } | null>(null);
  const [addingSticker, setAddingSticker] = useState(false);
  const [textExpanded, setTextExpanded] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const copyableText = getCopyableBubbleText(message);
  const canCopyText = copyableText.length > 0;
  const canAddSticker = isSticker && !isOwn && !!message.file_path && !!onAddSticker;
  const attachmentName = message.file_name || (isSticker ? "图片" : "文件");
  const canCopyFileName = (isFile || isSticker) && !!message.file_name;
  const canOpenAttachment = (isFile || isSticker) && !!message.file_path;
  const isTextMessage = message.msg_type === "text";
  const hasSearchQuery = !!searchQuery.trim();
  const shouldCollapseText = isTextMessage && !hasSearchQuery && isLongText(message.content);
  const visibleText = shouldCollapseText && !textExpanded ? getCollapsedText(message.content) : message.content;
  const isCodeTextMessage = isTextMessage && looksLikeCodeText(message.content);
  const messageTextClass = `message-text ${isCodeTextMessage ? "message-text-code" : ""}`;

  useEffect(() => {
    if (!showMenu) return;
    const handler = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) setShowMenu(false);
    };
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") setShowMenu(false);
    };
    document.addEventListener("mousedown", handler);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("mousedown", handler);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [showMenu]);

  useLayoutEffect(() => {
    if (!showMenu || !menuPosition || !menuRef.current) return;
    const padding = 8;
    const rect = menuRef.current.getBoundingClientRect();
    const nextX = Math.min(Math.max(padding, menuPosition.x), window.innerWidth - rect.width - padding);
    const nextY = Math.min(Math.max(padding, menuPosition.y), window.innerHeight - rect.height - padding);
    if (nextX !== menuPosition.x || nextY !== menuPosition.y) {
      setMenuPosition({ x: nextX, y: nextY });
    }
  }, [showMenu, menuPosition]);

  if (isNudge) {
    return (
      <div
        className={`message-row nudge-message-row flex justify-center mb-3 px-4 ${highlighted ? "bg-indigo-900/30 rounded-lg" : ""} ${selected ? "bg-indigo-900/20 rounded-lg" : ""} ${selectMode ? "cursor-pointer" : ""}`}
        onClick={() => { if (selectMode) onToggleSelect?.(message); }}
      >
        <span className="nudge-message-pill" title={formatTime(message.timestamp)}>
          <svg className="nudge-message-icon" fill="none" viewBox="0 0 24 24" stroke="currentColor">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.8} d="M9 4h6a2 2 0 012 2v12a2 2 0 01-2 2H9a2 2 0 01-2-2V6a2 2 0 012-2z" />
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.8} d="M4 8l-2 2 2 2M20 8l2 2-2 2M4 14l-2 2 2 2M20 14l2 2-2 2" />
          </svg>
          {getNudgeDisplayText(message, isOwn)}
        </span>
      </div>
    );
  }

  return (
    <div
      className={`message-row flex ${isOwn ? "justify-end" : "justify-start"} mb-3 px-4 group relative ${highlighted ? "bg-indigo-900/30 rounded-lg" : ""} ${selected ? "bg-indigo-900/20 rounded-lg" : ""} ${selectMode ? "cursor-pointer" : ""}`}
      onClick={() => { if (selectMode) onToggleSelect?.(message); }}
      onContextMenu={(e) => {
        if (!selectMode) {
          e.preventDefault();
          setMenuPosition({ x: e.clientX, y: e.clientY });
          setShowMenu(true);
        }
      }}
    >
      {showMenu && menuPosition && (
        <div
          ref={menuRef}
          className="context-menu fixed z-50 bg-gray-900 border border-gray-600 rounded-lg shadow-xl py-1"
          style={{ left: menuPosition.x, top: menuPosition.y }}
        >
          {canCopyText && (
            <button
              className="px-3 py-1 text-xs text-gray-200 hover:bg-gray-700 rounded-lg whitespace-nowrap"
              onClick={async (e) => {
                e.stopPropagation();
                try {
                  await copyTextToClipboard(copyableText);
                } catch (error) {
                  console.error("Failed to copy message:", error);
                } finally {
                  setShowMenu(false);
                }
              }}
            >
              复制文字
            </button>
          )}
          {canCopyFileName && (
            <button
              className="block w-full px-3 py-1 text-left text-xs text-gray-200 hover:bg-gray-700 rounded-lg whitespace-nowrap"
              onClick={async (e) => {
                e.stopPropagation();
                try {
                  await copyTextToClipboard(message.file_name || "");
                } catch (error) {
                  console.error("Failed to copy filename:", error);
                } finally {
                  setShowMenu(false);
                }
              }}
            >
              复制文件名
            </button>
          )}
          {canOpenAttachment && (
            <button
              className="block w-full px-3 py-1 text-left text-xs text-gray-200 hover:bg-gray-700 rounded-lg whitespace-nowrap"
              onClick={(e) => {
                e.stopPropagation();
                handleOpenFile(message.file_path);
                setShowMenu(false);
              }}
            >
              打开文件
            </button>
          )}
          {canOpenAttachment && (
            <button
              className="block w-full px-3 py-1 text-left text-xs text-gray-200 hover:bg-gray-700 rounded-lg whitespace-nowrap"
              onClick={(e) => {
                e.stopPropagation();
                handleOpenFolder(message.file_path);
                setShowMenu(false);
              }}
            >
              在文件夹中显示
            </button>
          )}
          <button
            className="px-3 py-1 text-xs text-gray-200 hover:bg-gray-700 rounded-lg whitespace-nowrap"
            onClick={(e) => { e.stopPropagation(); onStartForward?.(message); setShowMenu(false); }}
          >
            转发
          </button>
          {canAddSticker && (
            <button
              disabled={addingSticker}
              className="block w-full px-3 py-1 text-left text-xs text-gray-200 hover:bg-gray-700 rounded-lg whitespace-nowrap disabled:opacity-50"
              onClick={async (e) => {
                e.stopPropagation();
                setAddingSticker(true);
                try {
                  await onAddSticker(message);
                  setShowMenu(false);
                } catch (error) {
                  console.error("Failed to add sticker:", error);
                } finally {
                  setAddingSticker(false);
                }
              }}
            >
              {addingSticker ? "添加中..." : "添加到表情"}
            </button>
          )}
        </div>
      )}

      {selectMode && (
        <div className={`flex-shrink-0 flex items-center ${isOwn ? "order-last ml-2" : "mr-2"}`}>
          <div className={`w-5 h-5 rounded-full border-2 flex items-center justify-center transition-colors ${selected ? "bg-indigo-500 border-indigo-500" : "border-gray-500"}`}>
            {selected && <svg className="w-3 h-3 text-white" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth={3} d="M5 13l4 4L19 7" /></svg>}
          </div>
        </div>
      )}

      <div className={`message-stack ${isOwn ? "items-end" : "items-start"} flex flex-col`}>
        {!isOwn && showSender && (
          <span className="message-sender-label" title={message.sender_name}>{message.sender_name}</span>
        )}
        <div className={`${isMediaBubble ? "bg-transparent" : `message-bubble-shell rounded-2xl overflow-hidden ${isOwn ? "message-bubble-own bg-indigo-600 text-white rounded-br-md" : "message-bubble-other bg-gray-700 text-gray-100 rounded-bl-md"}`} ${!isMediaBubble && message.msg_type !== "forward_card" ? `message-bubble-content ${isCodeTextMessage ? "message-bubble-code-content" : ""}` : ""}`}>
          {message.msg_type === "forward_card" ? (() => {
            try {
              const card: ForwardCardData = JSON.parse(message.content);
              return <ForwardCard data={card} isOwn={isOwn} />;
            } catch {
              return <p className="text-sm px-4 py-2.5">[聊天记录]</p>;
            }
          })() : isSticker && message.file_path ? (
            <div className="message-media-frame message-sticker-frame">
              <ImagePreview filePath={message.file_path} fileSize={message.file_size} />
            </div>
          ) : showPreview ? (
            <div className="message-media-frame message-image-frame">
              <ImagePreview filePath={message.file_path!} fileSize={message.file_size} />
            </div>
          ) : isFile ? (
            <div className="message-file-card flex items-center gap-2">
              <button type="button" className="flex items-center gap-2 flex-1 min-w-0 text-left hover:opacity-85" onClick={() => handleOpenFile(message.file_path)} title={`打开 ${attachmentName}`}>
                <span className="message-file-icon">{getFileExtension(message.file_name)}</span>
                <div className="min-w-0">
                  <p className="message-file-name truncate" title={attachmentName}>{attachmentName}</p>
                  {message.file_size ? <p className="text-xs opacity-70">{formatFileSize(message.file_size)}</p> : null}
                </div>
              </button>
              <button onClick={(e) => { e.stopPropagation(); handleOpenFolder(message.file_path); }} className="flex-shrink-0 w-6 h-6 rounded flex items-center justify-center hover:bg-white/10" title="在文件夹中显示">
                <svg className="w-3.5 h-3.5 opacity-70" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z" /></svg>
              </button>
            </div>
          ) : isRps ? (
            <RpsSticker message={message} />
          ) : (
            <>
              <p className={messageTextClass}>
                {hasSearchQuery
                  ? renderTextWithSearchHighlights(visibleText, searchQuery, message.id, activeSearchHitId)
                  : renderTextWithLinks(visibleText)}
              </p>
              {shouldCollapseText ? (
                <button
                  type="button"
                  className="message-inline-action"
                  onClick={(e) => {
                    e.stopPropagation();
                    setTextExpanded((value) => !value);
                  }}
                >
                  {textExpanded ? "收起" : "展开全文"}
                </button>
              ) : null}
            </>
          )}
        </div>
        <span className={`message-meta ${isOwn ? "mr-1" : "ml-1"}`}>
          {formatTime(message.timestamp)}
        </span>
      </div>
    </div>
  );
}

function formatFileSize(bytes: number): string {
  if (bytes < 1024) return bytes + " B";
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + " KB";
  if (bytes < 1024 * 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + " MB";
  return (bytes / (1024 * 1024 * 1024)).toFixed(1) + " GB";
}
