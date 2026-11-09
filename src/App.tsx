import { getCurrentWindow } from "@tauri-apps/api/window";
import "material-symbols/rounded.css";
import Bar from "./components/Bar";
import Launcher from "./components/Launcher";
import Notch from "./components/Notch";
import Overlay from "./components/Overlay";
import Switcher from "./components/Switcher";
import { useTheme } from "./theme";
import "./App.css";

/** Semua window memuat frontend yang sama; labelnya menentukan isinya. */
export default function App() {
  const label = getCurrentWindow().label;
  // Window notch yang membaca wallpaper dan menghitung palet; yang lain hanya mengikuti.
  useTheme(label === "notch");

  if (label === "notch") return <Notch />;
  if (label === "launcher") return <Launcher />;
  if (label === "overlay") return <Overlay />;
  if (label === "switcher") return <Switcher />;
  return <Bar />;
}
