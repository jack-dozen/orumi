import { useEffect, useState } from "react";

export type Weather = {
  city: string;
  temp: number;
  label: string;
  icon: string;
};

const REFRESH_MS = 30 * 60 * 1000;
const RETRY_MS = 2 * 60 * 1000;
const GEO_KEY = "edgeshell-geo-en";
const WEATHER_KEY = "edgeshell-weather-en";

// WMO weather code -> [text, daytime icon, nighttime icon]. Icons are Material Symbols names.
// https://open-meteo.com/en/docs#weathervariables
const WMO: Record<number, [string, string, string]> = {
  0: ["clear", "sunny", "clear_night"],
  1: ["mainly clear", "sunny", "clear_night"],
  2: ["partly cloudy", "partly_cloudy_day", "partly_cloudy_night"],
  3: ["overcast", "cloud", "cloud"],
  45: ["foggy", "foggy", "foggy"],
  48: ["foggy", "foggy", "foggy"],
  51: ["light drizzle", "rainy", "rainy"],
  53: ["drizzle", "rainy", "rainy"],
  55: ["heavy drizzle", "rainy", "rainy"],
  56: ["freezing drizzle", "rainy", "rainy"],
  57: ["freezing drizzle", "rainy", "rainy"],
  61: ["light rain", "rainy", "rainy"],
  63: ["rain", "rainy", "rainy"],
  65: ["heavy rain", "rainy", "rainy"],
  66: ["freezing rain", "rainy", "rainy"],
  67: ["freezing rain", "rainy", "rainy"],
  71: ["light snow", "weather_snowy", "weather_snowy"],
  73: ["snow", "weather_snowy", "weather_snowy"],
  75: ["heavy snow", "weather_snowy", "weather_snowy"],
  77: ["snow grains", "weather_snowy", "weather_snowy"],
  80: ["light showers", "rainy", "rainy"],
  81: ["showers", "rainy", "rainy"],
  82: ["heavy showers", "rainy", "rainy"],
  85: ["snow showers", "weather_snowy", "weather_snowy"],
  86: ["heavy snow showers", "weather_snowy", "weather_snowy"],
  95: ["thunderstorm", "thunderstorm", "thunderstorm"],
  96: ["thunderstorm with hail", "thunderstorm", "thunderstorm"],
  99: ["thunderstorm with hail", "thunderstorm", "thunderstorm"],
};

type Geo = { query: string; lat: number; lon: number; name: string };

function readCache<T>(key: string): T | null {
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : null;
  } catch {
    return null;
  }
}

function writeCache(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    /* no problem */
  }
}

/** City name -> coordinates (cached because they almost never change). */
async function geocode(city: string): Promise<Geo> {
  const cached = readCache<Geo>(GEO_KEY);
  if (cached && cached.query === city) return cached;

  const res = await fetch(
    `https://geocoding-api.open-meteo.com/v1/search?name=${encodeURIComponent(city)}&count=1&language=en&format=json`,
  );
  if (!res.ok) throw new Error(`geocoding ${res.status}`);
  const first = (await res.json())?.results?.[0];
  if (!first) throw new Error("city not found");

  const geo: Geo = { query: city, lat: first.latitude, lon: first.longitude, name: first.name };
  writeCache(GEO_KEY, geo);
  return geo;
}

async function fetchCurrent(geo: Geo): Promise<Weather> {
  const res = await fetch(
    `https://api.open-meteo.com/v1/forecast?latitude=${geo.lat}&longitude=${geo.lon}&current=temperature_2m,weather_code,is_day&timezone=auto`,
  );
  if (!res.ok) throw new Error(`forecast ${res.status}`);
  const cur = (await res.json())?.current;
  if (typeof cur?.temperature_2m !== "number") throw new Error("invalid response");

  const [label, day, night] = WMO[cur.weather_code] ?? [
    "unknown",
    "device_thermostat",
    "device_thermostat",
  ];
  return {
    city: geo.name,
    temp: Math.round(cur.temperature_2m),
    label,
    icon: cur.is_day === 1 ? day : night,
  };
}

/** Weather for the configured city. Updated every 30 minutes; retried 2 minutes after a failure. */
export function useWeather(city: string): Weather | null {
  const [weather, setWeather] = useState<Weather | null>(() => readCache<Weather>(WEATHER_KEY));

  useEffect(() => {
    let cancelled = false;
    let timer: number | undefined;

    const run = async () => {
      let next = REFRESH_MS;
      try {
        const geo = await geocode(city);
        const current = await fetchCurrent(geo);
        if (!cancelled) {
          setWeather(current);
          writeCache(WEATHER_KEY, current);
        }
      } catch {
        next = RETRY_MS;
      }
      if (!cancelled) timer = window.setTimeout(run, next);
    };

    run();
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [city]);

  return weather;
}
