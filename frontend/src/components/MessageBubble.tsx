import { useState, useEffect, useLayoutEffect, useRef, useCallback } from "react";
import type { ReactNode } from "react";
import type { ChatMessage } from "../types";
import { openFile, openFolder } from "../api";
import { WebviewWindow } from "@tauri-apps/api/window";
import { convertFileSrc } from "@tauri-apps/api/tauri";
import { makeSearchHitId } from "./messageUtils";

const MAX_PREVIEW_BYTES = 20 * 1024 * 1024;

export interface ForwardCardData {
  title: string;
  items: { sender: string; content: string; msg_type: string; timestamp: string }[];
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
  return message.msg_type === "text" ? message.content : "";
}

function renderTextWithSearchHighlights(text: string, query: string, messageId: number, activeSearchHitId?: string): ReactNode {
  const needle = query.trim();
  if (!needle) return text;

  const lowerText = text.toLowerCase();
  const lowerNeedle = needle.toLowerCase();
  const parts: ReactNode[] = [];
  let cursor = 0;
  let matchIndex = lowerText.indexOf(lowerNeedle, cursor);
  let occurrenceIndex = 0;

  while (matchIndex !== -1) {
    if (matchIndex > cursor) {
      parts.push(text.slice(cursor, matchIndex));
    }
    const endIndex = matchIndex + needle.length;
    const hitId = makeSearchHitId(messageId, occurrenceIndex);
    parts.push(
      <mark
        key={hitId}
        className={`message-search-hit ${hitId === activeSearchHitId ? "message-search-hit-current" : ""}`}
        data-search-hit-id={hitId}
      >
        {text.slice(matchIndex, endIndex)}
      </mark>
    );
    occurrenceIndex += 1;
    cursor = endIndex;
    matchIndex = lowerText.indexOf(lowerNeedle, cursor);
  }

  if (cursor < text.length) {
    parts.push(text.slice(cursor));
  }

  return parts.length > 0 ? parts : text;
}

function handleOpenFile(filePath: string | null) {
  if (filePath) openFile(filePath).catch(console.error);
}

function handleOpenFolder(filePath: string | null) {
  if (filePath) openFolder(filePath).catch(console.error);
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
    <img
      src={src}
      alt=""
      className="w-full max-h-[320px] object-contain cursor-pointer rounded hover:opacity-90 transition-opacity"
      onClick={openPreview}
      onError={() => setFailedPath(filePath)}
    />
  );
}

function ForwardCard({ data, isOwn }: { data: ForwardCardData; isOwn: boolean }) {
  const [expanded, setExpanded] = useState(false);
  const preview = data.items.slice(0, 3);
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
            {item.msg_type === "file" ? "[文件]" : item.content}
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
                  <p className="text-sm text-gray-200 whitespace-pre-wrap break-words">
                    {item.msg_type === "file" ? "[文件]" : item.content}
                  </p>
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
  const isSticker = message.msg_type === "sticker";
  const isFile = message.msg_type === "file";
  const showPreview = isFile && isImageFile(message.file_name) && message.file_path;
  const [showMenu, setShowMenu] = useState(false);
  const [menuPosition, setMenuPosition] = useState<{ x: number; y: number } | null>(null);
  const [addingSticker, setAddingSticker] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const copyableText = getCopyableBubbleText(message);
  const canCopyText = copyableText.length > 0;
  const canAddSticker = isSticker && !isOwn && !!message.file_path && !!onAddSticker;

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

      <div className={`max-w-[70%] ${isOwn ? "items-end" : "items-start"} flex flex-col`}>
        {!isOwn && showSender && (
          <span className="text-xs text-indigo-300 mb-1 ml-1">{message.sender_name}</span>
        )}
        <div className={`${isSticker ? "bg-transparent" : `rounded-2xl overflow-hidden ${isOwn ? "message-bubble-own bg-indigo-600 text-white rounded-br-md" : "message-bubble-other bg-gray-700 text-gray-100 rounded-bl-md"}`} ${!isSticker && !showPreview && message.msg_type !== "forward_card" ? "px-4 py-2.5" : ""}`}>
          {message.msg_type === "forward_card" ? (() => {
            try {
              const card: ForwardCardData = JSON.parse(message.content);
              return <ForwardCard data={card} isOwn={isOwn} />;
            } catch {
              return <p className="text-sm px-4 py-2.5">[聊天记录]</p>;
            }
          })() : isSticker && message.file_path ? (
            <div className="max-w-[160px]">
              <ImagePreview filePath={message.file_path} fileSize={message.file_size} />
            </div>
          ) : showPreview ? (
            <div className="max-w-[260px]">
              <ImagePreview filePath={message.file_path!} fileSize={message.file_size} />
              <div className="flex items-center gap-1 px-3 py-2">
                <div className="flex-1 min-w-0 cursor-pointer hover:opacity-80" onClick={() => handleOpenFile(message.file_path)}>
                  <p className="text-xs font-medium truncate">{message.file_name}</p>
                  {message.file_size ? <p className="text-[10px] opacity-70">{formatFileSize(message.file_size)}</p> : null}
                </div>
                <button onClick={(e) => { e.stopPropagation(); handleOpenFolder(message.file_path); }} className="flex-shrink-0 w-6 h-6 rounded flex items-center justify-center hover:bg-white/10" title="在文件夹中显示">
                  <svg className="w-3.5 h-3.5 opacity-70" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z" /></svg>
                </button>
              </div>
            </div>
          ) : isFile ? (
            <div className="flex items-center gap-2">
              <div className="flex items-center gap-2 flex-1 min-w-0 cursor-pointer hover:opacity-80" onClick={() => handleOpenFile(message.file_path)} title="点击打开文件">
                <svg className="w-5 h-5 flex-shrink-0 opacity-80" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 10v6m0 0l-3-3m3 3l3-3m2 8H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z" /></svg>
                <div className="min-w-0">
                  <p className="text-sm font-medium truncate">{message.file_name || "文件"}</p>
                  {message.file_size ? <p className="text-xs opacity-70">{formatFileSize(message.file_size)}</p> : null}
                </div>
              </div>
              <button onClick={(e) => { e.stopPropagation(); handleOpenFolder(message.file_path); }} className="flex-shrink-0 w-6 h-6 rounded flex items-center justify-center hover:bg-white/10" title="在文件夹中显示">
                <svg className="w-3.5 h-3.5 opacity-70" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z" /></svg>
              </button>
            </div>
          ) : (
            <p className="text-sm whitespace-pre-wrap break-words">{renderTextWithSearchHighlights(message.content, searchQuery, message.id, activeSearchHitId)}</p>
          )}
        </div>
        <span className={`text-[10px] text-gray-500 mt-1 ${isOwn ? "mr-1" : "ml-1"}`}>
          {formatTime(message.timestamp)}
          {isOwn && <span className="ml-1">{message.is_read ? "✓✓" : "✓"}</span>}
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
