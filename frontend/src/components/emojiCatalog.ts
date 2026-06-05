export const DEFAULT_EMOJIS = [
  "😀","😃","😄","😁","😆","😂","🤣","😅","😊","🙂",
  "🙃","😉","😍","🥰","😘","😋","😜","🤪","😎","🤩",
  "🥳","🤭","🤫","🤔","🫡","😏","🙄","😬","😐","😑",
  "😶","🫥","🫣","🫢","😳","😮","😲","😵","😵‍💫","🤯",
  "🥺","😢","😭","😤","😡","🤬","😒","😔","😪","😴",
  "🥱","🤤","😷","🤒","🤧","🤮","😇","😈","🤡","🫠",
  "🤦","🙈","🙉","🙊","👍","👎","👌","🤌","👏","🙌",
  "🙏","🤝","💪","👊","✌️","🤟","👋","🤲","💅","👀",
  "🎉","🔥","❤️","🧡","💛","💚","💙","💜","💔","💯",
  "✅","❌","⭐","🌟","💡","🎵","🌹","☕","🍕","🚀",
  "🐶","🐕","🦮","🐾","🦴","🐱","🐭","🐹","🐰","🦊",
  "🐼","🐻","🐨","🐯","🦁","🐸","🐵","🐧","🐔","🐟",
  "📎","📁","🎂","🏆","🥇","💩"
];

type InlineEmojiSegment =
  | { type: "text"; text: string }
  | { type: "emoji"; id: string; emoji: string; raw: string };

const TOKEN_PATTERN = "\\[echo:([0-9a-f-]+)\\]";
const UFE0F_RE = /\uFE0F/g;
const ZERO_WIDTH_JOINER = "\u200D";
const EMOJI_BY_ID = new Map(DEFAULT_EMOJIS.map((emoji) => [emojiAssetId(emoji), emoji]));
const UNICODE_EMOJI_PATTERN = DEFAULT_EMOJIS
  .slice()
  .sort((a, b) => b.length - a.length)
  .map(escapeRegExp)
  .join("|");
const INLINE_EMOJI_RE = new RegExp(`${TOKEN_PATTERN}|(${UNICODE_EMOJI_PATTERN})`, "gu");
const ECHO_EMOJI_TOKEN_RE = new RegExp(TOKEN_PATTERN, "gi");

export function emojiAssetId(emoji: string): string {
  const normalized = emoji.includes(ZERO_WIDTH_JOINER) ? emoji : emoji.replace(UFE0F_RE, "");
  return Array.from(normalized)
    .map((part) => part.codePointAt(0)?.toString(16) ?? "")
    .filter(Boolean)
    .join("-");
}

export function emojiAssetSrc(id: string): string {
  return `${import.meta.env.BASE_URL}twemoji/svg/${id}.svg`;
}

export function emojiTokenFor(emoji: string): string {
  return `[echo:${emojiAssetId(emoji)}]`;
}

export function encodeDefaultEmojisAsTokens(text: string): string {
  return splitInlineEmojis(text)
    .map((segment) => segment.type === "emoji" ? `[echo:${segment.id}]` : segment.text)
    .join("");
}

export function decodeEchoEmojiTokens(text: string): string {
  return text.replace(ECHO_EMOJI_TOKEN_RE, (raw, id: string) => EMOJI_BY_ID.get(id.toLowerCase()) ?? raw);
}

export function splitInlineEmojis(text: string): InlineEmojiSegment[] {
  const segments: InlineEmojiSegment[] = [];
  let cursor = 0;
  INLINE_EMOJI_RE.lastIndex = 0;
  let match = INLINE_EMOJI_RE.exec(text);

  while (match) {
    if (match.index > cursor) {
      segments.push({ type: "text", text: text.slice(cursor, match.index) });
    }

    const tokenId = match[1]?.toLowerCase();
    const unicodeEmoji = match[2];
    const id = tokenId || emojiAssetId(unicodeEmoji ?? match[0]);
    segments.push({
      type: "emoji",
      id,
      emoji: EMOJI_BY_ID.get(id) ?? unicodeEmoji ?? match[0],
      raw: match[0],
    });

    cursor = match.index + match[0].length;
    match = INLINE_EMOJI_RE.exec(text);
  }

  if (cursor < text.length) {
    segments.push({ type: "text", text: text.slice(cursor) });
  }

  return segments.length > 0 ? segments : [{ type: "text", text }];
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
