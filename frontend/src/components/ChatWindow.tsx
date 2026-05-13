import { useState, useRef, useEffect, useCallback } from "react";
import type { ChatMessage, Peer } from "../types";
import { MessageBubble } from "./MessageBubble";

interface ChatWindowProps {
  peer: Peer | null;
  messages: ChatMessage[];
  myId: string;
  onSendMessage: (content: string) => void;
  onSendFile: (filePath: string) => void;
}

export function ChatWindow({ peer, messages, myId, onSendMessage, onSendFile }: ChatWindowProps) {
  const [inputText, setInputText] = useState("");
  const [isDragging, setIsDragging] = useState(false);
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  // Auto-scroll to bottom on new messages
  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages]);

  // Focus input when peer changes
  useEffect(() => {
    if (peer) {
      inputRef.current?.focus();
    }
  }, [peer]);

  const handleSend = useCallback(() => {
    const trimmed = inputText.trim();
    if (!trimmed || !peer) return;
    onSendMessage(trimmed);
    setInputText("");
    // Reset textarea height
    if (inputRef.current) {
      inputRef.current.style.height = "auto";
    }
  }, [inputText, peer, onSendMessage]);

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  // Auto-resize textarea
  const handleInputChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    setInputText(e.target.value);
    const el = e.target;
    el.style.height = "auto";
    el.style.height = Math.min(el.scrollHeight, 120) + "px";
  };

  // Drag and drop file handling
  const handleDragOver = (e: React.DragEvent) => {
    e.preventDefault();
    setIsDragging(true);
  };

  const handleDragLeave = (e: React.DragEvent) => {
    e.preventDefault();
    setIsDragging(false);
  };

  const handleDrop = (e: React.DragEvent) => {
    e.preventDefault();
    setIsDragging(false);
    if (!peer) return;

    // In Tauri, we get file paths from the drag event
    const files = e.dataTransfer.files;
    for (let i = 0; i < files.length; i++) {
      const file = files[i];
      // @ts-expect-error Tauri adds path property to File
      const filePath = file.path;
      if (filePath) {
        onSendFile(filePath);
      }
    }
  };

  // Empty state: no peer selected
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

  return (
    <div
      className="flex-1 flex flex-col bg-gray-800 h-full relative"
      onDragOver={handleDragOver}
      onDragLeave={handleDragLeave}
      onDrop={handleDrop}
    >
      {/* Drag overlay */}
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

      {/* Header */}
      <div className="flex items-center gap-3 px-5 py-3 bg-gray-900/80 border-b border-gray-700 backdrop-blur">
        <div className="relative">
          <div className="w-9 h-9 rounded-full bg-gray-600 flex items-center justify-center text-sm font-medium text-white">
            {peer.username.charAt(0).toUpperCase()}
          </div>
          <div
            className={`absolute -bottom-0.5 -right-0.5 w-2.5 h-2.5 rounded-full border-2 border-gray-900 ${
              peer.online ? "bg-green-400" : "bg-gray-500"
            }`}
          />
        </div>
        <div>
          <p className="text-white text-sm font-semibold">{peer.username}</p>
          <p className="text-xs text-gray-400">
            {peer.online ? `${peer.ip}:${peer.port}` : "离线"}
          </p>
        </div>
      </div>

      {/* Messages */}
      <div className="flex-1 overflow-y-auto py-4">
        {messages.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-full text-gray-500">
            <p className="text-sm">暂无消息</p>
            <p className="text-xs mt-1">向 {peer.username} 发送第一条消息吧</p>
          </div>
        ) : (
          messages.map((msg) => (
            <MessageBubble
              key={msg.id}
              message={msg}
              isOwn={msg.sender_id === myId}
            />
          ))
        )}
        <div ref={messagesEndRef} />
      </div>

      {/* Input area */}
      <div className="px-4 py-3 border-t border-gray-700 bg-gray-900/50">
        <div className="flex items-end gap-3">
          <textarea
            ref={inputRef}
            value={inputText}
            onChange={handleInputChange}
            onKeyDown={handleKeyDown}
            placeholder={peer.online ? `发送消息给 ${peer.username}...` : "对方离线，消息将在上线后发送"}
            rows={1}
            className="flex-1 bg-gray-700 text-white text-sm rounded-xl px-4 py-2.5 outline-none focus:ring-2 focus:ring-indigo-500 placeholder-gray-400 resize-none max-h-[120px]"
          />
          <button
            onClick={handleSend}
            disabled={!inputText.trim()}
            className="flex-shrink-0 w-10 h-10 rounded-xl bg-indigo-600 hover:bg-indigo-500 disabled:opacity-40 disabled:hover:bg-indigo-600 transition-colors flex items-center justify-center"
          >
            <svg className="w-5 h-5 text-white" fill="none" viewBox="0 0 24 24" stroke="currentColor">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 19l9 2-9-18-9 18 9-2zm0 0v-8" />
            </svg>
          </button>
        </div>
        <p className="text-[10px] text-gray-600 mt-1.5 ml-1">
          Enter 发送 · Shift+Enter 换行 · 拖拽文件发送
        </p>
      </div>
    </div>
  );
}