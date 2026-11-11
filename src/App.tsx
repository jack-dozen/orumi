import { getCurrentWindow } from "@tauri-apps/api/window";
import Bar from "./components/Bar";
import Launcher from "./components/Launcher";
import Notch from "./components/Notch";
import Overlay from "./components/Overlay";
import Switcher from "./components/Switcher";
import Preview from "./components/Preview";
import { useTheme } from "./theme";
import "./App.css";
import Activities from "./activities/Activities";

/** All windows load the same frontend; the label determines their contents. */
export default function App() {
  const label = getCurrentWindow().label;
  // The notch window reads the wallpaper and calculates the palette; the others only follow.
  useTheme(label === "notch");

  if (label === "notch") return <Notch />;
  if (label === "launcher") return <Launcher />;
  if (label === "overlay") return <Overlay />;
  if (label === "switcher") return <Switcher />;
  if (label === "preview") return <Preview />;
  if (label === "activities") return <Activities />;
  return <Bar />;
}
