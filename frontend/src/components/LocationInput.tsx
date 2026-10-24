import { useEffect, useId, useRef, useState } from "react";
import { Check, LoaderCircle, MapPin, Search, X } from "lucide-react";
import { isDemo, searchPlaces } from "../api/client";
import type { GeocodeResponse, Place, Scenario } from "../lib/types";

export type LocationValue = { text: string; place: Place | null };
export type GeocodeAttribution = GeocodeResponse["attribution"];
type Props = {
  label: string;
  value: LocationValue;
  onChange: (value: LocationValue) => void;
  scenario: Scenario;
  action?: React.ReactNode;
  /** Receives the provider attribution of each geocode response (frontend-spec 10). */
  onAttribution?: (attribution: GeocodeAttribution) => void;
};

export default function LocationInput({
  label,
  value,
  onChange,
  scenario,
  action,
  onAttribution,
}: Props) {
  const id = useId();
  const [items, setItems] = useState<Place[]>([]);
  const [attribution, setAttribution] = useState<GeocodeAttribution>([]);
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [composing, setComposing] = useState(false);
  const [explicit, setExplicit] = useState(0);
  const generation = useRef(0);
  const input = useRef<HTMLInputElement>(null);
  const container = useRef<HTMLDivElement>(null);
  const controller = useRef<AbortController | null>(null);
  const isComposing = useRef(false);

  useEffect(() => {
    const close = (event: PointerEvent) => {
      if (!container.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", close);
    return () => document.removeEventListener("pointerdown", close);
  }, []);

  useEffect(() => {
    const token = ++generation.current;
    controller.current?.abort();
    setItems([]);
    setActive(-1);
    setMessage("");
    setOpen(false);
    setBusy(false);
    if (
      value.place ||
      composing ||
      value.text.trim().length < 2 ||
      (!isDemo && explicit === 0)
    )
      return;
    const abort = new AbortController();
    controller.current = abort;
    const timer = setTimeout(
      async () => {
        setBusy(true);
        setOpen(true);
        try {
          const data = await searchPlaces(
            value.text.trim(),
            abort.signal,
            scenario,
          );
          if (generation.current !== token) return;
          setItems(data.results.slice(0, 5));
          setAttribution(data.attribution);
          onAttribution?.(data.attribution);
          setMessage(
            data.results.length
              ? ""
              : "候補が見つかりません。別の住所や駅名で検索してください。",
          );
        } catch (error) {
          if (abort.signal.aborted || generation.current !== token) return;
          setMessage(
            error instanceof Error
              ? error.message
              : "住所候補を取得できませんでした。",
          );
        } finally {
          if (generation.current === token) setBusy(false);
        }
      },
      isDemo ? 300 : 0,
    );
    return () => {
      clearTimeout(timer);
      abort.abort();
    };
  }, [value.text, value.place, composing, explicit, scenario]);

  function choose(place: Place) {
    controller.current?.abort();
    ++generation.current;
    onChange({ text: place.label, place });
    setOpen(false);
    setBusy(false);
    input.current?.focus();
  }

  function edit(text: string) {
    // Invalidate immediately, before the debounce or React's next effect runs.
    controller.current?.abort();
    ++generation.current;
    setExplicit(0);
    onChange({ text, place: null });
  }

  return (
    <div className="location-field" ref={container}>
      <div className="field-label">
        <label htmlFor={id}>{label}</label>
        {action}
      </div>
      <div className={`location-control ${value.place ? "is-confirmed" : ""}`}>
        <MapPin size={19} aria-hidden="true" />
        <input
          id={id}
          ref={input}
          value={value.text}
          placeholder="住所・駅名・施設名を入力"
          autoComplete="off"
          role="combobox"
          aria-autocomplete="list"
          aria-expanded={open}
          aria-controls={`${id}-list`}
          aria-activedescendant={
            open && active >= 0 ? `${id}-${active}` : undefined
          }
          aria-describedby={value.place ? `${id}-help` : undefined}
          onChange={(event) => edit(event.target.value)}
          onCompositionStart={() => {
            isComposing.current = true;
            setComposing(true);
          }}
          onCompositionEnd={() => {
            isComposing.current = false;
            setComposing(false);
          }}
          onFocus={() => {
            if (!value.place && (items.length || message)) setOpen(true);
          }}
          onKeyDown={(event) => {
            if (
              isComposing.current ||
              event.nativeEvent.isComposing ||
              event.keyCode === 229
            )
              return;
            if (event.key === "Escape") {
              setOpen(false);
              return;
            }
            if (event.key === "ArrowDown" || event.key === "ArrowUp") {
              event.preventDefault();
              setOpen(true);
              if (items.length)
                setActive((index) =>
                  index < 0
                    ? event.key === "ArrowDown"
                      ? 0
                      : items.length - 1
                    : (index +
                        (event.key === "ArrowDown" ? 1 : -1) +
                        items.length) %
                      items.length,
                );
            }
            if (event.key === "Enter") {
              event.preventDefault();
              if (open && active >= 0 && items[active]) choose(items[active]);
              else if (!isDemo && !value.place && value.text.trim().length >= 2)
                setExplicit((count) => count + 1);
            }
          }}
        />
        {value.text && (
          <button
            className="icon-button"
            type="button"
            aria-label={`${label}をクリア`}
            onClick={() => {
              edit("");
              input.current?.focus();
            }}
          >
            <X size={17} />
          </button>
        )}
        {!isDemo && (
          <button
            className="icon-button"
            type="button"
            aria-label={`${label}の候補を検索`}
            disabled={value.text.trim().length < 2 || busy}
            onClick={() => setExplicit((count) => count + 1)}
          >
            <Search size={18} />
          </button>
        )}
      </div>
      {value.place && (
        <div id={`${id}-help`} className="field-help confirmed">
          <Check size={14} />
          地点を確定しました
        </div>
      )}
      {open && !value.place && (
        <div className="suggestions">
          <div className="suggestion-heading">
            {isDemo ? "デモ候補" : "住所候補"}
          </div>
          {busy && (
            <div className="suggestion-status" role="status">
              <LoaderCircle size={16} className="spin" />
              検索中…
            </div>
          )}
          <ul id={`${id}-list`} role="listbox" aria-label={`${label}の候補`}>
            {items.map((place, index) => (
              <li
                id={`${id}-${index}`}
                key={`${place.lat},${place.lon}`}
                role="option"
                aria-selected={index === active}
                onPointerDown={(event) => event.preventDefault()}
                onClick={() => choose(place)}
                onPointerMove={() => setActive(index)}
              >
                <MapPin size={17} aria-hidden="true" />
                <span>{place.label}</span>
              </li>
            ))}
          </ul>
          {message && (
            <p className="suggestion-status" role="status">
              {message}
            </p>
          )}
          {attribution.length > 0 && (
            <div className="attribution">
              {attribution.map((item) => (
                <a
                  key={item.url}
                  href={item.url}
                  target="_blank"
                  rel="noreferrer"
                >
                  {item.text}
                </a>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
