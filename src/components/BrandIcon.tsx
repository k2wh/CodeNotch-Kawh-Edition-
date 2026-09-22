/**
 * Provider marks for the rings.
 *
 * Six of the seven are the vendors' own marks, from Simple Icons' CC0 icon
 * data (see brandPaths.ts). They have to read at ~22px inside a ring, on
 * black, in a single colour, which is exactly what those outlines are drawn
 * for. Each logo remains its owner's trademark; they identify which tool a
 * ring belongs to and nothing more.
 *
 * Codex uses OpenAI's mark as a mask rather than a path: the shape comes from
 * the image's own alpha and the colour from `currentColor`, so it takes the
 * ring's colour like every other mark.
 */

import type { ProviderId } from "../types";
import { BRAND_MONOGRAMS, BRAND_PATHS, BRAND_SHAPES } from "./brandPaths";
import codexMark from "../assets/codex-mark.png";

interface Props {
  provider: ProviderId;
  className?: string;
  style?: React.CSSProperties;
}

export function BrandIcon({ provider, className, style }: Props) {
  const path = BRAND_PATHS[provider];
  const shape = BRAND_SHAPES[provider];

  if (shape) {
    return (
      <svg
        viewBox={shape.viewBox ?? "0 0 24 24"}
        className={className}
        style={style}
        aria-hidden
      >
        {shape.paths.map((d) => (
          <path key={d.slice(0, 24)} d={d} fill="currentColor" fillRule="evenodd" />
        ))}
      </svg>
    );
  }

  // A provider whose mark isn't drawn here gets its initials rather than
  // somebody else's logo. Simple Icons has no CC0 mark for these, and
  // reproducing a trademark from memory would be both wrong and a guess.
  if (!path && provider !== "codex") {
    return (
      <span
        className={className}
        style={{
          ...style,
          display: "grid",
          placeItems: "center",
          font: `600 ${Math.round((Number(style?.width) || 22) * 0.46)}px/1 var(--font)`,
          letterSpacing: "-0.03em",
          color: "currentColor",
        }}
        aria-hidden
      >
        {BRAND_MONOGRAMS[provider] ?? provider.slice(0, 2).toUpperCase()}
      </span>
    );
  }

  if (!path) {
    // The mark is painted by `currentColor` showing through the image's alpha.
    return (
      <span
        className={className}
        style={{
          ...style,
          display: "block",
          background: "currentColor",
          maskImage: `url(${codexMark})`,
          maskSize: "contain",
          maskRepeat: "no-repeat",
          maskPosition: "center",
          WebkitMaskImage: `url(${codexMark})`,
          WebkitMaskSize: "contain",
          WebkitMaskRepeat: "no-repeat",
          WebkitMaskPosition: "center",
        }}
        aria-hidden
      />
    );
  }

  return (
    <svg viewBox="0 0 24 24" className={className} style={style} aria-hidden>
      <path d={path} fill="currentColor" />
    </svg>
  );
}
