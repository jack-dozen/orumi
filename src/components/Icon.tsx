import { useUiStyle } from "../theme";

type Props = {
  name: string;
  size?: number;
  /** Filled variant for the active state. */
  fill?: boolean;
  className?: string;
};

/** Verified Windows 11 Segoe Fluent Icons codepoints mapped from Material Symbols names. */
const FLUENT_GLYPHS: Record<string, string> = {
  // Navigation & UI
  apps: "\uE71D",
  arrow_back: "\uE72B",
  check: "\uE73E",
  chevron_right: "\uE76C",
  close: "\uE711",
  dashboard: "\uE80A",
  delete: "\uE74D",
  done_all: "\uE8FB",
  expand_more: "\uE70D",
  keyboard: "\uE765",
  more_vert: "\uE712",
  refresh: "\uE72C",
  settings: "\uE713",
  tune: "\uE9E9",
  visibility: "\uE7B3",
  visibility_off: "\uED1A",
  wallpaper: "\uEB9F",

  // System & Power
  bedtime: "\uE708",
  bolt: "\uE945",
  brightness_medium: "\uE706",
  coffee: "\uEC32",
  content_cut: "\uE8C6",
  dark_mode: "\uE708",
  desktop_access_disabled: "\uE7F4",
  device_thermostat: "\uE9CA",
  eco: "\uE790",
  light_mode: "\uE706",
  lock: "\uE72E",
  logout: "\uF3B1",
  monitoring: "\uE9D9",
  notifications: "\uEA8F",
  notifications_off: "\uE7ED",
  power_settings_new: "\uE7E8",
  restart_alt: "\uE777",
  schedule: "\uE917",

  // Battery
  battery_alert: "\uE850",
  battery_1_bar: "\uE851",
  battery_2_bar: "\uE852",
  battery_3_bar: "\uE854",
  battery_4_bar: "\uE856",
  battery_5_bar: "\uE858",
  battery_6_bar: "\uE859",
  battery_full: "\uE83F",
  battery_charging_full: "\uEA93",

  // Connectivity & Devices
  airplanemode_active: "\uE709",
  bluetooth: "\uE702",
  bluetooth_connected: "\uE702",
  bluetooth_disabled: "\uEB5E",
  bluetooth_searching: "\uE895",
  do_not_disturb_on: "\uE708",
  wifi: "\uE701",
  wifi_1_bar: "\uE872",
  wifi_2_bar: "\uE873",
  wifi_lock: "\uE701",
  wifi_off: "\uF384",
  wifi_tethering: "\uE704",
  devices: "\uE772",
  gamepad: "\uE7FC",
  headphones: "\uE7F6",
  mouse: "\uE962",
  smartphone: "\uE8EA",
  speaker: "\uE7F5",

  // Audio & Media
  mic: "\uE720",
  mic_off: "\uEC54",
  music_note: "\uE8D6",
  pause: "\uE769",
  play_arrow: "\uE768",
  radio: "\uE90B",
  repeat: "\uE8EE",
  repeat_one: "\uE8ED",
  shuffle: "\uE8B1",
  skip_next: "\uE893",
  skip_previous: "\uE892",
  volume_down: "\uE994",
  volume_off: "\uE74F",
  volume_up: "\uE995",

  // Weather
  sunny: "\uE706",
  clear_night: "\uE708",
  partly_cloudy_day: "\uE753",
  partly_cloudy_night: "\uE753",
  cloud: "\uE753",
  foggy: "\uE753",
  rainy: "\uE753",
  weather_snowy: "\uE9CA",
  thunderstorm: "\uE945",
};

/** Adaptive icon: Material Symbols Rounded or Windows 11 Segoe Fluent Icons. */
export default function Icon({ name, size, fill, className }: Props) {
  const uiStyle = useUiStyle();
  const fluentGlyph = uiStyle === "fluent" ? FLUENT_GLYPHS[name] : undefined;

  if (fluentGlyph) {
    return (
      <span
        className={`fluent-icon icon${fill ? " icon--fill" : ""}${className ? ` ${className}` : ""}`}
        style={size ? { fontSize: size } : undefined}
        aria-hidden="true"
      >
        {fluentGlyph}
      </span>
    );
  }

  return (
    <span
      className={`material-symbols-rounded icon${fill ? " icon--fill" : ""}${
        className ? ` ${className}` : ""
      }`}
      style={size ? { fontSize: size } : undefined}
      aria-hidden="true"
    >
      {name}
    </span>
  );
}
