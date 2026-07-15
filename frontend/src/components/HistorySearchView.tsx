import { useCallback, useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import type { ChatMessage, Peer } from "../types";
import { isSameIdentity } from "../types";
import {
  getConversationHistory,
  getGroupHistory,
  searchConversationMessages,
  searchGroupChatMessages,
} from "../api";
import type { HistoryFilter } from "../api";
import { DateDivider } from "./MessageBubble";
import { formatDateLabel } from "./messageUtils";
import { MESSAGE_TYPE_NUDGE, MESSAGE_TYPE_RPS } from "../messageTypes";

const HISTORY_PAGE_LIMIT = 80;
const HISTORY_SEARCH_LIMIT = 200;
const WEEKDAYS = ["日", "一", "二", "三", "四", "五", "六"];

const FILTERS: { id: HistoryFilter; label: string }[] = [
  { id: "all", label: "全部" },
  { id: "file", label: "文件" },
  { id: "image", label: "图片" },
];

interface HistorySearchViewProps {
  peer: Peer;
  myId: string;
  myNodeId: string;
  isGroup: boolean;
  groupId?: string | null;
  initialSearchRequest?: {
    query: string;
    messageId?: number | null;
    nonce: number;
  } | null;
  onJumpToMessage?: (messageId: number) => void;
  onClose: () => void;
}

function formatTimestamp(ts: string): string {
  try {
    return new Date(ts).toLocaleTimeString("zh-CN", {
      hour: "2-digit",
      minute: "2-digit",
    });
  } catch {
    return "";
  }
}

function getResultText(message: ChatMessage): string {
  if (message.msg_type === "file") return message.file_name || message.content || "[文件]";
  if (message.msg_type === "sticker") return message.file_name || "[表情]";
  if (message.msg_type === "forward_card") return "[聊天记录]";
  if (message.msg_type === MESSAGE_TYPE_NUDGE) return "[抖一抖]";
  if (message.msg_type === MESSAGE_TYPE_RPS) return message.content || "[猜拳]";
  return message.content;
}

function getTypeLabel(message: ChatMessage): string {
  if (message.msg_type === "file") return "文件";
  if (message.msg_type === "sticker") return "图片";
  if (message.msg_type === "forward_card") return "聊天记录";
  if (message.msg_type === MESSAGE_TYPE_NUDGE) return "抖一抖";
  if (message.msg_type === MESSAGE_TYPE_RPS) return "猜拳";
  return "";
}

function highlightText(text: string, query: string): ReactNode {
  const needle = query.trim();
  if (!needle) return text;

  const lowerText = text.toLowerCase();
  const lowerNeedle = needle.toLowerCase();
  const parts: ReactNode[] = [];
  let cursor = 0;
  let index = lowerText.indexOf(lowerNeedle, cursor);
  let key = 0;

  while (index !== -1) {
    if (index > cursor) parts.push(text.slice(cursor, index));
    const end = index + needle.length;
    parts.push(
      <mark key={key++} className="history-highlight">
        {text.slice(index, end)}
      </mark>
    );
    cursor = end;
    index = lowerText.indexOf(lowerNeedle, cursor);
  }

  if (cursor < text.length) parts.push(text.slice(cursor));
  return parts.length > 0 ? parts : text;
}

function mergeOlderMessages(current: ChatMessage[], older: ChatMessage[]) {
  const seen = new Set(current.map((message) => message.id));
  return [...older.filter((message) => !seen.has(message.id)), ...current];
}

function getDayRange(day: string) {
  if (!day) return { dayStart: undefined, dayEnd: undefined };
  const start = new Date(`${day}T00:00:00`);
  const end = new Date(start);
  end.setDate(start.getDate() + 1);
  return {
    dayStart: start.toISOString(),
    dayEnd: end.toISOString(),
  };
}

function getMonthStart(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), 1);
}

function addMonths(date: Date, amount: number): Date {
  return new Date(date.getFullYear(), date.getMonth() + amount, 1);
}

function formatLocalDayValue(date: Date): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function parseLocalDayValue(day: string): Date | null {
  const parts = day.split("-").map(Number);
  if (parts.length !== 3 || parts.some((part) => !Number.isFinite(part))) return null;
  const [year, month, date] = parts;
  const parsed = new Date(year, month - 1, date);
  if (parsed.getFullYear() !== year || parsed.getMonth() !== month - 1 || parsed.getDate() !== date) {
    return null;
  }
  return parsed;
}

function formatSelectedDayLabel(day: string): string {
  const date = parseLocalDayValue(day);
  if (!date) return "全部日期";
  return date.toLocaleDateString("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  });
}

function formatCalendarMonth(date: Date): string {
  return date.toLocaleDateString("zh-CN", {
    year: "numeric",
    month: "long",
  });
}

function getCalendarCells(month: Date): (Date | null)[] {
  const firstDay = getMonthStart(month);
  const daysInMonth = new Date(firstDay.getFullYear(), firstDay.getMonth() + 1, 0).getDate();
  const cells: (Date | null)[] = Array.from({ length: firstDay.getDay() }, () => null);
  for (let day = 1; day <= daysInMonth; day += 1) {
    cells.push(new Date(firstDay.getFullYear(), firstDay.getMonth(), day));
  }
  while (cells.length % 7 !== 0) cells.push(null);
  return cells;
}

export function HistorySearchView({ peer, myId, myNodeId, isGroup, groupId, initialSearchRequest = null, onJumpToMessage, onClose }: HistorySearchViewProps) {
  const [filter, setFilter] = useState<HistoryFilter>("all");
  const [selectedDay, setSelectedDay] = useState("");
  const [datePickerOpen, setDatePickerOpen] = useState(false);
  const [pickerMonth, setPickerMonth] = useState(() => getMonthStart(new Date()));
  const [query, setQuery] = useState(() => initialSearchRequest?.query ?? "");
  const [items, setItems] = useState<ChatMessage[]>([]);
  const [results, setResults] = useState<ChatMessage[]>([]);
  const [focusedMessageId, setFocusedMessageId] = useState<number | null>(() => initialSearchRequest?.messageId ?? null);
  const [hasMore, setHasMore] = useState(true);
  const [loadingInitial, setLoadingInitial] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [searching, setSearching] = useState(false);
  const [error, setError] = useState("");
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const datePickerRef = useRef<HTMLDivElement | null>(null);
  const timerRef = useRef<number | null>(null);
  const historySeqRef = useRef(0);
  const searchSeqRef = useRef(0);
  const mountedRef = useRef(true);
  const trimmedQuery = query.trim();
  const targetGroupId = isGroup ? groupId ?? peer.id : null;
  const { dayStart, dayEnd } = getDayRange(selectedDay);

  const fetchHistory = useCallback((beforeId: number | undefined, activeFilter: HistoryFilter) => {
    return targetGroupId
      ? getGroupHistory(targetGroupId, beforeId, HISTORY_PAGE_LIMIT, activeFilter, dayStart, dayEnd)
      : getConversationHistory(peer.id, beforeId, HISTORY_PAGE_LIMIT, activeFilter, dayStart, dayEnd);
  }, [dayEnd, dayStart, peer.id, targetGroupId]);

  const loadInitial = useCallback(async (activeFilter: HistoryFilter) => {
    const seq = historySeqRef.current + 1;
    historySeqRef.current = seq;
    setError("");
    setLoadingInitial(true);
    try {
      const nextItems = await fetchHistory(undefined, activeFilter);
      if (!mountedRef.current || historySeqRef.current !== seq) return;
      setItems(nextItems);
      setHasMore(nextItems.length === HISTORY_PAGE_LIMIT);
      requestAnimationFrame(() => {
        const el = scrollRef.current;
        if (el) el.scrollTop = el.scrollHeight;
      });
    } catch (err) {
      if (mountedRef.current && historySeqRef.current === seq) {
        setItems([]);
        setHasMore(false);
        setError(String(err));
      }
    } finally {
      if (mountedRef.current && historySeqRef.current === seq) {
        setLoadingInitial(false);
      }
    }
  }, [fetchHistory]);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      void loadInitial(filter);
    }, 0);
    return () => window.clearTimeout(timer);
  }, [filter, loadInitial]);

  useEffect(() => {
    return () => {
      mountedRef.current = false;
      if (timerRef.current !== null) {
        window.clearTimeout(timerRef.current);
      }
    };
  }, []);

  useEffect(() => {
    if (!datePickerOpen) return;

    const handlePointerDown = (event: MouseEvent | TouchEvent) => {
      const target = event.target;
      if (target instanceof Node && datePickerRef.current && !datePickerRef.current.contains(target)) {
        setDatePickerOpen(false);
      }
    };

    window.addEventListener("mousedown", handlePointerDown);
    window.addEventListener("touchstart", handlePointerDown);
    return () => {
      window.removeEventListener("mousedown", handlePointerDown);
      window.removeEventListener("touchstart", handlePointerDown);
    };
  }, [datePickerOpen]);

  const loadOlder = useCallback(async () => {
    if (trimmedQuery || loadingInitial || loadingMore || !hasMore || items.length === 0) return;

    const firstId = items[0].id;
    const el = scrollRef.current;
    const previousHeight = el?.scrollHeight ?? 0;
    const previousTop = el?.scrollTop ?? 0;
    const seq = historySeqRef.current;

    setLoadingMore(true);
    try {
      const olderItems = await fetchHistory(firstId, filter);
      if (!mountedRef.current || historySeqRef.current !== seq) return;
      setItems((current) => mergeOlderMessages(current, olderItems));
      setHasMore(olderItems.length === HISTORY_PAGE_LIMIT);
      requestAnimationFrame(() => {
        const nextEl = scrollRef.current;
        if (nextEl) {
          nextEl.scrollTop = nextEl.scrollHeight - previousHeight + previousTop;
        }
      });
    } catch (err) {
      if (mountedRef.current && historySeqRef.current === seq) {
        setError(String(err));
      }
    } finally {
      if (mountedRef.current && historySeqRef.current === seq) {
        setLoadingMore(false);
      }
    }
  }, [filter, fetchHistory, hasMore, items, loadingInitial, loadingMore, trimmedQuery]);

  const queueSearch = useCallback((value: string, activeFilter: HistoryFilter) => {
    const term = value.trim();
    const seq = searchSeqRef.current + 1;
    searchSeqRef.current = seq;

    if (timerRef.current !== null) {
      window.clearTimeout(timerRef.current);
      timerRef.current = null;
    }

    setError("");

    if (!term) {
      setResults([]);
      setSearching(false);
      return;
    }

    setSearching(true);
    timerRef.current = window.setTimeout(() => {
      timerRef.current = null;
      const request = targetGroupId
        ? searchGroupChatMessages(targetGroupId, term, HISTORY_SEARCH_LIMIT, activeFilter, dayStart, dayEnd)
        : searchConversationMessages(peer.id, term, HISTORY_SEARCH_LIMIT, activeFilter, dayStart, dayEnd);

      request
        .then((nextResults) => {
          if (mountedRef.current && searchSeqRef.current === seq) {
            setResults(nextResults);
          }
        })
        .catch((err) => {
          if (mountedRef.current && searchSeqRef.current === seq) {
            setResults([]);
            setError(String(err));
          }
        })
        .finally(() => {
          if (mountedRef.current && searchSeqRef.current === seq) {
            setSearching(false);
          }
        });
    }, 250);
  }, [dayEnd, dayStart, peer.id, targetGroupId]);

  useEffect(() => {
    const term = initialSearchRequest?.query.trim() ?? "";
    if (!term) return;

    let cancelled = false;
    queueMicrotask(() => {
      if (cancelled) return;
      setFilter("all");
      setSelectedDay("");
      setDatePickerOpen(false);
      setQuery(term);
      setFocusedMessageId(initialSearchRequest?.messageId ?? null);
      queueSearch(term, "all");
    });

    return () => {
      cancelled = true;
    };
  }, [initialSearchRequest?.messageId, initialSearchRequest?.nonce, initialSearchRequest?.query, queueSearch]);

  useEffect(() => {
    if (!focusedMessageId || !trimmedQuery || searching) return;
    if (!results.some((message) => message.id === focusedMessageId)) return;

    requestAnimationFrame(() => {
      const target = scrollRef.current?.querySelector<HTMLElement>(`[data-history-message-id="${focusedMessageId}"]`);
      target?.scrollIntoView({ behavior: "smooth", block: "center" });
    });
  }, [focusedMessageId, results, searching, trimmedQuery]);

  const handleDayChange = useCallback((day: string) => {
    setSelectedDay(day);
    if (query.trim()) {
      const range = getDayRange(day);
      const term = query.trim();
      const seq = searchSeqRef.current + 1;
      searchSeqRef.current = seq;
      if (timerRef.current !== null) {
        window.clearTimeout(timerRef.current);
        timerRef.current = null;
      }
      setError("");
      setSearching(true);
      const request = targetGroupId
        ? searchGroupChatMessages(targetGroupId, term, HISTORY_SEARCH_LIMIT, filter, range.dayStart, range.dayEnd)
        : searchConversationMessages(peer.id, term, HISTORY_SEARCH_LIMIT, filter, range.dayStart, range.dayEnd);
      request
        .then((nextResults) => {
          if (mountedRef.current && searchSeqRef.current === seq) setResults(nextResults);
        })
        .catch((err) => {
          if (mountedRef.current && searchSeqRef.current === seq) {
            setResults([]);
            setError(String(err));
          }
        })
        .finally(() => {
          if (mountedRef.current && searchSeqRef.current === seq) setSearching(false);
      });
    }
  }, [filter, peer.id, query, targetGroupId]);

  const handleToggleDatePicker = useCallback(() => {
    if (!datePickerOpen) {
      setPickerMonth(getMonthStart(parseLocalDayValue(selectedDay) ?? new Date()));
    }
    setDatePickerOpen((open) => !open);
  }, [datePickerOpen, selectedDay]);

  const handleDateCandidateSelect = useCallback((day: string) => {
    handleDayChange(day);
    setDatePickerOpen(false);
  }, [handleDayChange]);

  const handleFilterChange = useCallback((nextFilter: HistoryFilter) => {
    if (nextFilter === filter) return;
    setFilter(nextFilter);
    if (query.trim()) {
      queueSearch(query, nextFilter);
    }
  }, [filter, query, queueSearch]);

  const handleScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el || el.scrollTop > 80) return;
    void loadOlder();
  }, [loadOlder]);

  const visibleMessages = trimmedQuery ? results : items;
  const calendarCells = getCalendarCells(pickerMonth);
  const todayValue = formatLocalDayValue(new Date());
  const selectedDayLabel = selectedDay ? formatSelectedDayLabel(selectedDay) : "全部日期";
  let lastDateLabel = "";

  return (
    <div className="flex-1 min-h-0 flex flex-col bg-gray-800">
      <div className="history-toolbar flex items-center gap-2 px-4 py-2 border-b border-gray-700">
        <div className="relative flex-1 min-w-0">
          <svg className="w-4 h-4 absolute left-2.5 top-1/2 -translate-y-1/2 text-gray-500" fill="none" viewBox="0 0 24 24" stroke="currentColor">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
          </svg>
          <input
            autoFocus
            value={query}
            onChange={(event) => {
              setFocusedMessageId(null);
              setQuery(event.target.value);
              queueSearch(event.target.value, filter);
            }}
            onKeyDown={(event) => {
              if (event.key === "Escape") onClose();
            }}
            placeholder="搜索全部聊天记录..."
            className="w-full bg-gray-700 text-white text-sm rounded-lg pl-8 pr-8 py-1.5 outline-none focus:ring-1 focus:ring-indigo-500 placeholder-gray-400"
          />
          {query ? (
            <button
              type="button"
              onClick={() => {
                setFocusedMessageId(null);
                setQuery("");
                queueSearch("", filter);
              }}
              className="history-icon-button absolute right-2 top-1/2 -translate-y-1/2 w-5 h-5 rounded-full flex items-center justify-center"
              aria-label="清空搜索"
            >
              x
            </button>
          ) : null}
        </div>
        <div className="history-filter-tabs flex flex-shrink-0 rounded-lg p-0.5">
          {FILTERS.map((option) => (
            <button
              key={option.id}
              type="button"
              onClick={() => handleFilterChange(option.id)}
              className={`history-filter-button px-2 py-1 text-xs rounded-md ${filter === option.id ? "history-filter-button-active" : ""}`}
            >
              {option.label}
            </button>
          ))}
        </div>
        <div className="flex flex-shrink-0 items-center gap-1">
          <div ref={datePickerRef} className="history-date-picker relative">
            <button
              type="button"
              onClick={handleToggleDatePicker}
              className={`history-date-trigger h-8 rounded-lg px-2 text-xs flex items-center gap-1.5 ${selectedDay ? "history-date-trigger-active" : ""}`}
              aria-expanded={datePickerOpen}
              aria-label="选择日期"
              title="选择日期"
            >
              <svg className="w-3.5 h-3.5 flex-shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M8 7V4m8 3V4M5 11h14M7 5h10a2 2 0 012 2v11a2 2 0 01-2 2H7a2 2 0 01-2-2V7a2 2 0 012-2z" />
              </svg>
              <span className="truncate">{selectedDayLabel}</span>
            </button>
            {datePickerOpen ? (
              <div className="history-date-popover" role="dialog" aria-label="选择日期">
                <div className="history-calendar-header">
                  <button
                    type="button"
                    className="history-calendar-nav"
                    onClick={() => setPickerMonth((current) => addMonths(current, -1))}
                    aria-label="上个月"
                    title="上个月"
                  >
                    <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 19l-7-7 7-7" />
                    </svg>
                  </button>
                  <span>{formatCalendarMonth(pickerMonth)}</span>
                  <button
                    type="button"
                    className="history-calendar-nav"
                    onClick={() => setPickerMonth((current) => addMonths(current, 1))}
                    aria-label="下个月"
                    title="下个月"
                  >
                    <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9 5l7 7-7 7" />
                    </svg>
                  </button>
                </div>
                <div className="history-calendar-weekdays">
                  {WEEKDAYS.map((weekday) => (
                    <span key={weekday}>{weekday}</span>
                  ))}
                </div>
                <div className="history-calendar-grid">
                  {calendarCells.map((date, index) => {
                    if (!date) {
                      return <span key={`empty-${index}`} className="history-calendar-day-empty" />;
                    }
                    const dayValue = formatLocalDayValue(date);
                    const isSelected = selectedDay === dayValue;
                    const isToday = todayValue === dayValue;
                    return (
                      <button
                        key={dayValue}
                        type="button"
                        className={`history-calendar-day ${isToday ? "history-calendar-day-today" : ""} ${isSelected ? "history-calendar-day-active" : ""}`}
                        onClick={() => handleDateCandidateSelect(dayValue)}
                        aria-pressed={isSelected}
                      >
                        {date.getDate()}
                      </button>
                    );
                  })}
                </div>
                <div className="history-calendar-footer">
                  <button type="button" className="history-calendar-text-button" onClick={() => handleDateCandidateSelect(todayValue)}>
                    今天
                  </button>
                  {selectedDay ? (
                    <button type="button" className="history-calendar-text-button" onClick={() => handleDateCandidateSelect("")}>
                      全部日期
                    </button>
                  ) : null}
                </div>
              </div>
            ) : null}
          </div>
          {selectedDay ? (
            <button
              type="button"
              onClick={() => handleDateCandidateSelect("")}
              className="history-icon-button w-7 h-8 rounded-lg flex items-center justify-center"
              title="清除日期"
              aria-label="清除日期"
            >
              x
            </button>
          ) : null}
        </div>
        <button
          type="button"
          onClick={onClose}
          className="history-icon-button flex-shrink-0 w-8 h-8 rounded-lg flex items-center justify-center"
          title="关闭聊天记录"
          aria-label="关闭聊天记录"
        >
          <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 6l12 12M18 6L6 18" />
          </svg>
        </button>
      </div>

      <div ref={scrollRef} onScroll={handleScroll} className="flex-1 overflow-y-auto py-2">
        {!trimmedQuery && loadingMore ? (
          <div className="px-4 py-2 text-center text-xs text-gray-500">加载更早记录...</div>
        ) : null}
        {!trimmedQuery && !hasMore && items.length > 0 ? (
          <div className="px-4 py-2 text-center text-xs text-gray-500">已到最早记录</div>
        ) : null}
        {loadingInitial && !trimmedQuery ? (
          <div className="h-full flex items-center justify-center text-sm text-gray-500">加载中...</div>
        ) : searching && trimmedQuery && results.length === 0 ? (
          <div className="h-full flex items-center justify-center text-sm text-gray-500">搜索中...</div>
        ) : error ? (
          <div className="h-full flex items-center justify-center text-sm text-red-400">{error}</div>
        ) : visibleMessages.length === 0 ? (
          <div className="h-full flex items-center justify-center text-sm text-gray-500">
            {trimmedQuery ? "无匹配记录" : "暂无聊天记录"}
          </div>
        ) : (
          visibleMessages.map((message) => {
            const label = formatDateLabel(message.timestamp);
            const showDivider = label && label !== lastDateLabel;
            if (showDivider) lastDateLabel = label;
            const text = getResultText(message);
            const sender = isSameIdentity(message.sender_node_id, message.sender_id, myNodeId, myId)
              ? "我"
              : message.sender_name;
            const typeLabel = getTypeLabel(message);
            const focused = focusedMessageId === message.id;
            return (
              <div key={message.id}>
                {showDivider ? <DateDivider date={label} /> : null}
                <button
                  type="button"
                  data-history-message-id={message.id}
                  onClick={() => onJumpToMessage?.(message.id)}
                  className={`history-result-row block w-full text-left px-4 py-3 border-b border-gray-700/60 ${focused ? "history-result-row-focused" : ""}`}
                >
                  <div className="flex items-center gap-2 mb-1">
                    <span className="text-xs font-medium text-indigo-300 truncate">{sender}</span>
                    <span className="text-[10px] text-gray-500 flex-shrink-0">{formatTimestamp(message.timestamp)}</span>
                    {typeLabel ? (
                      <span className="text-[10px] text-gray-500 flex-shrink-0">{typeLabel}</span>
                    ) : null}
                    {onJumpToMessage ? (
                      <span className="history-jump-label ml-auto flex-shrink-0">定位</span>
                    ) : null}
                  </div>
                  <p className="text-sm text-gray-200 whitespace-pre-wrap break-words">
                    {highlightText(text, trimmedQuery)}
                  </p>
                </button>
              </div>
            );
          })
        )}
        {trimmedQuery && results.length >= HISTORY_SEARCH_LIMIT ? (
          <div className="px-4 py-3 text-center text-xs text-gray-500">仅显示前 {HISTORY_SEARCH_LIMIT} 条结果</div>
        ) : null}
      </div>
    </div>
  );
}
