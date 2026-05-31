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

export function makeSearchHitId(messageId: number, occurrenceIndex: number): string {
  return `search-hit-${messageId}-${occurrenceIndex}`;
}
