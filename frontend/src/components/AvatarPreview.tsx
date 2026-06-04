import { useCallback, useEffect, useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { convertFileSrc } from "@tauri-apps/api/tauri";
import { Avatar } from "./Avatar";

type AvatarSize = "xs" | "sm" | "md" | "lg" | "xl";

interface AvatarPreviewTriggerProps {
  name: string;
  src?: string | null;
  size?: AvatarSize;
  online?: boolean;
  fallbackClassName?: string;
  className?: string;
  title?: string;
}

interface AvatarPreviewDialogProps {
  name: string;
  src: string;
  onClose: () => void;
}

function AvatarPreviewDialog({ name, src, onClose }: AvatarPreviewDialogProps) {
  const [failed, setFailed] = useState(false);
  const previewSrc = useMemo(() => convertFileSrc(src), [src]);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  if (typeof document === "undefined") return null;

  return createPortal(
    <div
      className="avatar-preview-backdrop"
      role="dialog"
      aria-modal="true"
      aria-label={`${name}的头像预览`}
      onClick={onClose}
    >
      <div className="avatar-preview-dialog" onClick={(event) => event.stopPropagation()}>
        <button
          type="button"
          className="avatar-preview-close"
          onClick={onClose}
          aria-label="关闭"
        >
          ×
        </button>
        {failed ? (
          <div className="avatar-preview-fallback">
            {(name.trim().charAt(0) || "?").toUpperCase()}
          </div>
        ) : (
          <img
            src={previewSrc}
            alt={`${name}的头像`}
            className="avatar-preview-image"
            onError={() => setFailed(true)}
          />
        )}
        <p className="avatar-preview-name" title={name}>{name}</p>
      </div>
    </div>,
    document.body,
  );
}

export function AvatarPreviewTrigger({
  name,
  src,
  size = "md",
  online,
  fallbackClassName,
  className = "",
  title,
}: AvatarPreviewTriggerProps) {
  const [previewOpen, setPreviewOpen] = useState(false);
  const trimmedSrc = src?.trim() || "";
  const canPreview = !!trimmedSrc;

  const openPreview = useCallback(() => {
    if (canPreview) setPreviewOpen(true);
  }, [canPreview]);

  const closePreview = useCallback(() => {
    setPreviewOpen(false);
  }, []);

  return (
    <>
      <button
        type="button"
        className={`avatar-preview-trigger relative flex-shrink-0 rounded-full border-0 bg-transparent p-0 leading-none focus:outline-none focus:ring-2 focus:ring-indigo-400 ${canPreview ? "cursor-zoom-in" : "cursor-default"} ${className}`}
        title={title ?? (canPreview ? "预览头像" : undefined)}
        aria-label={canPreview ? `预览${name}的头像` : `${name}的头像`}
        onClick={openPreview}
        disabled={!canPreview}
      >
        <Avatar
          name={name}
          src={src}
          size={size}
          online={online}
          fallbackClassName={fallbackClassName}
        />
      </button>
      {previewOpen && canPreview ? (
        <AvatarPreviewDialog name={name} src={trimmedSrc} onClose={closePreview} />
      ) : null}
    </>
  );
}
