import {
  Bike,
  CarFront,
  Check,
  Cloud,
  CloudFog,
  CloudRain,
  CloudSnow,
  Footprints,
  Moon,
  MoonStar,
  RotateCcw,
  Sun,
  Sunrise,
  Sunset,
  type LucideIcon,
} from "lucide-react";
import type {
  ConditionValues,
  DayType,
  Period,
  Weather,
} from "../lib/types";

export type AutoFields = {
  period: boolean;
  day_type: boolean;
  weather: boolean;
};
type Props = {
  values: ConditionValues;
  onChange: (patch: Partial<ConditionValues>) => void;
  auto: AutoFields;
  onReset: (field: keyof AutoFields) => void;
  departure: "now" | "custom";
  dateTime: string;
  onDeparture: (mode: "now" | "custom", dateTime: string) => void;
  busy: boolean;
  warning: string;
  compact?: boolean;
};
const periods: { value: Period; hours: string; Icon: LucideIcon }[] = [
  { value: "早朝", hours: "04:00–08:59", Icon: Sunrise },
  { value: "昼", hours: "09:00–15:59", Icon: Sun },
  { value: "夕", hours: "16:00–18:59", Icon: Sunset },
  { value: "夜", hours: "19:00–23:59", Icon: Moon },
  { value: "深夜", hours: "00:00–03:59", Icon: MoonStar },
];
const weathers: { value: Weather; Icon: LucideIcon }[] = [
  { value: "晴", Icon: Sun },
  { value: "曇", Icon: Cloud },
  { value: "雨", Icon: CloudRain },
  { value: "霧", Icon: CloudFog },
  { value: "雪", Icon: CloudSnow },
];
const modes = [
  { value: "walk", label: "徒歩", Icon: Footprints },
  { value: "bike", label: "自転車", Icon: Bike },
  { value: "car", label: "自動車", Icon: CarFront },
] as const;

function Choice({
  selected,
  onClick,
  label,
  Icon,
  detail,
}: {
  selected: boolean;
  onClick: () => void;
  label: string;
  Icon?: LucideIcon;
  detail?: string;
}) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      className={`choice ${selected ? "selected" : ""}`}
      onClick={onClick}
    >
      {selected && (
        <Check className="choice-check" size={13} aria-hidden="true" />
      )}
      {Icon && <Icon size={24} strokeWidth={1.6} aria-hidden="true" />}
      <span>{label}</span>
      {detail && <small>{detail}</small>}
    </button>
  );
}

export default function ConditionFields({
  values,
  onChange,
  auto,
  onReset,
  departure,
  dateTime,
  onDeparture,
  busy,
  warning,
  compact = false,
}: Props) {
  const heading = (label: string, key: keyof AutoFields) => (
    <div className="group-heading">
      <span>
        {label}
        <span className="source-tag">{auto[key] ? "自動" : "手動"}</span>
      </span>
      {!auto[key] && (
        <button
          className="text-button"
          type="button"
          onClick={() => onReset(key)}
        >
          <RotateCcw size={13} />
          自動に戻す
        </button>
      )}
    </div>
  );
  return (
    <div className={`condition-fields ${compact ? "compact" : ""}`}>
      <fieldset>
        <legend>
          出発日時 <small>日本時間</small>
        </legend>
        <div className="segmented" role="radiogroup" aria-label="出発日時">
          <button
            type="button"
            role="radio"
            aria-checked={departure === "now"}
            onClick={() => onDeparture("now", dateTime)}
          >
            今すぐ出発
          </button>
          <button
            type="button"
            role="radio"
            aria-checked={departure === "custom"}
            onClick={() => onDeparture("custom", dateTime)}
          >
            日時を指定
          </button>
        </div>
        {departure === "custom" && (
          <input
            className="date-input"
            aria-label="出発日時（日本時間）"
            type="datetime-local"
            value={dateTime}
            min="2000-01-01T00:00"
            max="2100-12-31T23:59"
            onChange={(event) => onDeparture("custom", event.target.value)}
          />
        )}
      </fieldset>
      <fieldset>
        <legend>移動手段</legend>
        <div className="choices" role="radiogroup" aria-label="移動手段">
          {modes.map(({ value, label, Icon }) => (
            <Choice
              key={value}
              selected={values.mode === value}
              label={label}
              Icon={Icon}
              onClick={() => onChange({ mode: value })}
            />
          ))}
        </div>
      </fieldset>
      <fieldset>
        <legend className="sr-only">天気</legend>
        {heading("天気", "weather")}
        <div className="choices" role="radiogroup" aria-label="天気">
          {weathers.map(({ value, Icon }) => (
            <Choice
              key={value}
              selected={values.weather === value}
              label={value}
              Icon={Icon}
              onClick={() => onChange({ weather: value })}
            />
          ))}
        </div>
        <button
          type="button"
          className={`text-button all-option ${values.weather === "all" ? "active" : ""}`}
          aria-pressed={values.weather === "all"}
          onClick={() => onChange({ weather: "all" })}
        >
          {values.weather === "all" && <Check size={14} />}指定しない
        </button>
      </fieldset>
      <fieldset>
        <legend className="sr-only">時間帯</legend>
        {heading("時間帯", "period")}
        <div
          className="choices time-choices"
          role="radiogroup"
          aria-label="時間帯"
        >
          {periods.map(({ value, hours, Icon }) => (
            <Choice
              key={value}
              selected={values.period === value}
              label={value}
              Icon={Icon}
              detail={hours}
              onClick={() => onChange({ period: value })}
            />
          ))}
        </div>
        <button
          type="button"
          className={`text-button all-option ${values.period === "all" ? "active" : ""}`}
          aria-pressed={values.period === "all"}
          onClick={() => onChange({ period: "all" })}
        >
          {values.period === "all" && <Check size={14} />}全時間帯
        </button>
      </fieldset>
      <fieldset>
        <legend className="sr-only">曜日区分</legend>
        {heading("曜日区分", "day_type")}
        <div className="segmented" role="radiogroup" aria-label="曜日区分">
          {(["平日", "休日", "all"] as const).map((value) => (
            <button
              key={value}
              type="button"
              role="radio"
              aria-checked={values.day_type === value}
              onClick={() => onChange({ day_type: value as DayType | "all" })}
            >
              {value === "all" ? "全曜日" : value}
            </button>
          ))}
        </div>
      </fieldset>
      <fieldset className="detour-field">
        <legend>何分まで遠回りできますか？</legend>
        <div className="range-value">
          <output htmlFor={compact ? "detour-compact" : "detour"}>
            {values.max_detour_min}
            <span>分まで</span>
          </output>
        </div>
        <input
          id={compact ? "detour-compact" : "detour"}
          type="range"
          min="0"
          max="30"
          step="1"
          aria-label="許容追加時間"
          value={values.max_detour_min}
          onChange={(event) =>
            onChange({ max_detour_min: Number(event.target.value) })
          }
        />
        <div className="range-labels">
          <span>0分</span>
          <span>15分</span>
          <span>30分</span>
        </div>
      </fieldset>
      {busy && (
        <p className="field-help" role="status">
          条件を取得しています…
        </p>
      )}
      {warning && (
        <p className="notice warning" role="status">
          {warning}
        </p>
      )}
    </div>
  );
}
