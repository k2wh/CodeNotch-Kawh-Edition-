/**
 * The strip's silhouette.
 *
 * Each end of the strip is a single cubic Bezier that leaves the screen edge,
 * sweeps across the strip's thickness and settles onto its inner edge — the S
 * that makes the notch look milled out of the display rather than laid on top
 * of it. The curve ends with a vertical tangent, so it meets the strip's
 * straight side with no visible kink.
 *
 * The control polygon was measured off the reference art rather than guessed:
 * the silhouette was extracted row by row with sub-pixel edges and a cubic
 * fitted to it, landing at 1.2% RMS of the strip's thickness (about one pixel
 * at the default size). An arc pair — a circular fillet into an elliptical
 * corner — was tried first and is visibly wrong: the reference leaves the
 * screen edge far straighter than any circle tangent to it.
 */

import type { Edge } from "../types";

/**
 * Where the taper ends, as a multiple of the strip's thickness.
 *
 * The silhouette is within a percent of full width by ~1.05x, so the rings
 * clear it well before this; the last stretch is the curve flattening out.
 */
export const TAPER_RATIO = 1.21;

/** First handle: across the thickness, then along the edge. */
const HANDLE_ACROSS = 0.169;
const HANDLE_ALONG = 1.0;

/**
 * Second handle, on the strip's inner edge. Sitting this close to the top of
 * the taper is what holds the curve flat against the screen edge for the first
 * third before it swings across.
 */
const CORNER_ALONG = 0.086;

interface Props {
  /** Window-space size of the strip, in px. */
  width: number;
  height: number;
  edge: Edge;
  className?: string;
}

/** Trim to whole tenths: a tidy `d` attribute, no visible rounding. */
const r = (n: number) => Math.round(n * 10) / 10;

/**
 * Build the silhouette for one edge.
 *
 * The curve is defined once in (`depth` into the screen, `along` the edge)
 * space and mapped onto the requested edge, with `flip` measuring `along` back
 * from the far end for the second taper.
 */
export function notchPath(edge: Edge, width: number, height: number): string {
  const vertical = edge === "left" || edge === "right";
  const thickness = vertical ? width : height;
  const length = vertical ? height : width;

  // Both tapers have to fit end to end; on a one-provider strip they can't, so
  // the curve is squeezed along the edge rather than allowed to overlap.
  const taper = Math.min(thickness * TAPER_RATIO, length / 2);
  const squeeze = taper / (thickness * TAPER_RATIO);

  const point = (depth: number, along: number, flip: boolean): string => {
    const a = flip ? length - along : along;
    switch (edge) {
      case "right":
        return `${r(width - depth)} ${r(a)}`;
      case "left":
        return `${r(depth)} ${r(a)}`;
      case "top":
        return `${r(a)} ${r(depth)}`;
      default:
        return `${r(a)} ${r(height - depth)}`;
    }
  };

  const h1 = (flip: boolean) =>
    point(HANDLE_ACROSS * thickness, HANDLE_ALONG * thickness * squeeze, flip);
  const h2 = (flip: boolean) =>
    point(thickness, CORNER_ALONG * thickness * squeeze, flip);
  const edgeEnd = (flip: boolean) => point(0, 0, flip);
  const innerEnd = (flip: boolean) => point(thickness, taper, flip);

  return [
    `M ${edgeEnd(false)}`,
    // Out of the screen edge and across to the strip's inner side.
    `C ${h1(false)} ${h2(false)} ${innerEnd(false)}`,
    // The straight run holding the rings.
    `L ${innerEnd(true)}`,
    // ...and the mirrored taper back to the edge, handles in reverse order.
    `C ${h2(true)} ${h1(true)} ${edgeEnd(true)}`,
    // Closing runs along the screen edge itself.
    "Z",
  ].join(" ");
}

export function NotchShape({ width, height, edge, className }: Props) {
  if (width <= 0 || height <= 0) return null;

  return (
    <svg
      className={className}
      width={width}
      height={height}
      viewBox={`0 0 ${width} ${height}`}
      // Exact pixel geometry: no scaling, so the curve meets the screen edge
      // cleanly instead of landing on a half pixel.
      preserveAspectRatio="none"
      aria-hidden
    >
      <path d={notchPath(edge, width, height)} fill="var(--notch-black)" />
    </svg>
  );
}
