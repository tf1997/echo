import { useMemo, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/tauri";

type AvatarSize = "xs" | "sm" | "md" | "lg" | "xl";

interface AvatarProps {
  name: string;
  src?: string | null;
  size?: AvatarSize;
  online?: boolean;
  className?: string;
  fallbackClassName?: string;
  title?: string;
}

const SIZE_CLASSES: Record<AvatarSize, string> = {
  xs: "w-7 h-7 text-xs",
  sm: "w-8 h-8 text-xs",
  md: "w-9 h-9 text-sm",
  lg: "w-10 h-10 text-lg",
  xl: "w-12 h-12 text-xl",
};

const STATUS_CLASSES: Record<AvatarSize, string> = {
  xs: "w-2.5 h-2.5 border-2",
  sm: "w-2.5 h-2.5 border-2",
  md: "w-3 h-3 border-2",
  lg: "w-3 h-3 border-2",
  xl: "w-3.5 h-3.5 border-2",
};

export function Avatar({
  name,
  src,
  size = "md",
  online,
  className = "",
  fallbackClassName = "bg-gray-600",
  title,
}: AvatarProps) {
  const [failedSrc, setFailedSrc] = useState<string | null>(null);
  const trimmedSrc = src?.trim() || "";
  const canUseImage = !!trimmedSrc && failedSrc !== trimmedSrc;
  const imageSrc = useMemo(() => {
    if (!canUseImage) return "";
    return convertFileSrc(trimmedSrc);
  }, [canUseImage, trimmedSrc]);
  const fallback = (name.trim().charAt(0) || "?").toUpperCase();

  return (
    <div className={`relative flex-shrink-0 ${className}`} title={title}>
      <div className={`${SIZE_CLASSES[size]} overflow-hidden rounded-full flex items-center justify-center font-medium text-white ${canUseImage ? "bg-gray-800" : fallbackClassName}`}>
        {canUseImage ? (
          <img
            src={imageSrc}
            alt=""
            className="h-full w-full object-cover"
            onError={() => setFailedSrc(trimmedSrc)}
          />
        ) : (
          fallback
        )}
      </div>
      {online !== undefined ? (
        <span
          className={`absolute -bottom-0.5 -right-0.5 rounded-full border-gray-900 ${STATUS_CLASSES[size]} ${online ? "bg-green-400" : "bg-gray-500"}`}
        />
      ) : null}
    </div>
  );
}
