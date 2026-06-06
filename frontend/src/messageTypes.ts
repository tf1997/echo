export const MESSAGE_TYPE_NUDGE = "nudge";
export const MESSAGE_TYPE_RPS = "rps";
export const NUDGE_MESSAGE_CONTENT = "抖了一下";
export const NUDGE_COOLDOWN_MS = 10_000;

export type RpsMove = "rock" | "paper" | "scissors";

export const RPS_MOVES: RpsMove[] = ["rock", "scissors", "paper"];

export function isNudgeMessageType(msgType: string): boolean {
  return msgType === MESSAGE_TYPE_NUDGE;
}

export function getRpsMoveLabel(move: RpsMove): string {
  if (move === "rock") return "石头";
  if (move === "scissors") return "剪刀";
  return "布";
}

export function getRpsMessageContent(move: RpsMove): string {
  return `猜拳：${getRpsMoveLabel(move)}`;
}

export function parseRpsMoveFromContent(content: string): RpsMove | null {
  if (content.includes("石头")) return "rock";
  if (content.includes("剪刀")) return "scissors";
  if (content.includes("布")) return "paper";
  return null;
}
