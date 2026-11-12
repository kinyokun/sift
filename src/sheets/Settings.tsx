import { openUrl } from "@tauri-apps/plugin-opener";
import { useEffect, useState } from "react";
import { api } from "../lib/api";
import { useT } from "../lib/i18n";
import { ACTIONS, DEFAULT_KEYS, comboOf, keyCaps, type Action } from "../lib/shortcuts";
import { useStore } from "../lib/store";
import { Row, Segmented, Sheet, Toggle } from "../ui/controls";

const REPO = "https://github.com/kinyokun/sift";
/** What Nikon asks of everyone using the engine that official builds include. */
const NIKON_SDK_TERMS = `${REPO}/blob/main/NIKON_SDK_TERMS.md`;
const ORIGIN_REPO = "https://github.com/R4ksh4/AnicropStudio";

function openLink(url: string) {
  openUrl(url).catch((e) => useStore.getState().notify(String(e)));
}

function KeyCap({ children }: { children: React.ReactNode }) {
  return <kbd>{children}</kbd>;
}

/** A binding such as `Shift+G` drawn as one key cap per key. */
function Binding({ value }: { value: string }) {
  const t = useT();
  if (!value) return <span className="dim">{t.unassigned}</span>;
  return (
    <>
      {keyCaps(value).map((cap, i) => (
        <KeyCap key={i}>{cap}</KeyCap>
      ))}
    </>
  );
}

export function SettingsSheet({ onClose }: { onClose: () => void }) {
  const t = useT();
  const settings = useStore((s) => s.settings);
  const info = useStore((s) => s.info);
  const setSettings = useStore((s) => s.setSettings);
  const [capturing, setCapturing] = useState<Action | null>(null);
  const [cacheBytes, setCacheBytes] = useState(info?.cache_bytes ?? 0);
  const mobile = !!info?.mobile;
  const library = useStore((s) => s.library);
  useEffect(() => {
    api.appInfo().then((i) => setCacheBytes(i.cache_bytes));
  }, []);

  // The next key press (with any modifiers) becomes the shortcut. If another
  // action already uses it, the two trade places so nothing is left unbound.
  useEffect(() => {
    if (!capturing) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") return setCapturing(null);
      const combo = comboOf(e);
      if (!combo) return;
      const keys = { ...settings.keys };
      const clash = ACTIONS.find((a) => a !== capturing && keys[a] === combo);
      if (clash) {
        keys[clash] = keys[capturing];
        useStore.getState().notify(`shortcutClash:${clash}`);
      }
      keys[capturing] = combo;
      setSettings({ keys });
      setCapturing(null);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [capturing, settings.keys]);

  const wheelZoom = settings.wheelZoom ?? info?.platform !== "macos";

  return (
    <Sheet title={t.settings} onClose={capturing ? () => setCapturing(null) : onClose}>
      <div className="form">
        <h4>{t.general}</h4>
        <div className="group">
          <Row label={t.language}>
            <Segmented
              value={settings.lang}
              options={[
                { value: "auto", label: t.langAuto },
                { value: "zh", label: "中文" },
                { value: "en", label: "English" },
              ]}
              onChange={(lang) => setSettings({ lang })}
            />
          </Row>
          <Row label={t.pairRaw}>
            <Toggle
              checked={settings.pair}
              onChange={(pair) => {
                setSettings({ pair });
                useStore.getState().reload();
              }}
            />
          </Row>
          <Row label={t.preferRaw} hint={t.preferRawHint}>
            <Toggle
              checked={settings.preferRaw}
              onChange={(preferRaw) => {
                setSettings({ preferRaw });
                useStore.getState().reload();
              }}
            />
          </Row>
          <Row label={t.autoAdvance}>
            <Toggle checked={settings.autoAdvance} onChange={(autoAdvance) => setSettings({ autoAdvance })} />
          </Row>
          {!mobile && (
            <Row label={t.infoPinned}>
              <Toggle checked={settings.infoPinned} onChange={(infoPinned) => setSettings({ infoPinned })} />
            </Row>
          )}
          <Row label={t.autoDevelop} hint={t.autoDevelopHint}>
            <Toggle checked={settings.autoDevelop} onChange={(autoDevelop) => setSettings({ autoDevelop })} />
          </Row>
          {!mobile && (
            <Row label={t.wheelZoom}>
              <Toggle checked={wheelZoom} onChange={(v) => setSettings({ wheelZoom: v })} />
            </Row>
          )}
        </div>

        {!mobile && (
          <>
            <h4>
              {t.shortcuts}
              <button className="link" onClick={() => setSettings({ keys: { ...DEFAULT_KEYS } })}>
                {t.resetShortcuts}
              </button>
            </h4>
            <div className="group keys">
              {ACTIONS.map((a) => (
                <Row key={a} label={t.actions[a]} onClick={() => setCapturing(a)}>
                  {capturing === a ? <span className="capturing">{t.pressKey}</span> : <Binding value={settings.keys[a]} />}
                </Row>
              ))}
            </div>
          </>
        )}

        <h4>{t.lookManage}</h4>
        <div className="group">
          {library.map((l) => (
            <Row key={l.id} label={l.name} hint={l.id}>
              <button className="link" onClick={async () => useStore.getState().setLibrary(await api.removeLook(l.id))}>
                {t.remove}
              </button>
            </Row>
          ))}
          {!library.length && <Row label={<span className="dim">{t.lookEmpty}</span>} />}
        </div>
        {library.some((l) => l.nikon) && <p className="note">{t.lookNp3Note}</p>}

        {info?.nikon_supported && (
          <>
            <h4>{t.sdkTitle}</h4>
            <div className="group">
              <Row label={t.sdkName} hint={info.nikon_sdk ? t.sdkOn : t.sdkOff} />
              {info.nikon_sdk && (
                <Row label={t.sdkTerms}>
                  <button className="link" onClick={() => openLink(NIKON_SDK_TERMS)}>
                    {t.view}
                  </button>
                </Row>
              )}
            </div>
            <p className="note">{info.nikon_sdk ? t.sdkNote : t.sdkMissingNote}</p>
          </>
        )}

        <h4>{t.cache}</h4>
        <div className="group">
          <Row label={`${(cacheBytes / 1048576).toFixed(0)} MB`}>
            <button
              className="link"
              onClick={async () => {
                await api.clearCache();
                setCacheBytes(0);
              }}
            >
              {t.clearCache}
            </button>
          </Row>
        </div>

        <h4>{t.about}</h4>
        <div className="group">
          <Row label="Sift">
            <span className="dim">
              {t.version} {info?.version}
            </span>
          </Row>
          <Row label={t.sourceCode} hint="GPL-3.0">
            <button className="link" onClick={() => openLink(REPO)}>
              GitHub
            </button>
          </Row>
          <Row label={t.licenses}>
            <button className="link" onClick={() => openLink(`${REPO}/blob/main/THIRD_PARTY_NOTICES.md`)}>
              {t.view}
            </button>
          </Row>
        </div>
        <p className="note">
          {t.originNote}{" "}
          <button className="link" onClick={() => openLink(ORIGIN_REPO)}>
            Anicrop Studio
          </button>
        </p>
      </div>
    </Sheet>
  );
}

export function HelpSheet({ onClose }: { onClose: () => void }) {
  const t = useT();
  const keys = useStore((s) => s.settings.keys);
  const onboarded = useStore((s) => s.settings.onboarded);
  const mobile = useStore((s) => !!s.info?.mobile);
  const finish = () => {
    useStore.getState().setSettings({ onboarded: true });
    onClose();
  };
  const cards: [string, string, React.ReactNode][] = [
    [mobile ? t.help1TitleTouch : t.help1Title, mobile ? t.help1BodyTouch : t.help1Body, null],
    [t.help2Title, mobile ? t.help2BodyTouch : t.help2Body, <><Binding value={keys.af} /><Binding value={keys.eye} /><Binding value={keys.zoom} /></>],
    [t.help3Title, t.help3Body, <><Binding value={keys.rate1} />–<Binding value={keys.rate5} /><Binding value={keys.pick} /><Binding value={keys.reject} /></>],
    [t.help4Title, t.help4Body, null],
  ];
  const fixed: [React.ReactNode, string][] = [
    [<><KeyCap>←</KeyCap><KeyCap>→</KeyCap></>, t.fixedKeys.arrows],
    [<KeyCap>↩</KeyCap>, t.fixedKeys.enter],
    [<KeyCap>Esc</KeyCap>, t.fixedKeys.esc],
    [t.mouseRight, t.fixedKeys.rightHold],
    [t.pinchOrCtrl, t.fixedKeys.pinch],
  ];
  return (
    <Sheet
      title={t.helpTitle}
      onClose={finish}
      footer={
        <>
          <span className="spacer" />
          <button className="primary" onClick={finish}>
            {onboarded ? t.done : t.getStarted}
          </button>
        </>
      }
    >
      <div className="help">
        <ol className="help-steps">
          {cards.map(([title, body, extra], i) => (
            <li key={i}>
              <span className="step">{i + 1}</span>
              <div>
                <strong>{title}</strong>
                <p>{body}</p>
                {!mobile && extra && <div className="help-keys">{extra}</div>}
              </div>
            </li>
          ))}
        </ol>
        {!mobile && (
          <>
            <h4>{t.shortcutList}</h4>
            <div className="key-list">
              {ACTIONS.map((a) => (
                <div key={a}>
                  <span><Binding value={keys[a]} /></span>
                  {t.actions[a]}
                </div>
              ))}
              {fixed.map(([cap, label], i) => (
                <div key={i}>
                  <span>{cap}</span>
                  {label}
                </div>
              ))}
            </div>
          </>
        )}
      </div>
    </Sheet>
  );
}
