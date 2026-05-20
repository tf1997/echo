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

  useEffect(() => {
    if (fileSize !== null && fileSize > MAX_PREVIEW_BYTES) { setFailed(true); return; }
    readFileBase64(filePath)
      .then((data) => setSrc(`data:${data.mime};base64,${data.base64}`))
      .catch(() => setFailed(true));
  }, [filePath, fileSize]);

  const openInNewWindow = useCallback(() => {
    if (!src) return;
    const win = window.open("", "_blank", "width=900,height=700,menubar=no,toolbar=no,resizable=yes");
    if (!win) return;

    // Extract filename from filePath for title
    const fileName = filePath.replace(/\\/g, "/").split("/").pop() || "图片预览";

    win.document.write(`<!DOCTYPE html>
<html>
<head>
  <meta charset="UTF-8">
  <title>${fileName}</title>
  <style>
    * { margin: 0; padding: 0; box-sizing: border-box; }
    body {
      width: 100vw;
      height: 100vh;
      overflow: hidden;
      background: rgba(0,0,0,0.85);
      font-family: system-ui, -apple-system, sans-serif;
      user-select: none;
      cursor: default;
    }
    .container {
      position: relative;
      width: 100%;
      height: 100%;
      display: flex;
      align-items: center;
      justify-content: center;
    }
    .topbar {
      position: absolute;
      top: 0;
      left: 0;
      right: 0;
      display: flex;
      align-items: center;
      justify-content: space-between;
      padding: 12px 16px;
      background: linear-gradient(to bottom, rgba(0,0,0,0.6), transparent);
      z-index: 10;
    }
    .zoom-label {
      color: rgba(255,255,255,0.7);
      font-size: 12px;
    }
    .controls {
      display: flex;
      align-items: center;
      gap: 6px;
    }
    .btn {
      width: 32px;
      height: 32px;
      border: none;
      border-radius: 8px;
      background: rgba(255,255,255,0.1);
      color: white;
      cursor: pointer;
      display: flex;
      align-items: center;
      justify-content: center;
      font-size: 16px;
      transition: background 0.15s;
    }
    .btn:hover { background: rgba(255,255,255,0.2); }
    .close-btn {
      width: 36px;
      height: 36px;
      border-radius: 50%;
      font-size: 20px;
    }
    img {
      max-width: 95%;
      max-height: 95vh;
      object-fit: contain;
      cursor: grab;
      transition: transform 0.15s ease-out;
      transform-origin: center center;
    }
    img.dragging { cursor: grabbing; transition: none; }
    img.scaled { cursor: grab; }
  </style>
</head>
<body>
  <div class="container" id="container">
    <div class="topbar">
      <span class="zoom-label" id="zoomLabel">100%</span>
      <div class="controls">
        <button class="btn" id="zoomIn" title="放大">+</button>
        <button class="btn" id="zoomOut" title="缩小">−</button>
        <button class="btn" id="reset" title="重置">1:1</button>
        <button class="btn close-btn" id="close" title="关闭">×</button>
      </div>
    </div>
    <img id="img" src="${src}" />
  </div>
  <script>
    (function() {
      const img = document.getElementById('img');
      const container = document.getElementById('container');
      const zoomLabel = document.getElementById('zoomLabel');
      const zoomIn = document.getElementById('zoomIn');
      const zoomOut = document.getElementById('zoomOut');
      const reset = document.getElementById('reset');
      const close = document.getElementById('close');

      let scale = 1;
      let panX = 0;
      let panY = 0;
      let dragging = false;
      let dragStartX = 0;
      let dragStartY = 0;
      let panStartX = 0;
      let panStartY = 0;

      function updateTransform() {
        img.style.transform = 'translate(' + panX + 'px, ' + panY + 'px) scale(' + scale + ')';
        zoomLabel.textContent = Math.round(scale * 100) + '%';
      }

      zoomIn.addEventListener('click', function(e) {
        e.stopPropagation();
        scale = Math.min(10, scale + 0.25);
        updateTransform();
      });

      zoomOut.addEventListener('click', function(e) {
        e.stopPropagation();
        scale = Math.max(0.1, scale - 0.25);
        updateTransform();
      });

      reset.addEventListener('click', function(e) {
        e.stopPropagation();
        scale = 1;
        panX = 0;
        panY = 0;
        updateTransform();
      });

      close.addEventListener('click', function() {
        window.close();
      });

      img.addEventListener('wheel', function(e) {
        e.preventDefault();
        const delta = e.deltaY > 0 ? -0.15 : 0.15;
        scale = Math.min(10, Math.max(0.1, scale + delta));
        updateTransform();
      });

      img.addEventListener('mousedown', function(e) {
        if (scale <= 1) return;
        e.preventDefault();
        dragging = true;
        dragStartX = e.clientX;
        dragStartY = e.clientY;
        panStartX = panX;
        panStartY = panY;
        img.classList.add('dragging');
      });

      document.addEventListener('mousemove', function(e) {
        if (!dragging) return;
        panX = panStartX + (e.clientX - dragStartX);
        panY = panStartY + (e.clientY - dragStartY);
        updateTransform();
      });

      document.addEventListener('mouseup', function() {
        dragging = false;
        img.classList.remove('dragging');
      });

      img.addEventListener('dblclick', function() {
        scale = 1;
        panX = 0;
        panY = 0;
        updateTransform();
      });

      // Update cursor based on scale
      function updateCursor() {
        if (scale > 1) {
          img.classList.add('scaled');
        } else {
          img.classList.remove('scaled');
        }
      }
      updateCursor();

      // Keyboard shortcuts
      document.addEventListener('keydown', function(e) {
        if (e.key === 'Escape') window.close();
        if (e.key === '+' || e.key === '=') {
          scale = Math.min(10, scale + 0.25);
          updateTransform();
        }
        if (e.key === '-') {
          scale = Math.max(0.1, scale - 0.25);
          updateTransform();
        }
      });
    })();
  </script>
</body>
</html>`);
    win.document.close();
  }, [src, filePath]);

  if (failed || !src) return null;
  return (
    <>
      <img src={src} alt="" className="w-full max-h-[320px] object-contain cursor-pointer rounded hover:opacity-90 transition-opacity"
        onClick={openInNewWindow} onError={() => setFailed(true)} />
      <div className="px-3 py-1.5 text-[10px] text-gray-400">点击查看大图</div>
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
