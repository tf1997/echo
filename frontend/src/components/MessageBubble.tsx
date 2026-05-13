import { useState, useEffect } from "react";
import type { ChatMessage } from "../types";
import { readFileBase64, openFile, openFolder } from "../api";

const MAX_PREVIEW_BYTES = 2 * 1024 * 1024; // 2MB

interface MessageBubbleProps {
  message: ChatMessage;
  isOwn: boolean;
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
  return ["png", "jpg", "jpeg", "gif", "webp", "bmp"].includes(ext);
}

function handleOpenFile(filePath: string | null) {
  if (filePath) {
    openFile(filePath).catch(console.error);
  }
}

function handleOpenFolder(filePath: string | null) {
  if (filePath) {
    openFolder(filePath).catch(console.error);
  }
}

function ImagePreview({ filePath, fileSize }: { filePath: string; fileSize: number | null }) {
  const [src, setSrc] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    if (fileSize !== null && fileSize > MAX_PREVIEW_BYTES) {
      setFailed(true);
      return;
    }

    readFileBase64(filePath)
      .then((data) => setSrc(`data:${data.mime};base64,${data.base64}`))
      .catch(() => setFailed(true));
  }, [filePath, fileSize]);

  if (failed || !src) {
    return null;
  }

  return (
    <img
      src={src}
      alt=""
      className="w-full max-h-[320px] object-cover cursor-pointer"
      onClick={() => handleOpenFile(filePath)}
      onError={() => setFailed(true)}
    />
  );
}

export function MessageBubble({ message, isOwn }: MessageBubbleProps) {
  const isFile = message.msg_type === "file";
  const showPreview = isFile && isImageFile(message.file_name) && message.file_path;

  return (
    <div className={`flex ${isOwn ? "justify-end" : "justify-start"} mb-3 px-4`}>
      <div className={`max-w-[70%] ${isOwn ? "items-end" : "items-start"} flex flex-col`}>
        {!isOwn && (
          <span className="text-xs text-gray-400 mb-1 ml-1">{message.sender_name}</span>
        )}

        <div
          className={`rounded-2xl overflow-hidden ${
            isOwn
              ? "bg-indigo-600 text-white rounded-br-md"
              : "bg-gray-700 text-gray-100 rounded-bl-md"
          } ${!showPreview ? "px-4 py-2.5" : ""}`}
        >
          {showPreview ? (
            <div className="max-w-[260px]">
              <ImagePreview filePath={message.file_path!} fileSize={message.file_size} />
              <div className="flex items-center gap-1 px-3 py-2">
                <div
                  className="flex-1 min-w-0 cursor-pointer hover:opacity-80"
                  onClick={() => handleOpenFile(message.file_path)}
                >
                  <p className="text-xs font-medium truncate">{message.file_name}</p>
                  {message.file_size ? (
                    <p className="text-[10px] opacity-70">{formatFileSize(message.file_size)}</p>
                  ) : null}
                </div>
                <button
                  onClick={(e) => { e.stopPropagation(); handleOpenFolder(message.file_path); }}
                  className="flex-shrink-0 w-6 h-6 rounded flex items-center justify-center hover:bg-white/10"
                  title="在文件夹中显示"
                >
                  <svg className="w-3.5 h-3.5 opacity-70" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z" />
                  </svg>
                </button>
              </div>
            </div>
          ) : isFile ? (
            <div className="flex items-center gap-2">
              <div
                className="flex items-center gap-2 flex-1 min-w-0 cursor-pointer hover:opacity-80"
                onClick={() => handleOpenFile(message.file_path)}
                title="点击打开文件"
              >
                <svg className="w-5 h-5 flex-shrink-0 opacity-80" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 10v6m0 0l-3-3m3 3l3-3m2 8H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z" />
                </svg>
                <div className="min-w-0">
                  <p className="text-sm font-medium truncate">{message.file_name || "文件"}</p>
                  {message.file_size ? (
                    <p className="text-xs opacity-70">{formatFileSize(message.file_size)}</p>
                  ) : null}
                </div>
              </div>
              <button
                onClick={(e) => { e.stopPropagation(); handleOpenFolder(message.file_path); }}
                className="flex-shrink-0 w-6 h-6 rounded flex items-center justify-center hover:bg-white/10"
                title="在文件夹中显示"
              >
                <svg className="w-3.5 h-3.5 opacity-70" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z" />
                </svg>
              </button>
            </div>
          ) : (
            <p className="text-sm whitespace-pre-wrap break-words">{message.content}</p>
          )}
        </div>

        <span className={`text-[10px] text-gray-500 mt-1 ${isOwn ? "mr-1" : "ml-1"}`}>
          {formatTime(message.timestamp)}
          {isOwn && (
            <span className="ml-1">{message.is_read ? "✓✓" : "✓"}</span>
          )}
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
