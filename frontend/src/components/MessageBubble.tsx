import { useState, useEffect, useRef, useCallback } from "react";
import type { ChatMessage } from "../types";
import { readFileBase64, openFile, openFolder } from "../api";

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
  selectMode?: boolean;
  selected?: boolean;
  onToggleSelect?: (message: ChatMessage) => void;
  onStartForward?: (message: ChatMessage) => void;
}

export function DateDivider({ date }: { date: string }) {
  return (
    <div className="flex items-center gap-3 px-4 my-2">
      <div className="flex-1 h-px bg-gray-700" />
      <span className="text-[11px] text-gray-500 select-none">{date}</span>
      <div className="flex-1 h-px bg-gray-700" />
    </div>
  );
}

export function formatDateLabel(ts: string): string {
  try {
    const date = new Date(ts);
    const now = new Date();
    const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
    const yesterday = new Date(today.getTime() - 86400000);
    const msgDay = new Date(date.getFullYear(), date.getMonth(), date.getDate());
    if (msgDay.getTime() === today.getTime()) return "今天";
    if (msgDay.getTime() === yesterday.getTime()) return "昨天";
    const year = date.getFullYear();
    const month = String(date.getMonth() + 1).padStart(2, "0");
    const day = String(date.getDate()).padStart(2, "0");
    return year === now.getFullYear() ? `${month}月${day}日` : `${year}年${month}月${day}日`;
  } catch {
    return "";
  }
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

function handleOpenFile(filePath: string | null) {
  if (filePath) openFile(filePath).catch(console.error);
}

function handleOpenFolder(filePath: string | null) {
  if (filePath) openFolder(filePath).catch(console.error);
}

function ImagePreview({ filePath, fileSize }: { filePath: string; fileSize: number | null }) {
  const [src, setSrc] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const [expanded, setExpanded] = useState(false);

  // Zoom & pan state
  const [scale, setScale] = useState(1);
  const [pan, setPan] = useState({ x: 0, y: 0 });
  const [dragging, setDragging] = useState(false);
  const dragStart = useRef({ x: 0, y: 0 });
  const panStart = useRef({ x: 0, y: 0 });

  useEffect(() => {
    if (fileSize !== null && fileSize > MAX_PREVIEW_BYTES) { setFailed(true); return; }
    readFileBase64(filePath)
      .then((data) => setSrc(`data:${data.mime};base64,${data.base64}`))
      .catch(() => setFailed(true));
  }, [filePath, fileSize]);

  // Reset zoom/pan when opening
  useEffect(() => {
    if (expanded) {
      setScale(1);
      setPan({ x: 0, y: 0 });
    }
  }, [expanded]);

  useEffect(() => {
    if (!expanded) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") setExpanded(false); };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [expanded]);

  const handleWheel = useCallback((e: React.WheelEvent) => {
    e.stopPropagation();
    const delta = e.deltaY > 0 ? -0.15 : 0.15;
    setScale((prev) => Math.min(10, Math.max(0.1, prev + delta)));
  }, []);

  const handleMouseDown = useCallback((e: React.MouseEvent) => {
    if (scale <= 1) return;
    e.stopPropagation();
    setDragging(true);
    dragStart.current = { x: e.clientX, y: e.clientY };
    panStart.current = { ...pan };
  }, [scale, pan]);

  const handleMouseMove = useCallback((e: React.MouseEvent) => {
    if (!dragging) return;
    const dx = e.clientX - dragStart.current.x;
    const dy = e.clientY - dragStart.current.y;
    setPan({ x: panStart.current.x + dx, y: panStart.current.y + dy });
  }, [dragging]);

  const handleMouseUp = useCallback(() => {
    setDragging(false);
  }, []);

  // Close lightbox only when clicking the backdrop (not the image)
  const handleBackdropClick = useCallback(() => {
    setExpanded(false);
  }, []);

  if (failed || !src) return null;
  return (
    <>
      <img src={src} alt="" className="w-full max-h-[320px] object-contain cursor-pointer rounded hover:opacity-90 transition-opacity"
        onClick={() => setExpanded(true)} onError={() => setFailed(true)} />
      {expanded && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/10 backdrop-blur-sm select-none"
          onClick={handleBackdropClick}
          onMouseMove={handleMouseMove}
          onMouseUp={handleMouseUp}
          onMouseLeave={handleMouseUp}>
          {/* Top bar */}
          <div className="absolute top-0 left-0 right-0 flex items-center justify-between px-4 py-3 bg-gradient-to-b from-black/60 to-transparent z-10">
            <span className="text-white/70 text-xs">{Math.round(scale * 100)}%</span>
            <div className="flex items-center gap-1">
              <button
                onClick={(e) => { e.stopPropagation(); setScale((s) => Math.min(10, s + 0.25)); }}
                className="w-8 h-8 rounded-lg bg-white/10 hover:bg-white/20 flex items-center justify-center text-white text-lg transition-colors"
                title="放大"
              >+</button>
              <button
                onClick={(e) => { e.stopPropagation(); setScale((s) => Math.max(0.1, s - 0.25)); }}
                className="w-8 h-8 rounded-lg bg-white/10 hover:bg-white/20 flex items-center justify-center text-white text-lg transition-colors"
                title="缩小"
              >−</button>
              <button
                onClick={(e) => { e.stopPropagation(); setScale(1); setPan({ x: 0, y: 0 }); }}
                className="w-8 h-8 rounded-lg bg-white/10 hover:bg-white/20 flex items-center justify-center text-white text-sm transition-colors"
                title="重置"
              >1:1</button>
              <button
                onClick={(e) => { e.stopPropagation(); setExpanded(false); }}
                className="w-10 h-10 rounded-full bg-white/10 hover:bg-white/20 flex items-center justify-center text-white text-xl transition-colors"
              >×</button>
            </div>
          </div>
          <img
            src={src}
            alt=""
            className={`rounded-lg shadow-2xl ${scale > 1 ? "cursor-grab" : ""} ${dragging ? "cursor-grabbing" : ""}`}
            style={{
              transform: `translate(${pan.x}px, ${pan.y}px) scale(${scale})`,
              maxWidth: "95vw",
              maxHeight: "95vh",
              objectFit: "contain",
              transition: dragging ? "none" : "transform 0.15s ease-out",
            }}
            onClick={(e) => e.stopPropagation()}
            onDoubleClick={() => { setScale(1); setPan({ x: 0, y: 0 }); }}
            onWheel={handleWheel}
            onMouseDown={handleMouseDown}
            draggable={false}
          />
        </div>
      )}
    </>
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

export function MessageBubble({ message, isOwn, showSender = false, highlighted = false, selectMode = false, selected = false, onToggleSelect, onStartForward }: MessageBubbleProps) {
  const isFile = message.msg_type === "file";
  const showPreview = isFile && isImageFile(message.file_name) && message.file_path;
  const [showMenu, setShowMenu] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!showMenu) return;
    const handler = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) setShowMenu(false);
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [showMenu]);

  return (
    <div
      className={`flex ${isOwn ? "justify-end" : "justify-start"} mb-3 px-4 group relative ${highlighted ? "bg-indigo-900/30 rounded-lg" : ""} ${selected ? "bg-indigo-900/20 rounded-lg" : ""} ${selectMode ? "cursor-pointer" : ""}`}
      onClick={() => { if (selectMode) onToggleSelect?.(message); }}
      onContextMenu={(e) => { if (!selectMode) { e.preventDefault(); setShowMenu(true); } }}
    >
      {/* Floating menu above the bubble, not overlapping it */}
      {showMenu && (
        <div
          ref={menuRef}
          className={`absolute bottom-full ${isOwn ? "right-4" : "left-4"} mb-1 z-50 bg-gray-900 border border-gray-600 rounded-lg shadow-xl py-1`}
        >
          <button
            className="px-3 py-1 text-xs text-gray-200 hover:bg-gray-700 rounded-lg whitespace-nowrap"
            onClick={(e) => { e.stopPropagation(); onStartForward?.(message); setShowMenu(false); }}
          >
            转发
          </button>
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
        <div className={`rounded-2xl overflow-hidden ${isOwn ? "bg-indigo-600 text-white rounded-br-md" : "bg-gray-700 text-gray-100 rounded-bl-md"} ${!showPreview && message.msg_type !== "forward_card" ? "px-4 py-2.5" : ""}`}>
          {message.msg_type === "forward_card" ? (() => {
            try {
              const card: ForwardCardData = JSON.parse(message.content);
              return <ForwardCard data={card} isOwn={isOwn} />;
            } catch {
              return <p className="text-sm px-4 py-2.5">[聊天记录]</p>;
            }
          })() : showPreview ? (
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
            <p className="text-sm whitespace-pre-wrap break-words">{message.content}</p>
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
