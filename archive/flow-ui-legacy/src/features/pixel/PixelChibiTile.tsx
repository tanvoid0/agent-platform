import { cn } from "@/lib/utils";

/** 5×5 silhouette: head, torso, legs — reads as a tiny “agent” at strip scale. */
const CHIBI_MASK: number[][] = [
  [0, 0, 1, 0, 0],
  [0, 1, 1, 1, 0],
  [0, 1, 1, 1, 0],
  [0, 1, 0, 1, 0],
  [1, 0, 0, 0, 1],
];

type Props = {
  color: string;
  /** Slightly darker fill for contrast (pixel shadow). */
  shadowColor?: string;
  title?: string;
  pulse?: boolean;
  className?: string;
};

/**
 * Tiny CSS-only “sprite” for the process strip (no image assets).
 * Each logical pixel is 2×2 CSS px inside a 10×10 box.
 */
export function PixelChibiTile({ color, shadowColor, title, pulse, className }: Props) {
  const shadow =
    shadowColor ?? `color-mix(in srgb, ${color} 55%, var(--color-foreground) 45%)`;

  return (
    <span
      title={title}
      className={cn(
        "inline-grid shrink-0 grid-cols-5 grid-rows-5 gap-0 rounded-[1px] border border-black/20 dark:border-white/15",
        pulse && "animate-pulse",
        className,
      )}
      style={{ width: 10, height: 10 }}
      aria-hidden
    >
      {CHIBI_MASK.flatMap((row, y) =>
        row.map((cell, x) => {
          if (!cell) {
            return (
              <span
                key={`${y}-${x}`}
                className="pointer-events-none bg-transparent"
                style={{ width: 2, height: 2 }}
              />
            );
          }
          const isEdge = y === 4 && (x === 0 || x === 4);
          return (
            <span
              key={`${y}-${x}`}
              className="pointer-events-none"
              style={{
                width: 2,
                height: 2,
                backgroundColor: isEdge ? shadow : color,
              }}
            />
          );
        }),
      )}
    </span>
  );
}
