# Membuat font ikon Material Symbols Rounded yang hanya berisi ikon yang dipakai app.
# Jalankan dari folder project:
#   powershell -ExecutionPolicy Bypass -File scripts\make-icon-font.ps1
# Setiap kali menambah ikon baru di kode, tambahkan namanya ke daftar di bawah lalu jalankan ulang.

$icons = @(
  "apps", "battery_1_bar", "battery_2_bar", "battery_3_bar", "battery_4_bar",
  "battery_5_bar", "battery_6_bar", "battery_alert", "battery_charging_full", "battery_full",
  "bedtime", "brightness_medium", "clear_night", "cloud", "dashboard",
  "desktop_access_disabled", "device_thermostat", "foggy", "keyboard", "lock", "logout",
  "monitoring", "more_vert", "music_note", "partly_cloudy_day", "partly_cloudy_night",
  "pause", "play_arrow", "power_settings_new", "radio", "rainy", "refresh", "restart_alt",
  "schedule", "settings", "skip_next", "skip_previous", "sunny", "thunderstorm", "tune",
  "volume_down", "volume_off", "volume_up", "wallpaper", "weather_snowy", "wifi", "wifi_off"
)

# Google mewajibkan daftar nama terurut abjad.
$sorted = [string[]]($icons | Select-Object -Unique)
[Array]::Sort($sorted, [StringComparer]::Ordinal)
$names = $sorted -join ","

$ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36"
$cssUrl = "https://fonts.googleapis.com/css2?family=Material+Symbols+Rounded:opsz,wght,FILL,GRAD@24,400,0..1,0&icon_names=$names&display=block"
$css = (Invoke-WebRequest -UseBasicParsing -Headers @{ "User-Agent" = $ua } -Uri $cssUrl).Content
$m = [regex]::Match($css, 'url\((https://[^)]+)\)')
if (-not $m.Success) { throw "URL font tidak ditemukan di respons Google Fonts" }

New-Item -ItemType Directory -Force -Path "src\assets" | Out-Null
Invoke-WebRequest -UseBasicParsing -Uri $m.Groups[1].Value -OutFile "src\assets\material-symbols-rounded.woff2"
$kb = [math]::Round((Get-Item "src\assets\material-symbols-rounded.woff2").Length / 1KB, 1)
"Selesai: $($sorted.Count) ikon, $kb KB"
