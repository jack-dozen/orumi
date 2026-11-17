import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { BtCategory, BtDevice, BtNearbyDevice, BtPairPrompt, BtListResult } from "../useBluetooth";
import { useWifiList, type WifiNetwork } from "../useWifi";
import Icon from "./Icon";

const setNotchFocus = (enable: boolean) =>
  invoke("panel_activation", { label: "notch", enable }).catch(() => {});

function btCategoryIcon(cat: BtCategory): string {
  switch (cat) {
    case "headphones":
      return "headphones";
    case "speaker":
    case "audio":
      return "speaker";
    case "keyboard":
      return "keyboard";
    case "mouse":
      return "mouse";
    case "gamepad":
      return "gamepad";
    case "phone":
      return "smartphone";
    default:
      return "devices";
  }
}

function wifiSignalIcon(signal: number): string {
  if (signal >= 65) return "wifi";
  if (signal >= 32) return "wifi_2_bar";
  return "wifi_1_bar";
}

function ToggleSwitch({
  checked,
  disabled,
  label,
  onChange,
}: {
  checked: boolean;
  disabled?: boolean;
  label: string;
  onChange: (next: boolean) => void;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      className={`qswitch ${checked ? "qswitch--on" : ""}`}
      onClick={() => onChange(!checked)}
    >
      <span className="qswitch__thumb" />
    </button>
  );
}

export function WifiPanel({
  radioOn,
  busyToggle,
  visible,
  onToggleRadio,
  onBack,
  onOpenSettings,
}: {
  radioOn: boolean | null;
  busyToggle: boolean;
  visible: boolean;
  onToggleRadio: (on: boolean) => void;
  onBack: () => void;
  onOpenSettings: (page: string) => void;
}) {
  const effectiveOn = radioOn === true;
  const { data, scanning, scan, connect, disconnect, forget } = useWifiList(visible);
  const [selectedSsid, setSelectedSsid] = useState<string | null>(null);
  const [password, setPassword] = useState("");
  const [showPw, setShowPw] = useState(false);
  const [autoConnect, setAutoConnect] = useState(true);
  const [askNewPw, setAskNewPw] = useState(false);
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const pwRef = useRef<HTMLInputElement>(null);

  // Trigger a fresh scan when opening the Wi-Fi sub-view while the radio is on.
  useEffect(() => {
    if (visible && effectiveOn) {
      scan();
    }
  }, [visible, effectiveOn, scan]);

  // Release keyboard focus hold when closing or leaving.
  useEffect(() => {
    if (!visible) {
      setSelectedSsid(null);
      setPassword("");
      setError(null);
      setAskNewPw(false);
      setNotchFocus(false);
    }
  }, [visible]);

  const handleBack = () => {
    setNotchFocus(false);
    onBack();
  };

  const selectNetwork = (net: WifiNetwork) => {
    if (selectedSsid === net.ssid) {
      setSelectedSsid(null);
      setPassword("");
      setError(null);
      setAskNewPw(false);
      setNotchFocus(false);
      return;
    }
    setSelectedSsid(net.ssid);
    setPassword("");
    setShowPw(false);
    setAutoConnect(true);
    setError(null);
    setAskNewPw(false);
    const needsInput =
      net.secured &&
      !net.saved &&
      net.auth_kind !== "enterprise" &&
      net.auth_kind !== "other";
    if (needsInput) {
      setNotchFocus(true);
      window.setTimeout(() => pwRef.current?.focus(), 20);
    } else {
      setNotchFocus(false);
    }
  };

  const runConnect = async (net: WifiNetwork, pw?: string) => {
    setBusyKey(net.ssid);
    setError(null);
    setNotchFocus(false);
    try {
      await connect(net.ssid, {
        password: pw,
        autoConnect,
        authKind: net.auth_kind,
      });
      setPassword("");
      setAskNewPw(false);
    } catch (e) {
      const msg = typeof e === "string" ? e : String(e);
      setError(msg || "Unable to connect to this network");
      if (net.secured && !net.saved) {
        setNotchFocus(true);
        window.setTimeout(() => pwRef.current?.focus(), 20);
      }
    } finally {
      setBusyKey(null);
    }
  };

  const runDisconnect = async (net: WifiNetwork) => {
    setBusyKey(net.ssid);
    setError(null);
    try {
      await disconnect();
    } catch (e) {
      setError(typeof e === "string" ? e : String(e));
    } finally {
      setBusyKey(null);
    }
  };

  const runForget = async (net: WifiNetwork) => {
    const profile = net.profile_name ?? net.ssid;
    setBusyKey(net.ssid);
    setError(null);
    try {
      await forget(profile);
      setSelectedSsid(null);
    } catch (e) {
      setError(typeof e === "string" ? e : String(e));
    } finally {
      setBusyKey(null);
    }
  };

  const networks = data?.networks ?? [];
  const adapterMissing = radioOn === null || data?.adapter === false;

  return (
    <div className="qsub">
      <div className="qsub__head">
        <button type="button" className="qsub__back" onClick={handleBack} title="Back">
          <Icon name="arrow_back" size={20} />
        </button>
        <span className="qsub__title">Wi-Fi</span>
        <div className="qsub__head-right">
          {effectiveOn && (
            <button
              type="button"
              className={`qsub__iconbtn ${scanning ? "qsub__iconbtn--spin" : ""}`}
              onClick={scan}
              disabled={scanning}
              title="Scan for networks"
            >
              <Icon name="refresh" size={18} />
            </button>
          )}
          <ToggleSwitch
            checked={effectiveOn}
            disabled={adapterMissing || busyToggle}
            label="Toggle Wi-Fi"
            onChange={(next) => {
              setNotchFocus(false);
              onToggleRadio(next);
            }}
          />
        </div>
      </div>

      {data?.location_denied && (
        <div className="settings__warn">
          Windows blocked Wi-Fi scanning. Enable "Let desktop apps access your location" in Location
          settings.{" "}
          <button type="button" className="settings__link" onClick={() => onOpenSettings("location")}>
            Open Location settings
          </button>
        </div>
      )}

      {adapterMissing ? (
        <div className="qsub__empty">
          <Icon name="wifi_off" size={28} />
          <span>No Wi-Fi adapter detected</span>
        </div>
      ) : !effectiveOn ? (
        <div className="qsub__empty">
          <Icon name="wifi_off" size={28} />
          <span>Wi-Fi is turned off</span>
          <button
            type="button"
            className="qbtn qbtn--primary"
            disabled={busyToggle}
            onClick={() => onToggleRadio(true)}
          >
            Turn on Wi-Fi
          </button>
        </div>
      ) : networks.length === 0 ? (
        <div className="qsub__empty">
          <Icon name="wifi" size={28} />
          <span>{scanning ? "Scanning for networks…" : "No Wi-Fi networks found"}</span>
        </div>
      ) : (
        <div className="qsub__list">
          {networks.map((net) => {
            const expanded = selectedSsid === net.ssid;
            const isBusy = busyKey === net.ssid;
            const isUnsupported = net.auth_kind === "enterprise" || net.auth_kind === "other";
            const showPasswordBox =
              expanded && !net.connected && net.secured && !isUnsupported && (!net.saved || askNewPw);

            const subParts: string[] = [];
            if (net.connected) subParts.push("Connected");
            else if (net.saved) subParts.push("Saved");
            subParts.push(net.auth);

            return (
              <div
                key={net.ssid}
                className={`qnet ${net.connected ? "qnet--connected" : ""} ${
                  expanded ? "qnet--open" : ""
                }`}
              >
                <button
                  type="button"
                  className="qnet__row"
                  onClick={() => selectNetwork(net)}
                >
                  <span className="qnet__icon">
                    <Icon name={wifiSignalIcon(net.signal)} size={20} fill={net.connected} />
                    {net.secured && (
                      <span className="qnet__lock" aria-hidden="true">
                        <Icon name="lock" size={11} />
                      </span>
                    )}
                  </span>
                  <div className="qnet__meta">
                    <span className="qnet__name">{net.ssid}</span>
                    <span className="qnet__sub">{subParts.join(" · ")}</span>
                  </div>
                  <span className="qnet__pct">{net.signal}%</span>
                </button>

                {expanded && (
                  <div className="qnet__body">
                    {isUnsupported && !net.connected && !net.saved ? (
                      <div className="qnet__warn">
                        <span>{net.auth} requires Windows sign-in prompt.</span>
                        <button
                          type="button"
                          className="qbtn"
                          onClick={() => onOpenSettings("wifi")}
                        >
                          Open Settings
                        </button>
                      </div>
                    ) : showPasswordBox ? (
                      <form
                        className="qnet__form"
                        onSubmit={(e) => {
                          e.preventDefault();
                          if (password && !isBusy) runConnect(net, password);
                        }}
                      >
                        <div className="qnet__pw-wrap">
                          <input
                            ref={pwRef}
                            type={showPw ? "text" : "password"}
                            className="qinput"
                            placeholder="Enter the network security key"
                            value={password}
                            disabled={isBusy}
                            onFocus={() => setNotchFocus(true)}
                            onChange={(e) => setPassword(e.target.value)}
                            onKeyDown={(e) => {
                              if (e.key === "Escape") {
                                e.stopPropagation();
                                setSelectedSsid(null);
                                setPassword("");
                                setAskNewPw(false);
                                setNotchFocus(false);
                              }
                            }}
                          />
                          <button
                            type="button"
                            className="qnet__eye"
                            title={showPw ? "Hide password" : "Show password"}
                            onClick={() => setShowPw((s) => !s)}
                          >
                            <Icon name={showPw ? "visibility_off" : "visibility"} size={18} />
                          </button>
                        </div>

                        <label className="qcheck">
                          <input
                            type="checkbox"
                            checked={autoConnect}
                            onChange={(e) => setAutoConnect(e.target.checked)}
                          />
                          <span>Connect automatically</span>
                        </label>

                        <div className="qnet__btns">
                          <button
                            type="submit"
                            className="qbtn qbtn--primary"
                            disabled={!password || isBusy}
                          >
                            {isBusy ? "Connecting…" : "Connect"}
                          </button>
                          <button
                            type="button"
                            className="qbtn"
                            disabled={isBusy}
                            onClick={() => {
                              setSelectedSsid(null);
                              setPassword("");
                              setAskNewPw(false);
                              setNotchFocus(false);
                            }}
                          >
                            Cancel
                          </button>
                        </div>
                      </form>
                    ) : (
                      <>
                        {!net.connected && !net.saved && !net.secured && (
                          <label className="qcheck">
                            <input
                              type="checkbox"
                              checked={autoConnect}
                              onChange={(e) => setAutoConnect(e.target.checked)}
                            />
                            <span>Connect automatically</span>
                          </label>
                        )}
                        <div className="qnet__btns">
                          {net.connected ? (
                            <button
                              type="button"
                              className="qbtn"
                              disabled={isBusy}
                              onClick={() => runDisconnect(net)}
                            >
                              {isBusy ? "Disconnecting…" : "Disconnect"}
                            </button>
                          ) : (
                            <button
                              type="button"
                              className="qbtn qbtn--primary"
                              disabled={isBusy}
                              onClick={() => runConnect(net)}
                            >
                              {isBusy ? "Connecting…" : "Connect"}
                            </button>
                          )}
                          {net.saved && (
                            <button
                              type="button"
                              className="qbtn qbtn--ghost"
                              disabled={isBusy}
                              onClick={() => runForget(net)}
                            >
                              Forget
                            </button>
                          )}
                          {net.saved && !net.connected && net.secured && !isUnsupported && error && (
                            <button
                              type="button"
                              className="qbtn qbtn--ghost"
                              disabled={isBusy}
                              onClick={() => {
                                setAskNewPw(true);
                                setError(null);
                                setNotchFocus(true);
                                window.setTimeout(() => pwRef.current?.focus(), 20);
                              }}
                            >
                              Enter password
                            </button>
                          )}
                        </div>
                      </>
                    )}

                    {error && <div className="qnet__err">{error}</div>}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}

      <div className="qsub__foot">
        <button type="button" className="settings__link" onClick={() => onOpenSettings("wifi")}>
          More Wi-Fi settings
        </button>
      </div>
    </div>
  );
}

export function BluetoothPanel({
  radioOn,
  busyToggle,
  visible,
  btData,
  nearby,
  scanning,
  pairPrompt,
  onStartScan,
  onStopScan,
  onConnect,
  onDisconnect,
  onUnpair,
  onPair,
  onRespondPair,
  onToggleRadio,
  onBack,
  onOpenSettings,
}: {
  radioOn: boolean | null;
  busyToggle: boolean;
  visible: boolean;
  btData: BtListResult | null;
  nearby: BtNearbyDevice[];
  scanning: boolean;
  pairPrompt: BtPairPrompt | null;
  onStartScan: () => Promise<void>;
  onStopScan: () => Promise<void>;
  onConnect: (d: BtDevice) => Promise<void>;
  onDisconnect: (d: BtDevice) => Promise<void>;
  onUnpair: (d: BtDevice) => Promise<void>;
  onPair: (d: BtNearbyDevice) => Promise<void>;
  onRespondPair: (accept: boolean, pin?: string) => Promise<void>;
  onToggleRadio: (on: boolean) => void;
  onBack: () => void;
  onOpenSettings: (page: string) => void;
}) {
  const effectiveOn = radioOn === true;
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pinInput, setPinInput] = useState("");
  const pinRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (pairPrompt?.kind === "provide_pin") {
      setPinInput("");
      setNotchFocus(true);
      window.setTimeout(() => pinRef.current?.focus(), 20);
    } else if (!pairPrompt) {
      setPinInput("");
      setNotchFocus(false);
    }
  }, [pairPrompt]);

  useEffect(() => {
    if (!visible) {
      setError(null);
      setBusyId(null);
      setNotchFocus(false);
    }
  }, [visible]);

  const handleBack = () => {
    setNotchFocus(false);
    onStopScan();
    onBack();
  };

  const runAction = async (id: string, fn: () => Promise<void>) => {
    setBusyId(id);
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError(typeof e === "string" ? e : String(e));
    } finally {
      setBusyId(null);
    }
  };

  const devices = btData?.devices ?? [];
  const adapterMissing = radioOn === null || btData?.adapter === false;

  return (
    <div className="qsub">
      <div className="qsub__head">
        <button type="button" className="qsub__back" onClick={handleBack} title="Back">
          <Icon name="arrow_back" size={20} />
        </button>
        <span className="qsub__title">Bluetooth</span>
        <div className="qsub__head-right">
          {effectiveOn && (
            <button
              type="button"
              className={`qbtn qbtn--sm ${scanning ? "qbtn--primary" : ""}`}
              onClick={() => (scanning ? onStopScan() : onStartScan())}
              title={scanning ? "Stop scanning for devices" : "Scan for nearby devices to pair"}
            >
              <Icon name="bluetooth_searching" size={16} />
              <span>{scanning ? "Scanning…" : "Add device"}</span>
            </button>
          )}
          <ToggleSwitch
            checked={effectiveOn}
            disabled={adapterMissing || busyToggle}
            label="Toggle Bluetooth"
            onChange={(next) => {
              if (!next) onStopScan();
              onToggleRadio(next);
            }}
          />
        </div>
      </div>

      {pairPrompt && (
        <div className="qbt-prompt">
          <div className="qbt-prompt__title">
            {pairPrompt.kind === "confirm_pin" && `Pair with ${pairPrompt.name}?`}
            {pairPrompt.kind === "display_pin" && `Type PIN on ${pairPrompt.name}`}
            {pairPrompt.kind === "provide_pin" && `Enter PIN for ${pairPrompt.name}`}
          </div>
          {pairPrompt.pin && <div className="qbt-prompt__pin">{pairPrompt.pin}</div>}
          {pairPrompt.kind === "confirm_pin" && (
            <div className="qnet__btns">
              <button
                type="button"
                className="qbtn qbtn--primary"
                onClick={() => onRespondPair(true)}
              >
                Pair
              </button>
              <button type="button" className="qbtn" onClick={() => onRespondPair(false)}>
                Cancel
              </button>
            </div>
          )}
          {pairPrompt.kind === "provide_pin" && (
            <form
              className="qbt-prompt__form"
              onSubmit={(e) => {
                e.preventDefault();
                if (pinInput) {
                  setNotchFocus(false);
                  onRespondPair(true, pinInput);
                }
              }}
            >
              <input
                ref={pinRef}
                type="text"
                className="qinput"
                placeholder="Enter PIN (e.g. 0000 or 1234)"
                value={pinInput}
                onFocus={() => setNotchFocus(true)}
                onChange={(e) => setPinInput(e.target.value)}
              />
              <button type="submit" className="qbtn qbtn--primary" disabled={!pinInput}>
                Pair
              </button>
              <button
                type="button"
                className="qbtn"
                onClick={() => {
                  setNotchFocus(false);
                  onRespondPair(false);
                }}
              >
                Cancel
              </button>
            </form>
          )}
        </div>
      )}

      {error && (
        <div className="qnet__err qnet__err--banner">
          <span>{error}</span>
          <button type="button" className="settings__link" onClick={() => onOpenSettings("bluetooth")}>
            Open Settings
          </button>
        </div>
      )}

      {adapterMissing ? (
        <div className="qsub__empty">
          <Icon name="bluetooth_disabled" size={28} />
          <span>No Bluetooth adapter detected</span>
        </div>
      ) : !effectiveOn ? (
        <div className="qsub__empty">
          <Icon name="bluetooth_disabled" size={28} />
          <span>Bluetooth is turned off</span>
          <button
            type="button"
            className="qbtn qbtn--primary"
            disabled={busyToggle}
            onClick={() => onToggleRadio(true)}
          >
            Turn on Bluetooth
          </button>
        </div>
      ) : (
        <div className="qsub__list">
          {devices.length === 0 && !scanning && nearby.length === 0 ? (
            <div className="qsub__empty">
              <Icon name="bluetooth" size={28} />
              <span>No paired Bluetooth devices</span>
              <button type="button" className="qbtn" onClick={() => onStartScan()}>
                Scan for devices
              </button>
            </div>
          ) : (
            <>
              {devices.map((d) => {
                const isBusy = busyId === d.id;
                const statusText = d.connected
                  ? d.battery !== null
                    ? `Connected · ${d.battery}%`
                    : "Connected"
                  : "Paired";
                return (
                  <div
                    key={d.id}
                    className={`qbt ${d.connected ? "qbt--connected" : ""}`}
                  >
                    <span className={`qbt__icon ${d.connected ? "qbt__icon--on" : ""}`}>
                      <Icon name={btCategoryIcon(d.category)} size={20} fill={d.connected} />
                    </span>
                    <div className="qbt__meta">
                      <span className="qbt__name">{d.name}</span>
                      <span className="qbt__sub">{statusText}</span>
                    </div>
                    <div className="qbt__actions">
                      {d.connected ? (
                        <button
                          type="button"
                          className="qbtn qbtn--sm"
                          disabled={isBusy}
                          onClick={() => runAction(d.id, () => onDisconnect(d))}
                        >
                          {isBusy ? "…" : "Disconnect"}
                        </button>
                      ) : (
                        <button
                          type="button"
                          className="qbtn qbtn--sm qbtn--primary"
                          disabled={isBusy}
                          onClick={() => runAction(d.id, () => onConnect(d))}
                        >
                          {isBusy ? "Connecting…" : "Connect"}
                        </button>
                      )}
                      <button
                        type="button"
                        className="qbt__del"
                        title="Remove / unpair device"
                        disabled={isBusy}
                        onClick={() => runAction(d.id, () => onUnpair(d))}
                      >
                        <Icon name="delete" size={16} />
                      </button>
                    </div>
                  </div>
                );
              })}

              {(scanning || nearby.length > 0) && (
                <div className="qbt-section">
                  <div className="qbt-section__head">
                    <span>Available devices</span>
                    {scanning && <span className="qbt-section__scan">Searching…</span>}
                  </div>
                  {nearby.length === 0 ? (
                    <div className="qbt-section__empty">
                      Put your device in pairing mode nearby…
                    </div>
                  ) : (
                    nearby.map((n) => {
                      const isBusy = busyId === n.id;
                      return (
                        <div key={n.id} className="qbt">
                          <span className="qbt__icon">
                            <Icon name={btCategoryIcon(n.category)} size={20} />
                          </span>
                          <div className="qbt__meta">
                            <span className="qbt__name">{n.name}</span>
                            <span className="qbt__sub">
                              {n.is_le ? "Bluetooth LE" : "Bluetooth"}
                            </span>
                          </div>
                          <div className="qbt__actions">
                            <button
                              type="button"
                              className="qbtn qbtn--sm qbtn--primary"
                              disabled={isBusy || !n.can_pair}
                              onClick={() => runAction(n.id, () => onPair(n))}
                            >
                              {isBusy ? "Pairing…" : "Pair"}
                            </button>
                          </div>
                        </div>
                      );
                    })
                  )}
                </div>
              )}
            </>
          )}
        </div>
      )}

      <div className="qsub__foot">
        <button type="button" className="settings__link" onClick={() => onOpenSettings("bluetooth")}>
          More Bluetooth settings
        </button>
      </div>
    </div>
  );
}
