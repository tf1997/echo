import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { emit } from "@tauri-apps/api/event";
import { WebviewWindow, appWindow, PhysicalPosition, PhysicalSize } from "@tauri-apps/api/window";
import { captureScreenshotNative } from "../api";

const ANNOTATION_COLOR = "#ef4444";
const ANNOTATION_LINE_WIDTH = 4;
const TEXT_FONT_SIZE = 24;

interface CropState {
  imageUrl: string;
  naturalWidth: number;
  naturalHeight: number;
  selection: {
    x: number;
    y: number;
    width: number;
    height: number;
  } | null;
  dragging: boolean;
  startX: number;
  startY: number;
}

type ToolMode = "select" | "pen" | "rect" | "text";

interface Point {
  x: number;
  y: number;
}

interface PenAnnotation {
  type: "pen";
  points: Point[];
}

interface RectAnnotation {
  type: "rect";
  x: number;
  y: number;
  width: number;
  height: number;
}

interface TextAnnotation {
  type: "text";
  x: number;
  y: number;
  text: string;
}

type Annotation = PenAnnotation | RectAnnotation | TextAnnotation;

interface TextDraft {
  x: number;
  y: number;
  value: string;
}

interface ToolIconButtonProps {
  active?: boolean;
  disabled?: boolean;
  title: string;
  tone?: "default" | "success" | "danger";
  onClick: () => void | Promise<void>;
  children: ReactNode;
}

function ToolIconButton({
  active = false,
  disabled = false,
  title,
  tone = "default",
  onClick,
  children,
}: ToolIconButtonProps) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      onClick={() => void onClick()}
      disabled={disabled}
      className={`screenshot-tool-button grid h-9 w-9 place-items-center rounded-md border transition disabled:cursor-not-allowed disabled:opacity-40 ${active ? "screenshot-tool-button-active" : ""} ${tone === "success" ? "screenshot-tool-button-success" : ""} ${tone === "danger" ? "screenshot-tool-button-danger" : ""}`}
    >
      {children}
    </button>
  );
}

function PenIcon() {
  return (
    <svg viewBox="0 0 24 24" className="h-5 w-5" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M14.5 4.5 19.5 9.5" />
      <path d="M5 19 7 13.5 16.8 3.7a2.1 2.1 0 0 1 3 3L10.5 16 5 19Z" />
    </svg>
  );
}

function RectIcon() {
  return (
    <svg viewBox="0 0 24 24" className="h-5 w-5" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <rect x="5" y="6" width="14" height="12" rx="1.5" />
    </svg>
  );
}

function TextIcon() {
  return (
    <svg viewBox="0 0 24 24" className="h-5 w-5" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M5 6h14" />
      <path d="M12 6v13" />
      <path d="M9 19h6" />
    </svg>
  );
}

function UndoIcon() {
  return (
    <svg viewBox="0 0 24 24" className="h-5 w-5" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M8 7H4v4" />
      <path d="M4 11a7 7 0 1 0 2-5" />
    </svg>
  );
}

function CancelIcon() {
  return (
    <svg viewBox="0 0 24 24" className="h-5 w-5" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M6 6l12 12" />
      <path d="M18 6 6 18" />
    </svg>
  );
}

function DoneIcon() {
  return (
    <svg viewBox="0 0 24 24" className="h-5 w-5" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="m5 12 5 5L20 7" />
    </svg>
  );
}

function base64ToBlob(base64: string, mime: string): Blob {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return new Blob([bytes], { type: mime });
}

function makeScreenshotFileName(): string {
  const stamp = new Date()
    .toISOString()
    .replace(/\.\d{3}Z$/, "")
    .replace(/[-:]/g, "")
    .replace("T", "-");
  return `screenshot-${stamp}.png`;
}

async function copyBlobToClipboard(blob: Blob): Promise<boolean> {
  if (!navigator.clipboard || typeof ClipboardItem === "undefined") return false;
  try {
    await navigator.clipboard.write([
      new ClipboardItem({
        [blob.type || "image/png"]: blob,
      }),
    ]);
    return true;
  } catch {
    return false;
  }
}

async function blobToBase64(blob: Blob): Promise<string> {
  const buffer = await blob.arrayBuffer();
  let binary = "";
  const bytes = new Uint8Array(buffer);
  const chunkSize = 0x8000;
  for (let i = 0; i < bytes.length; i += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunkSize));
  }
  return btoa(binary);
}

export function ScreenshotOverlay() {
  const [crop, setCrop] = useState<CropState | null>(null);
  const [error, setError] = useState("");
  const [toolMode, setToolMode] = useState<ToolMode>("select");
  const [annotations, setAnnotations] = useState<Annotation[]>([]);
  const [draftAnnotation, setDraftAnnotation] = useState<Annotation | null>(null);
  const [drawing, setDrawing] = useState(false);
  const [drawStart, setDrawStart] = useState<Point | null>(null);
  const [textDraft, setTextDraft] = useState<TextDraft | null>(null);
  const surfaceRef = useRef<HTMLDivElement>(null);
  const textInputRef = useRef<HTMLInputElement>(null);
  const startedRef = useRef(false);
  const selectionDragMovedRef = useRef(false);

  useEffect(() => {
    if (startedRef.current) return;
    startedRef.current = true;
    let mounted = true;
    let objectUrl: string | null = null;

    const load = async () => {
      try {
        const screenshot = await captureScreenshotNative();
        const blob = base64ToBlob(screenshot.base64, screenshot.mime);
        objectUrl = URL.createObjectURL(blob);

        await appWindow.setDecorations(false);
        await appWindow.setAlwaysOnTop(true);
        await appWindow.setResizable(false);
        await appWindow.setSize(new PhysicalSize(screenshot.width, screenshot.height));
        await appWindow.setPosition(new PhysicalPosition(screenshot.x, screenshot.y));
        const scaleFactor = await appWindow.scaleFactor().catch(() => window.devicePixelRatio || 1);
        const screenWidth = Math.round(window.screen.width * scaleFactor);
        const screenHeight = Math.round(window.screen.height * scaleFactor);
        const matchesPrimaryScreen =
          screenshot.x === 0 &&
          screenshot.y === 0 &&
          Math.abs(screenshot.width - screenWidth) <= 3 &&
          Math.abs(screenshot.height - screenHeight) <= 3;
        if (matchesPrimaryScreen) {
          await appWindow.setFullscreen(true).catch(() => {});
        }
        await appWindow.show();
        await appWindow.setFocus();

        if (!mounted) return;
        setCrop({
          imageUrl: objectUrl,
          naturalWidth: screenshot.width || 1,
          naturalHeight: screenshot.height || 1,
          selection: {
            x: 0,
            y: 0,
            width: screenshot.width || 1,
            height: screenshot.height || 1,
          },
          dragging: false,
          startX: 0,
          startY: 0,
        });
      } catch (err) {
        await emit("screenshot-overlay-closed").catch(() => {});
        await appWindow.show().catch(() => {});
        await appWindow.setFocus().catch(() => {});
        setError(String(err));
      }
    };

    void load();
    return () => {
      mounted = false;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, []);

  const selectionStyle = useMemo(() => crop?.selection ? {
    left: `${(crop.selection.x / crop.naturalWidth) * 100}%`,
    top: `${(crop.selection.y / crop.naturalHeight) * 100}%`,
    width: `${(crop.selection.width / crop.naturalWidth) * 100}%`,
    height: `${(crop.selection.height / crop.naturalHeight) * 100}%`,
  } : undefined, [crop]);

  const textDraftStyle = useMemo(() => crop && textDraft ? {
    left: `${(textDraft.x / crop.naturalWidth) * 100}%`,
    top: `${(textDraft.y / crop.naturalHeight) * 100}%`,
  } : undefined, [crop, textDraft]);

  const hasSelection = !!crop?.selection && crop.selection.width >= 2 && crop.selection.height >= 2;
  const canAnnotate = hasSelection && toolMode !== "select";
  const canUndo = annotations.length > 0 || !!draftAnnotation || !!textDraft;

  const toolbarStyle = useMemo(() => {
    if (!crop?.selection) return undefined;
    const left = ((crop.selection.x + crop.selection.width) / crop.naturalWidth) * window.innerWidth;
    const top = ((crop.selection.y + crop.selection.height) / crop.naturalHeight) * window.innerHeight;
    return {
      left: Math.min(Math.max(12, left - 250), Math.max(12, window.innerWidth - 292)),
      top: Math.min(Math.max(12, top + 10), Math.max(12, window.innerHeight - 56)),
    };
  }, [crop]);

  const clampToSelection = useCallback((point: Point): Point | null => {
    const selection = crop?.selection;
    if (!selection || !hasSelection) return null;
    return {
      x: Math.max(selection.x, Math.min(selection.x + selection.width, point.x)),
      y: Math.max(selection.y, Math.min(selection.y + selection.height, point.y)),
    };
  }, [crop?.selection, hasSelection]);

  const closeOverlay = useCallback(async () => {
    await emit("screenshot-overlay-closed");
    await WebviewWindow.getByLabel("screenshot-overlay")?.close();
  }, []);

  const commitTextDraft = useCallback(() => {
    setTextDraft((prev) => {
      const text = prev?.value.trim();
      if (prev && text) {
        setAnnotations((annotationsPrev) => [...annotationsPrev, {
          type: "text",
          x: prev.x,
          y: prev.y,
          text,
        }]);
      }
      return null;
    });
  }, []);

  const undoLastAnnotation = useCallback(() => {
    setDraftAnnotation(null);
    setTextDraft(null);
    setAnnotations((prev) => prev.slice(0, -1));
  }, []);

  useEffect(() => {
    if (!textDraft) return;
    requestAnimationFrame(() => textInputRef.current?.focus());
  }, [textDraft]);

  const getPoint = useCallback((event: React.PointerEvent<HTMLDivElement>) => {
    if (!crop || !surfaceRef.current) return null;
    const rect = surfaceRef.current.getBoundingClientRect();
    const x = Math.max(0, Math.min(crop.naturalWidth, ((event.clientX - rect.left) / rect.width) * crop.naturalWidth));
    const y = Math.max(0, Math.min(crop.naturalHeight, ((event.clientY - rect.top) / rect.height) * crop.naturalHeight));
    return { x, y };
  }, [crop]);

  const beginSelection = useCallback((event: React.PointerEvent<HTMLDivElement>) => {
    const point = getPoint(event);
    if (!point) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    selectionDragMovedRef.current = false;

    if (canAnnotate) {
      const clamped = clampToSelection(point);
      if (!clamped) return;
      commitTextDraft();
      if (toolMode === "text") {
        setTextDraft({ x: clamped.x, y: clamped.y, value: "" });
        return;
      }

      setDrawing(true);
      setDrawStart(clamped);
      setDraftAnnotation(toolMode === "pen"
        ? { type: "pen", points: [clamped] }
        : { type: "rect", x: clamped.x, y: clamped.y, width: 0, height: 0 }
      );
      return;
    }

    commitTextDraft();
    setCrop((prev) => prev ? {
      ...prev,
      dragging: true,
      startX: point.x,
      startY: point.y,
      selection: prev.selection ?? { x: point.x, y: point.y, width: 0, height: 0 },
    } : prev);
    setDraftAnnotation(null);
    setToolMode("select");
  }, [canAnnotate, clampToSelection, commitTextDraft, getPoint, toolMode]);

  const updateSelection = useCallback((event: React.PointerEvent<HTMLDivElement>) => {
    const point = getPoint(event);
    if (!point) return;

    if (drawing) {
      const clamped = clampToSelection(point);
      if (!clamped) return;
      setDraftAnnotation((prev) => {
        if (!prev || !drawStart) return prev;
        if (prev.type === "pen") {
          return { ...prev, points: [...prev.points, clamped] };
        }
        return {
          type: "rect",
          x: Math.min(drawStart.x, clamped.x),
          y: Math.min(drawStart.y, clamped.y),
          width: Math.abs(clamped.x - drawStart.x),
          height: Math.abs(clamped.y - drawStart.y),
        };
      });
      return;
    }

    setCrop((prev) => {
      if (!prev?.dragging) return prev;
      const width = Math.abs(point.x - prev.startX);
      const height = Math.abs(point.y - prev.startY);
      if (width < 2 || height < 2) return prev;
      if (!selectionDragMovedRef.current) {
        selectionDragMovedRef.current = true;
        setAnnotations([]);
        setTextDraft(null);
      }
      return {
        ...prev,
        selection: {
          x: Math.min(prev.startX, point.x),
          y: Math.min(prev.startY, point.y),
          width,
          height,
        },
      };
    });
  }, [clampToSelection, drawStart, drawing, getPoint]);

  const endSelection = useCallback((event: React.PointerEvent<HTMLDivElement>) => {
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }

    if (drawing) {
      if (draftAnnotation) {
        if (draftAnnotation.type === "pen" && draftAnnotation.points.length > 1) {
          setAnnotations((prev) => [...prev, draftAnnotation]);
        } else if (draftAnnotation.type === "rect" && draftAnnotation.width >= 2 && draftAnnotation.height >= 2) {
          setAnnotations((prev) => [...prev, draftAnnotation]);
        }
      }
      setDrawing(false);
      setDrawStart(null);
      setDraftAnnotation(null);
      return;
    }

    const selectionMoved = selectionDragMovedRef.current;
    selectionDragMovedRef.current = false;
    setCrop((prev) => prev ? { ...prev, dragging: false } : prev);
    if (selectionMoved) {
      setToolMode((prev) => prev === "select" ? "pen" : prev);
    }
  }, [draftAnnotation, drawing]);

  const confirmSelection = useCallback(async () => {
    if (!crop) return;
    const selection = crop.selection && hasSelection
      ? crop.selection
      : { x: 0, y: 0, width: crop.naturalWidth, height: crop.naturalHeight };
    const cropWidth = Math.max(1, Math.round(selection.width));
    const cropHeight = Math.max(1, Math.round(selection.height));
    const cropX = Math.round(selection.x);
    const cropY = Math.round(selection.y);
    const text = textDraft?.value.trim();
    const finalAnnotations: Annotation[] = textDraft && text
      ? [...annotations, { type: "text", x: textDraft.x, y: textDraft.y, text }]
      : annotations;

    try {
      const image = new Image();
      image.src = crop.imageUrl;
      await image.decode();

      const canvas = document.createElement("canvas");
      canvas.width = cropWidth;
      canvas.height = cropHeight;
      const context = canvas.getContext("2d");
      if (!context) throw new Error("无法创建截图画布");

      context.drawImage(image, cropX, cropY, cropWidth, cropHeight, 0, 0, cropWidth, cropHeight);
      context.save();
      context.beginPath();
      context.rect(0, 0, cropWidth, cropHeight);
      context.clip();
      context.translate(-cropX, -cropY);
      context.strokeStyle = ANNOTATION_COLOR;
      context.lineWidth = ANNOTATION_LINE_WIDTH;
      context.lineCap = "round";
      context.lineJoin = "round";

      for (const annotation of finalAnnotations) {
        if (annotation.type === "pen") {
          if (annotation.points.length < 2) continue;
          context.beginPath();
          context.moveTo(annotation.points[0].x, annotation.points[0].y);
          for (const point of annotation.points.slice(1)) {
            context.lineTo(point.x, point.y);
          }
          context.stroke();
        } else if (annotation.type === "rect") {
          context.strokeRect(annotation.x, annotation.y, annotation.width, annotation.height);
        } else {
          context.fillStyle = ANNOTATION_COLOR;
          context.font = `${TEXT_FONT_SIZE}px sans-serif`;
          context.textBaseline = "top";
          context.fillText(annotation.text, annotation.x, annotation.y);
        }
      }
      context.restore();

      const blob = await new Promise<Blob>((resolve, reject) => {
        canvas.toBlob((result) => {
          if (result) resolve(result);
          else reject(new Error("截图导出失败"));
        }, "image/png");
      });
      const copiedToClipboard = await copyBlobToClipboard(blob);
      const base64 = await blobToBase64(blob);
      await emit("screenshot-captured", {
        base64,
        mime: "image/png",
        filename: makeScreenshotFileName(),
        copiedToClipboard,
      });
      await closeOverlay();
    } catch (err) {
      setError(String(err));
    }
  }, [annotations, closeOverlay, crop, hasSelection, textDraft]);

  const renderAnnotation = (annotation: Annotation, key: string) => {
    if (annotation.type === "pen") {
      const points = annotation.points.map((point) => `${point.x},${point.y}`).join(" ");
      return (
        <polyline
          key={key}
          points={points}
          fill="none"
          stroke={ANNOTATION_COLOR}
          strokeWidth={ANNOTATION_LINE_WIDTH}
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      );
    }

    if (annotation.type === "rect") {
      return (
        <rect
          key={key}
          x={annotation.x}
          y={annotation.y}
          width={annotation.width}
          height={annotation.height}
          fill="none"
          stroke={ANNOTATION_COLOR}
          strokeWidth={ANNOTATION_LINE_WIDTH}
        />
      );
    }

    return (
      <text
        key={key}
        x={annotation.x}
        y={annotation.y}
        fill={ANNOTATION_COLOR}
        fontSize={TEXT_FONT_SIZE}
        fontFamily="sans-serif"
        dominantBaseline="text-before-edge"
      >
        {annotation.text}
      </text>
    );
  };

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      const isTextEntry = target?.tagName === "INPUT" || target?.tagName === "TEXTAREA" || !!target?.isContentEditable;

      if (!isTextEntry && (event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "z") {
        event.preventDefault();
        undoLastAnnotation();
        return;
      }
      if (event.key === "Escape") {
        if (isTextEntry && textDraft) {
          event.preventDefault();
          setTextDraft(null);
          return;
        }
        void closeOverlay();
      }
      if (event.key === "Enter") {
        event.preventDefault();
        if (isTextEntry) {
          commitTextDraft();
          return;
        }
        void confirmSelection();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [closeOverlay, commitTextDraft, confirmSelection, textDraft, undoLastAnnotation]);

  return (
    <div className="fixed inset-0 bg-black select-none overflow-hidden">
      {crop ? (
        <div
          ref={surfaceRef}
          className={`relative h-screen w-screen ${toolMode === "text" ? "cursor-text" : "cursor-crosshair"}`}
          onPointerDown={beginSelection}
          onPointerMove={updateSelection}
          onPointerUp={endSelection}
          onPointerCancel={endSelection}
          onDoubleClick={(event) => {
            event.preventDefault();
            void confirmSelection();
          }}
        >
          <img src={crop.imageUrl} alt="" className="absolute inset-0 h-full w-full pointer-events-none" draggable={false} />
          {selectionStyle && (
            <div
              className="absolute border-2 border-indigo-300 bg-indigo-500/20 shadow-[0_0_0_9999px_rgba(0,0,0,0.45)] pointer-events-none"
              style={selectionStyle}
            />
          )}
          <svg className="absolute inset-0 h-full w-full pointer-events-none" viewBox={`0 0 ${crop.naturalWidth} ${crop.naturalHeight}`} preserveAspectRatio="none">
            <defs>
              {crop.selection && (
                <clipPath id="screenshot-selection-clip">
                  <rect x={crop.selection.x} y={crop.selection.y} width={crop.selection.width} height={crop.selection.height} />
                </clipPath>
              )}
            </defs>
            <g clipPath={crop.selection ? "url(#screenshot-selection-clip)" : undefined}>
              {annotations.map((annotation, index) => renderAnnotation(annotation, `annotation-${index}`))}
              {draftAnnotation ? renderAnnotation(draftAnnotation, "draft") : null}
            </g>
          </svg>
          {textDraftStyle && (
            <input
              ref={textInputRef}
              value={textDraft?.value ?? ""}
              onChange={(event) => setTextDraft((prev) => prev ? { ...prev, value: event.target.value } : prev)}
              onBlur={commitTextDraft}
              onPointerDown={(event) => event.stopPropagation()}
              onPointerMove={(event) => event.stopPropagation()}
              onPointerUp={(event) => event.stopPropagation()}
              className="absolute z-20 min-w-32 max-w-80 rounded border border-red-300/70 bg-white/95 px-2 py-1 text-2xl leading-none text-red-500 outline-none shadow-xl"
              style={textDraftStyle}
            />
          )}
          {selectionStyle && toolbarStyle && (
            <div
              className="screenshot-toolbar absolute flex items-center gap-1.5 rounded-lg border p-1.5 shadow-2xl"
              style={toolbarStyle}
              onPointerDown={(event) => event.stopPropagation()}
            >
              <ToolIconButton title="画笔" active={toolMode === "pen"} onClick={() => {
                commitTextDraft();
                setToolMode("pen");
              }}>
                <PenIcon />
              </ToolIconButton>
              <ToolIconButton title="矩形框" active={toolMode === "rect"} onClick={() => {
                commitTextDraft();
                setToolMode("rect");
              }}>
                <RectIcon />
              </ToolIconButton>
              <ToolIconButton title="文字" active={toolMode === "text"} onClick={() => {
                commitTextDraft();
                setToolMode("text");
              }}>
                <TextIcon />
              </ToolIconButton>
              <ToolIconButton title="撤销" disabled={!canUndo} onClick={undoLastAnnotation}>
                <UndoIcon />
              </ToolIconButton>
              <ToolIconButton title="取消" tone="danger" onClick={closeOverlay}>
                <CancelIcon />
              </ToolIconButton>
              <ToolIconButton title="完成" tone="success" disabled={!hasSelection} onClick={confirmSelection}>
                <DoneIcon />
              </ToolIconButton>
            </div>
          )}
        </div>
      ) : (
        <div className="flex h-screen w-screen items-center justify-center bg-black px-6 text-sm text-gray-300">
          {error ? (
            <div className="max-w-md rounded-lg border border-red-400/30 bg-gray-900/95 p-4 text-center shadow-2xl">
              <p className="mb-2 text-sm font-medium text-red-200">截图失败</p>
              <p className="max-h-40 overflow-y-auto whitespace-pre-wrap break-words text-left text-xs leading-relaxed text-gray-300">
                {error}
              </p>
              <button
                type="button"
                onClick={() => void closeOverlay()}
                className="screenshot-tool-button mt-4 h-8 rounded-md border px-3 text-xs"
              >
                关闭
              </button>
            </div>
          ) : (
            <div className="rounded-lg border border-white/10 bg-gray-900/90 px-4 py-3 shadow-2xl">
              正在准备截图...
            </div>
          )}
        </div>
      )}
    </div>
  );
}
