import { useState, useRef, useEffect, useCallback } from "react";
import type { ChatMessage, Peer } from "../types";
import { MessageBubble } from "./MessageBubble";
import { saveTempFile } from "../api";

export interface PendingMessage {
  id: number;
  content: string;
  msg_type: string;
  file_name?: string;
  file_path?: string;
  file_size?: number;
  status: "sending" | "failed";
  error?: string;
}

interface ChatWindowProps {
  peer: Peer | null;
  messages: ChatMessage[];
  myId: string;
  onSendMessage: (content: string) => Promise<ChatMessage>;
  onSendFile: (filePath: string) => Promise<ChatMessage>;
}

let pendingId = Date.now();

async function readFileAndSave(file: File): Promise<string> {
  const buffer = await file.arrayBuffer();
  const data = Array.from(new Uint8Array(buffer));
  return await saveTempFile(data, file.name || "file");
}

export function ChatWindow({ peer, messages, myId, onSendMessage, onSendFile }: ChatWindowProps) {
  const [inputText, setInputText] = useState("");
  const [isDragging, setIsDragging] = useState(false);
  const [pendingMessages, setPendingMessages] = useState<PendingMessage[]>([]);
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const messagesContainerRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const nearBottomRef = useRef(true);

  // Clear pending when switching peers + focus input
  useEffect(() => {
    setPendingMessages([]);
    if (peer) {
      // Small delay to let React finish rendering
      requestAnimationFrame(() => inputRef.current?.focus());
    }
  }, [peer?.id]);

  const handleScroll = useCallback(() => {
    const el = messagesContainerRef.current;
    if (!el) return;
    nearBottomRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 100;
  }, []);

  useEffect(() => {
    if (nearBottomRef.current) {
      messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
    }
  }, [messages, pendingMessages]);

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
        await onSendFile(filePath);
        setPendingMessages((prev) => prev.filter((p) => p.id !== tempId));
        return;
      }
    } catch {
      // no path
    }

    try {
      const savedPath = await readFileAndSave(file);
      await onSendFile(savedPath);
      setPendingMessages((prev) => prev.filter((p) => p.id !== tempId));
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

  const handleDragOver = (e: React.DragEvent) => { e.preventDefault(); setIsDragging(true); };
  const handleDragLeave = (e: React.DragEvent) => { e.preventDefault(); setIsDragging(false); };

  const handleDrop = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    setIsDragging(false);
    if (!peer) return;
    for (let i = 0; i < e.dataTransfer.files.length; i++) {
      sendFileToPeer(e.dataTransfer.files[i]);
    }
  }, [peer, sendFileToPeer]);

  const handlePickFile = () => fileInputRef.current?.click();

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

  return (
    <div
      className="flex-1 flex flex-col bg-gray-800 h-full relative"
      onDragOver={handleDragOver}
      onDragLeave={handleDragLeave}
      onDrop={handleDrop}
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

      <div className="flex items-center gap-3 px-5 py-3 bg-gray-900/80 border-b border-gray-700 backdrop-blur">
        <div className="relative">
          <div className="w-9 h-9 rounded-full bg-gray-600 flex items-center justify-center text-sm font-medium text-white">
            {peer.username.charAt(0).toUpperCase()}
          </div>
          <div className={`absolute -bottom-0.5 -right-0.5 w-2.5 h-2.5 rounded-full border-2 border-gray-900 ${peer.online ? "bg-green-400" : "bg-gray-500"}`} />
        </div>
        <div>
          <p className="text-white text-sm font-semibold">{peer.username}</p>
          <p className="text-xs text-gray-400">{peer.online ? `${peer.ip}:${peer.port}` : "离线"}</p>
        </div>
      </div>

      <div ref={messagesContainerRef} onScroll={handleScroll} className="flex-1 overflow-y-auto py-4">
        {allItems.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-full text-gray-500">
            <p className="text-sm">暂无消息</p>
            <p className="text-xs mt-1">向 {peer.username} 发送第一条消息吧</p>
          </div>
        ) : (
          allItems.map((item) => {
            if ("status" in item) {
              // Pending message
              return (
                <div key={`pending-${item.id}`} className="flex justify-end mb-3 px-4">
                  <div className="max-w-[70%] flex flex-col items-end">
                    <div className={`rounded-2xl px-4 py-2.5 rounded-br-md ${
                      item.status === "failed" ? "bg-red-600/30 border border-red-500/50" : "bg-indigo-600/50"
                    } text-white`}>
                      {item.msg_type === "file" ? (
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
                    <div className="flex items-center gap-2 mt-1">
                      <span className="text-[10px] text-gray-500">{item.status === "failed" ? "发送失败" : "发送中..."}</span>
                      {item.status === "failed" && (
                        <button
                          onClick={() => {
                            if (item.msg_type === "file") {
                              retryFile(item);
                            } else {
                              retryText(item);
                            }
                          }}
                          className="text-[10px] text-indigo-400 hover:text-indigo-300"
                        >
                          重试
                        </button>
                      )}
                    </div>
                  </div>
                </div>
              );
            }
            return (
              <MessageBubble key={item.id} message={item} isOwn={item.sender_id === myId} />
            );
          })
        )}
        <div ref={messagesEndRef} />
      </div>

      <div className="px-4 py-3 border-t border-gray-700 bg-gray-900/50">
        <div className="flex items-end gap-2">
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
  );
}
