export const MESSAGE_TYPE_NUDGE = "nudge";
export const NUDGE_MESSAGE_CONTENT = "抖了一下";
export const NUDGE_COOLDOWN_MS = 10_000;

export function isNudgeMessageType(msgType: string): boolean {
  return msgType === MESSAGE_TYPE_NUDGE;
}
