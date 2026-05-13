import type { ChatMessage } from "../types";

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

export function MessageBubble({ message, isOwn }: MessageBubbleProps) {
  const isFile = message.msg_type === "file";

  return (
    <div className={`flex ${isOwn ? "justify-end" : "justify-start"} mb-3 px-4`}>
      <div className={`max-w-[70%] ${isOwn ? "items-end" : "items-start"} flex flex-col`}>
        {/* Sender name (for received messages) */}
        {!isOwn && (
          <span className="text-xs text-gray-400 mb-1 ml-1">{message.sender_name}</span>
        )}

        {/* Message bubble */}
        <div
          className={`rounded-2xl px-4 py-2.5 ${
            isOwn
              ? "bg-indigo-600 text-white rounded-br-md"
              : "bg-gray-700 text-gray-100 rounded-bl-md"
          }`}
        >
          {isFile ? (
            <div className="flex items-center gap-2">
              <svg className="w-5 h-5 flex-shrink-0 opacity-80" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 10v6m0 0l-3-3m3 3l3-3m2 8H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z" />
              </svg>
              <div className="min-w-0">
                <p className="text-sm font-medium truncate">{message.file_name || "文件"}</p>
                {message.file_size && (
                  <p className="text-xs opacity-70">{formatFileSize(message.file_size)}</p>
                )}
              </div>
            </div>
          ) : (
            <p className="text-sm whitespace-pre-wrap break-words">{message.content}</p>
          )}
        </div>

        {/* Timestamp */}
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