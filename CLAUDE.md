# CLAUDE.md – Glowframe

Natives Windows-Programm in Rust: Spotify-„Now Playing“-Vollbildmodus mit rotierendem, beat-reaktivem Leuchtrahmen in Cover-Farben, Player-/Cover-Ansicht und zeilengenauen Lyrics.

**Die verbindliche Spezifikation steht in `SPEC.md`.** Bei Widersprüchen gilt SPEC.md. Bei Unklarheiten den Projektinhaber fragen, nicht raten. Änderungen am Umfang werden zuerst in SPEC.md nachgezogen.

## Zusammenarbeit
- **Sprache:** Mit dem Projektinhaber wird Deutsch gesprochen. Er möchte direkte, technisch präzise Antworten.
- **Code:** Bezeichner, Kommentare und Commit-Messages auf Englisch. UI-Texte nur über `locales/*.toml`.
- **Visuelle Abnahme:** Visuelle Ergebnisse kann nur der Projektinhaber beurteilen. Nach jeder Phase mit sichtbarer Änderung bauen, starten und ihn um eine kurze Abnahme bitten, bevor es weitergeht.
- **Der Vollbild-Modus überdeckt den ganzen Bildschirm.** Für eigene Tests deshalb immer mit `--exit-after <sek>` starten (siehe unten). Den Modus nie unangekündigt auf dem Bildschirm des Projektinhabers starten.

## Umgebung
- **Plattform:** Windows nativ, **kein WSL**. Shell: PowerShell.
- **Toolchain:** Ist noch nicht installiert, das ist Phase 0. Installation per `winget`:
  - `Rustlang.Rustup` → `rustup default stable-x86_64-pc-windows-msvc`
  - `Microsoft.VisualStudio.2022.BuildTools` mit Workload `Microsoft.VisualStudio.Workload.VCTools` und Windows-SDK (`--includeRecommended`)
  - `JRSoftware.InnoSetup` (für den Installer)
  - `GitHub.cli`, `Git.Git` (falls nicht vorhanden)
- **Admin-Rechte:** Installationen, die eine UAC-Bestätigung brauchen, vorher ankündigen. Der Projektinhaber bestätigt sie selbst.
- **GitHub:** Vor dem Anlegen des Repos muss der Projektinhaber `gh auth login` selbst durchführen. Darum bitten, nicht umgehen.

## Stack (Details in SPEC.md §3)
- **Kern:** `windows` (windows-rs) für GSMTC, WASAPI, D3D11, D2D, DirectWrite, WIC und DXGI.
- **Fenster und Bedienung:** `winit`, `tray-icon`, `global-hotkey`.
- **Einstellungsfenster:** `egui`.
- **Analyse und Netzwerk:** `realfft`, `ureq`, `serde`/`serde_json`, `tracing` + `tracing-appender`.
- **Crate-Versionen:** **Vor dem Hinzufügen die aktuelle Version und API auf crates.io/docs.rs prüfen.** Nicht aus dem Gedächtnis annehmen, die Windows-Crates ändern ihre APIs häufig.
- **Grenzen:** Kein Tokio, keine Webview, kein Electron/Tauri. Jede neue Abhängigkeit kurz begründen, denn Leichtgewichtigkeit ist ein Hauptziel.

## Architektur (Zielstruktur)
```
src/
  main.rs            # Einstieg, Single-Instance, CLI-Flags, Event-Loop
  app.rs             # Zustandsmaschine: Idle (Tray) ↔ Active (Vollbild)
  tray.rs            # Tray-Icon + Menü
  hotkey.rs
  settings/          # Modell, Laden/Speichern, Defaults, Migration, Pfade (portable)
  i18n.rs            # locales/de.toml, en.toml eingebettet
  media/             # GSMTC: Spotify-Session-Filter, Metadaten, Steuerung, Positions-Interpolation
  audio/             # WASAPI Process-Loopback + Fallback, FFT, Beat/Energie
  cover/             # Thumbnail, HD-Cover (iTunes), WIC-Decode, Farbextraktion (OKLab k-means), Cache
  lyrics/            # LRCLIB-Client, LRC-Parser, Cache, Zeilen-Timing
  render/            # D3D11-Device/Swapchain pro Monitor, Glow-Shader (HLSL), D2D-UI-Layer
    shaders/glow.hlsl
  ui/                # Player-Ansicht, Cover-Ansicht, Lyrics-Layout, Animationen/Easing, Hit-Testing
  settings_window/   # egui-Fenster
locales/  assets/ (icon)  installer/glowframe.iss  scripts/package.ps1
```
**Threads:**
- **UI/Event-Loop:** Rendering.
- **Audio-Thread:** Capture + Analyse; liefert Werte lock-frei an den Render-Thread.
- **Netzwerk-Worker:** HTTP für Lyrics und Cover über eine Job-Queue.
- **GSMTC:** Events werden in die Event-Loop gepostet.

Im Idle-Zustand laufen **weder Rendering noch Audio-Capture**.

## Befehle
```powershell
cargo build                     # Debug
cargo build --release           # Release
cargo fmt; cargo clippy --all-targets -- -D warnings
cargo test
.\scripts\package.ps1           # → dist\ (portable ZIP + Setup)
```
**Release-Profil:** `lto = "fat"`, `codegen-units = 1`, `strip = true`, `panic = "abort"`. In `.cargo/config.toml`: `+crt-static` für eine Exe ohne VC-Runtime. Außerdem `#![windows_subsystem = "windows"]` im Release.

## CLI-Flags für Entwicklung und Tests
- `--start`: Modus sofort starten.
- `--exit-after <sek>`: Programm nach X Sekunden beenden. **Für eigene Testläufe immer verwenden.**
- `--demo`: Fake-Spotify-Daten (Titel, Cover aus `assets/demo/`, eigenes Testbild, kein urheberrechtlich geschütztes Cover) und synthetischer 120-BPM-Beat. Damit lässt sich ohne Spotify testen.
- `--log-level <level>`: Log-Ausgabe.

**Logs:** `%LOCALAPPDATA%\Glowframe\logs\` (portabel: neben der Exe), rotierend, die letzten 3.

## Konventionen
- **Fehlerbehandlung:** Kein `unwrap()`/`expect()` in Laufzeitpfaden. Fehler loggen und degradiert weiterlaufen: keine Lyrics, kein Beat, Fallback-Palette. Das Programm darf wegen Netzwerk-, Audio- oder Spotify-Problemen nie abstürzen.
- **`unsafe`:** Nur in den Windows-API-Wrappern, mit kurzem `// SAFETY:`-Kommentar.
- **Tests:** Unit-Tests für die reine Logik: LRC-Parser, Positions-Interpolation, Beat-Detektor (mit synthetischem Signal), Farbextraktion, Settings-Migration, Namens-Normalisierung für den HD-Cover-Abgleich.
- **Urheberrecht:** Keine fremden Cover, Songtexte oder Audiodateien ins Repo, auch nicht als Testdaten. Demo-Assets selbst erzeugen.

## Git
- **Repo:** privates GitHub-Repo `glowframe` (`gh repo create glowframe --private --source . --push`).
- **Ignorieren:** `target/`, `dist/`, Logs, Cache.
- **Commits:** Conventional Commits (`feat:`, `fix:`, `chore:` …), mindestens ein Commit pro abgeschlossener Phase. Pushen nach jeder Phase.

## Phasenplan
Jede Phase endet mit: Build ok, `clippy` sauber, Tests grün, Commit + Push und bei sichtbaren Änderungen der Abnahme durch den Projektinhaber.

0. **Setup:**
   - Toolchain installieren
   - Projektgerüst, Release-Profil, Logging, Single-Instance, CLI-Flags
   - Git + privates Repo
   - Crate-Versionen prüfen und Settings-Renderer für egui festlegen (Entscheidung kurz in SPEC.md §3 notieren)
1. **Tray & Modus-Gerüst:**
   - Tray-Menü, Hotkey, Doppel-Esc
   - Settings-Modell + Persistenz (inkl. portable)
   - Vollbildfenster pro Monitor nach Monitor-Modus, schwarze Fenster
   - Bildschirm wach halten
2. **Spotify (GSMTC):**
   - Session-Filter, Metadaten, Thumbnail
   - Steuerung inkl. Shuffle/Repeat
   - Positions-Interpolation, Zustände Spielt/Pause/Keine Session
   - Debug-Ausgabe im Log
3. **Rendering & Rahmen:**
   - D3D11/D2D-Pipeline
   - Glow-Shader mit Rotation, Grundglühen und Palette
   - Farbextraktion + Übergänge
   - Demo-Modus
4. **UI:**
   - Player-Ansicht (zentriert), Cover-Ansicht
   - Wechsel nach Maus-Inaktivität, Cursor-Handling
   - Animationen
   - Zustände Pause, Standby und Trackwechsel nach SPEC §5.6
   - HD-Cover inkl. Cache
5. **Beat:**
   - Process Loopback auf Spotify + Fallback
   - FFT, Beat und Energie
   - Kopplung an den Rahmen, Sensitivität
6. **Lyrics:**
   - LRCLIB-Client + Cache + Parser
   - Geteiltes Layout in der Player-Ansicht
   - Lyrics-Zeile in der Cover-Ansicht, Versatz
7. **Einstellungsfenster:**
   - egui-UI mit allen Einstellungen aus SPEC §7
   - Live-Übernahme, DE/EN-Umschalter rechts oben, Hotkey-Recorder
8. **Packaging:**
   - Icon/Versionsinfo
   - Inno-Setup-Skript, `package.ps1`
   - Test auf sauberem System (sofern möglich: den Projektinhaber fragen)
9. **Feinschliff:** Performance gegen die Zielwerte aus SPEC §3 messen, Optimierungen, Akzeptanzkriterien SPEC §10 durchgehen.

## Bekannte Stolperstellen
- **Spotify-Session:** Die `SourceAppUserModelId` unterscheidet sich zwischen der Desktop- und der Store-Version. Per „contains spotify“ ohne Groß-/Kleinschreibung filtern.
- **Timeline:** Spotify meldet die Position nur sporadisch. Interpolieren und bei jedem Event neu synchronisieren.
- **Thumbnail:** Das GSMTC-Cover ist oft niedrig aufgelöst, daher der HD-Cover-Pfad (SPEC §6.3).
- **Process Loopback** (`AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK`, `ActivateAudioInterfaceAsync`) ist nicht auf jeder Windows-10-Version verfügbar. Die Laufzeitprüfung muss sauber auf das System-Loopback zurückfallen.
- **Fokus:** Topmost-Vollbildfenster brauchen den Tastaturfokus für Esc. Beim Start `SetForegroundWindow` verwenden und den Fokus bei Klick zurückholen. Der Hotkey funktioniert immer.
- **DPI:** Per-Monitor v2 und unterschiedliche Skalierungen pro Monitor beachten (Schriftgrößen in DIPs).
- **Ressourcen:** Bei `WM_DISPLAYCHANGE` und `DXGI_ERROR_DEVICE_REMOVED` die Swapchain bzw. das Device neu aufbauen.
