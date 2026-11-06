import type { Weather } from "../useWeather";
import Icon from "./Icon";

export default function WeatherCard({ weather }: { weather: Weather | null }) {
  if (!weather) {
    return <div className="card card--placeholder">memuat cuaca…</div>;
  }
  return (
    <div className="card weather">
      <Icon name={weather.icon} size={38} fill className="weather__icon" />
      <div className="weather__text">
        <div className="weather__city">{weather.city}</div>
        <div className="weather__temp">{weather.temp}°C</div>
        <div className="weather__label">{weather.label}</div>
      </div>
    </div>
  );
}
