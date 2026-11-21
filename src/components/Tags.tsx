import { useEffect, useImperativeHandle, useRef, useState, type Ref } from "react";
import type { Item, Tag } from "../lib/api";
import { useT } from "../lib/i18n";
import { TAG_ACTIONS, keyCaps } from "../lib/shortcuts";
import { TAG_COLORS, tagsOf, useStore } from "../lib/store";
import { tipProps, useInsideWindow } from "../ui/controls";
import { Icon } from "../ui/Icon";

/** How many remembered names are offered at once. */
const MAX_SUGGESTIONS = 6;

export function TagDot({ color }: { color: number }) {
  return <i className="tag-dot" style={{ background: TAG_COLORS[color % TAG_COLORS.length] }} />;
}

/** The tags a photo carries, in the order the folder lists them. */
export function tagsOn(tags: Tag[], tagged: Record<string, string[]>, item: Item): Tag[] {
  const ids = tagsOf(tagged, item);
  return ids.length ? tags.filter((tag) => ids.includes(tag.id)) : [];
}

export type TagMenuHandle = { open: () => void };

/** A button that opens the folder's tags: tick the ones `targets` should carry, or make a new one. */
export function TagMenu(props: { targets: Item[]; label: string; up?: boolean; keys?: string[]; ref?: Ref<TagMenuHandle> }) {
  const t = useT();
  const tags = useStore((s) => s.tags);
  const tagged = useStore((s) => s.tagged);
  const keys = useStore((s) => s.settings.keys);
  const remembered = useStore((s) => s.settings.tagNames);
  const mobile = useStore((s) => !!s.info?.mobile);
  const act = useStore.getState();
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState("");
  /** The tag whose delete button has been pressed once. */
  const [doomed, setDoomed] = useState<string | null>(null);
  const anchor = useRef<HTMLDivElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  useImperativeHandle(props.ref, () => ({ open: () => setOpen(true) }), []);

  useInsideWindow(menu, open);

  useEffect(() => {
    if (!open) return;
    // A phone would bring up its keyboard over the list.
    if (!mobile) input.current?.focus();
    const close = (e: Event) => {
      if (e instanceof KeyboardEvent) {
        if (e.key !== "Escape") return;
        // Escape closes only the menu, not the screen underneath.
        e.stopPropagation();
        setOpen(false);
      } else if (!anchor.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("pointerdown", close, true);
    window.addEventListener("keydown", close, true);
    return () => {
      window.removeEventListener("pointerdown", close, true);
      window.removeEventListener("keydown", close, true);
      setEditing(false);
      setDoomed(null);
      setName("");
    };
  }, [open, mobile]);

  const ids = props.targets.map((it) => it.id);
  const carried = (tag: Tag) => props.targets.filter((it) => tagsOf(tagged, it).includes(tag.id)).length;
  const marked = props.targets.some((it) => tagsOf(tagged, it).length > 0);
  const create = (text: string) => {
    const id = act.addTag(text);
    if (id && ids.length) act.tag(ids, id, true);
    setName("");
  };
  const rename = (tag: Tag, field: HTMLInputElement) => {
    if (field.value === tag.name || act.renameTag(tag.id, field.value)) return;
    if (field.value.trim()) act.notify("tagTaken");
    field.value = tag.name;
  };
  const suggestions = remembered.filter((n) => !tags.some((tag) => tag.name.toLowerCase() === n.toLowerCase())).slice(0, MAX_SUGGESTIONS);

  return (
    <div className={`menu-anchor ${marked ? "on" : ""}`} ref={anchor}>
      <button className={`icon-button ${open ? "on" : ""}`} {...tipProps(props.label, props.keys)} aria-label={props.label} aria-expanded={open} onClick={() => setOpen(!open)}>
        <Icon name="tag" />
      </button>
      {open && (
        <div ref={menu} className={`menu solid tag-menu ${props.up ? "up" : "right"}`} role="menu" aria-label={t.tags}>
          {!tags.length && <p className="tag-hint">{t.tagHint}</p>}
          {tags.map((tag, i) => {
            if (editing)
              return (
                <div className="tag-edit" key={tag.id}>
                  <button className="tag-color" {...tipProps(t.tagColor)} aria-label={t.tagColor} onClick={() => act.recolorTag(tag.id)}>
                    <TagDot color={tag.color} />
                  </button>
                  <input
                    className="text"
                    key={tag.name}
                    defaultValue={tag.name}
                    maxLength={40}
                    spellCheck={false}
                    aria-label={tag.name}
                    onBlur={(e) => rename(tag, e.currentTarget)}
                    onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
                  />
                  <button
                    className={`tag-remove ${doomed === tag.id ? "sure" : ""}`}
                    {...tipProps(t.tagRemove)}
                    aria-label={t.tagRemove}
                    onClick={() => (doomed === tag.id ? act.removeTag(tag.id) : setDoomed(tag.id))}
                  >
                    {doomed === tag.id ? t.tagRemoveSure : <Icon name="x" size={14} />}
                  </button>
                </div>
              );
            const n = carried(tag);
            const all = n > 0 && n === ids.length;
            const binding = TAG_ACTIONS[i] ? keys[TAG_ACTIONS[i]] : "";
            return (
              <button key={tag.id} role="menuitemcheckbox" aria-checked={all ? true : n > 0 ? "mixed" : false} disabled={!ids.length} onClick={() => act.tag(ids, tag.id)}>
                <span className="menu-check">{all ? <Icon name="check" size={15} /> : n > 0 ? "–" : null}</span>
                <TagDot color={tag.color} />
                <span className="tag-name">{tag.name}</span>
                {binding && !mobile && <span className="tag-key">{keyCaps(binding).join("")}</span>}
              </button>
            );
          })}
          {tags.length > 0 && <hr />}
          <form
            className="tag-new"
            onSubmit={(e) => {
              e.preventDefault();
              create(name);
            }}
          >
            <input ref={input} className="text" value={name} maxLength={40} spellCheck={false} placeholder={t.tagNew} aria-label={t.tagNew} onChange={(e) => setName(e.target.value)} />
            <button type="submit" disabled={!name.trim()} {...tipProps(t.tagAdd)} aria-label={t.tagAdd}>
              <Icon name="plus" size={16} />
            </button>
          </form>
          {suggestions.length > 0 && (
            <div className="tag-suggest" aria-label={t.tagRecent}>
              <small>{t.tagRecent}</small>
              {suggestions.map((n) => (
                <button key={n} onClick={() => create(n)}>
                  {n}
                </button>
              ))}
            </div>
          )}
          {tags.length > 0 && (
            <button className="tag-manage" onClick={() => (setEditing(!editing), setDoomed(null))}>
              {editing ? t.done : t.tagManage}
            </button>
          )}
        </div>
      )}
    </div>
  );
}
