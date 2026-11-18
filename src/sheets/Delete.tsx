import { useEffect, useRef } from "react";
import { useT } from "../lib/i18n";
import { useStore } from "../lib/store";
import { Sheet } from "../ui/controls";

/** Asks before photos are deleted: several at once, or any that cannot be taken back out of a trash. */
export function DeleteSheet() {
  const t = useT();
  const asked = useStore((s) => s.doomed);
  const mac = useStore((s) => s.info?.platform === "macos");
  const only = useStore((s) => (s.doomed?.ids.length === 1 ? s.items.find((it) => it.id === s.doomed!.ids[0])?.name : undefined));
  const act = useStore.getState();
  const yes = useRef<HTMLButtonElement>(null);
  const no = useRef<HTMLButtonElement>(null);
  // Enter is "yes" only to what can be taken back. The question may change while the sheet is up (the trash
  // refused), and a second Enter must not answer the new one.
  const final = !!asked?.forGood;
  const busyNow = !!asked?.busy;
  useEffect(() => {
    if (!busyNow) (final ? no : yes).current?.focus();
  }, [final, busyNow]);
  if (!asked) return null;
  const { forGood, why, busy } = asked;
  const count = asked.ids.length;
  const bin = t.trashName(mac);
  const what = t.deleteWhat(count, only);
  return (
    <Sheet
      title={t.deleteTitle}
      onClose={busy ? undefined : act.cancelDelete}
      footer={
        <>
          {busy && <span className="spinner small" />}
          <span className="spacer" />
          <button ref={no} disabled={busy} onClick={act.cancelDelete}>
            {t.cancel}
          </button>
          <button ref={yes} className="primary danger" disabled={busy} onClick={act.confirmDelete}>
            {busy ? t.deleting : forGood ? t.deleteForGood : t.deleteToTrash(bin)}
          </button>
        </>
      }
    >
      <div className="delete-ask">
        {why !== undefined ? (
          <>
            <p className="note lead">{t.deleteNoTrash(what, bin)}</p>
            {why && <p className="note">{why}</p>}
            <p className="note bad">{t.deleteForGoodInstead(bin)}</p>
          </>
        ) : forGood ? (
          <>
            <p className="note lead">{t.deleteForGoodAsk(what)}</p>
            <p className="note bad">{t.deleteForGoodHint}</p>
          </>
        ) : (
          <>
            <p className="note lead">{t.deleteAsk(what, bin)}</p>
            <p className="note">{t.deleteHint(bin)}</p>
          </>
        )}
      </div>
    </Sheet>
  );
}
