import { render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DataAttribution } from "./DataAttribution";

afterEach(() => vi.unstubAllEnvs());

const geocode = [
  { text: "Powered by Geoapify", url: "https://www.geoapify.com/" },
  {
    text: "© OpenStreetMap contributors",
    url: "https://www.openstreetmap.org/copyright",
  },
];

describe("DataAttribution (frontend-spec 10)", () => {
  it("lists every live provider with data years and geocode links", () => {
    vi.stubEnv("VITE_ROUTING_ATTRIBUTION", "経路提供元 <b>A</b>");
    vi.stubEnv("VITE_MAP_ATTRIBUTION", "地図提供元 B");
    render(
      <DataAttribution
        dataYears={[2023, 2024]}
        geocode={geocode}
        demo={false}
      />,
    );
    expect(
      screen.getByRole("link", { name: "警察庁 交通事故統計オープンデータ" }),
    ).toBeInTheDocument();
    expect(screen.getByText(/2023・2024年/)).toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: "Weather data by Open-Meteo.com" }),
    ).toHaveAttribute("href", "https://open-meteo.com/");
    expect(screen.getByText(/CC BY 4\.0/)).toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: "内閣府「国民の祝日」" }),
    ).toBeInTheDocument();
    for (const item of geocode)
      expect(screen.getByRole("link", { name: item.text })).toHaveAttribute(
        "href",
        item.url,
      );
    // Provider strings are text, never HTML.
    expect(screen.getByText("経路：経路提供元 <b>A</b>")).toBeInTheDocument();
    expect(screen.getByText("地図：地図提供元 B")).toBeInTheDocument();
    expect(
      screen.getByText(/過去の事故記録に基づく参考指標/),
    ).toBeInTheDocument();
  });

  it("makes missing live routing/map attribution visible instead of hiding it", () => {
    vi.stubEnv("VITE_ROUTING_ATTRIBUTION", "");
    vi.stubEnv("VITE_MAP_ATTRIBUTION", "");
    render(<DataAttribution dataYears={null} geocode={[]} demo={false} />);
    expect(
      screen.getByText("経路：（帰属表示が未設定です）"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("地図：（帰属表示が未設定です）"),
    ).toBeInTheDocument();
  });

  it("does not attribute fictional demo data to real providers", () => {
    render(<DataAttribution dataYears={[2024]} geocode={geocode} demo />);
    expect(screen.getByText(/架空のデータです/)).toBeInTheDocument();
    expect(screen.getByText(/参考指標/)).toBeInTheDocument();
    expect(screen.queryByRole("link")).not.toBeInTheDocument();
  });
});
