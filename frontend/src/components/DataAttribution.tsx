import type { GeocodeAttribution } from "./LocationInput";

type Props = {
  /** source_year of the loaded release (health or route response); null while unknown. */
  dataYears: number[] | null;
  geocode: GeocodeAttribution;
  demo: boolean;
};

const UNSET = "（帰属表示が未設定です）";

const link = (href: string, text: string) => (
  <a href={href} target="_blank" rel="noreferrer">
    {text}
  </a>
);

/**
 * frontend-spec 10: providers and data years, plus the note that results are a
 * reference indicator. Provider strings are rendered as text, never as HTML.
 */
export function DataAttribution({ dataYears, geocode, demo }: Props) {
  const routing = import.meta.env.VITE_ROUTING_ATTRIBUTION;
  const map = import.meta.env.VITE_MAP_ATTRIBUTION;
  return (
    <footer className="data-attribution" aria-label="データの出典">
      <p>
        表示する事故件数と重みは、過去の事故記録に基づく参考指標です。交通量で補正した事故の起こりやすさではなく、安全を保証するものではありません。
      </p>
      {demo ? (
        <p>デモの経路・事故・天気・住所候補は架空のデータです。</p>
      ) : (
        <ul>
          <li>
            事故データ：
            {link(
              "https://www.npa.go.jp/publications/statistics/koutsuu/opendata/index_opendata.html",
              "警察庁 交通事故統計オープンデータ",
            )}
            {dataYears &&
              dataYears.length > 0 &&
              `（${dataYears.join("・")}年）`}
          </li>
          <li>
            気象データ：
            {link("https://open-meteo.com/", "Weather data by Open-Meteo.com")}
            （CC BY 4.0）
          </li>
          <li>
            祝日データ：
            {link(
              "https://www8.cao.go.jp/chosei/shukujitsu/gaiyou.html",
              "内閣府「国民の祝日」",
            )}
          </li>
          {geocode.length > 0 && (
            <li>
              住所検索：
              {geocode.map((item, index) => (
                <span key={item.url}>
                  {index > 0 && " / "}
                  {link(item.url, item.text)}
                </span>
              ))}
            </li>
          )}
          {/* Both are required build values (architecture.md 6); show the gap if missing. */}
          <li>経路：{routing || UNSET}</li>
          <li>地図：{map || UNSET}</li>
        </ul>
      )}
    </footer>
  );
}
