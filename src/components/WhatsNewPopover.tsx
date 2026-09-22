/**
 * What changed in this version, shown once when it first opens — and again
 * from the version in settings.
 *
 * No tail: it belongs to the whole notch rather than to one ring, so it sits
 * beside the strip like the settings panel does.
 */

import { Sparkles } from "lucide-react";

import type { Release } from "../lib/changelog";
import { useI18n } from "../lib/i18n";

export function WhatsNewPopover({
  release,
  onClose,
}: {
  release: Release;
  onClose: () => void;
}) {
  const { t, lang, locale } = useI18n();
  const date = new Date(release.date);
  const when = Number.isNaN(date.getTime())
    ? null
    : date.toLocaleDateString(locale, { day: "numeric", month: "long" });

  return (
    <div className="popover" role="dialog" aria-label={t("whatsNew.title", { version: release.version })}>
      <header className="popover-head">
        <Sparkles className="popover-mark update-card-mark" aria-hidden />
        <span className="popover-title">{t("whatsNew.title", { version: release.version })}</span>
      </header>
      {when && <p className="popover-account">{when}</p>}

      <ul className="whats-new-list">
        {release.changes[lang].map((change) => (
          <li key={change}>{change}</li>
        ))}
      </ul>

      <div className="update-actions">
        <button type="button" className="update-primary" onClick={onClose}>
          {t("whatsNew.done")}
        </button>
      </div>
    </div>
  );
}
