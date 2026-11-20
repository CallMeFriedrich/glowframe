# Glowframe – Spezifikation v1

> Arbeitstitel „Glowframe“, jederzeit umbenennbar.
> Diese Datei ist die verbindliche Quelle für **was** gebaut wird. `CLAUDE.md` beschreibt **wie**.

## 1. Ziel

Ein natives Windows-Programm (ohne WSL, ohne Browser-Unterbau), das per Tray-Icon oder Hotkey einen **Vollbild-Modus** startet:

- Auf dem Hauptmonitor erscheint ein schwarzer Bildschirm mit einem **leuchtenden Rahmen**. Der Rahmen läuft um den Bildschirmrand, nimmt die Farben des aktuellen Spotify-Covers an und pulsiert im Beat.
- In der Mitte wird abwechselnd eine **Player-Ansicht** oder eine **Cover-Ansicht** angezeigt, optional mit zeilengenauen **Lyrics**.
- Alle anderen Monitore werden schwarz. Das ist pro Monitor einstellbar.

Vorbild: das Instagram-Reel mit „Lumn“ und dem Song „Flashing Lights“. Die Notch-Aussparung des MacBooks wird **nicht** übernommen.

## 2. Nicht-Ziele (v1)

- Keine anderen Player außer Spotify: kein Apple Music, kein Browser, kein YouTube.
- Keine Spotify Web API, kein Login, kein Spotify-Developer-Account. Daten kommen ausschließlich aus der Windows-Mediensteuerung (GSMTC).
- Kein Overlay im normalen Desktop-Betrieb. Der Rahmen existiert nur im Vollbild-Modus.
- Keine Notch.
- Keine Leiste unten im Vollbild und kein Lyrics-Button. Alles wird **ausschließlich in den Einstellungen** gesteuert.
- Kein Autostart mit Windows.
- Keine Lyrics-Karte im Apple-Maps-Stil und keine Verci-Effekte.
- Keine wortgenauen Lyrics. Zeilengenau reicht. LRCLIB liefert inzwischen teilweise Wort-Timings (`hasWordSync`), das ist ein Kandidat für später.
- Kein Fortschrittsbalken und keine Lautstärkeregelung.

## 3. Plattform & Technik

| Thema | Entscheidung |
|---|---|
| OS | Windows 10 (2004+) und Windows 11, x64 |
| Sprache | Rust (stable, MSVC-Toolchain) |
| Windows-APIs | `windows` crate (windows-rs): GSMTC, WASAPI, Direct3D 11, Direct2D, DirectWrite, WIC, DXGI |
| Fenster/Event-Loop | `winit` für alle Fenster. Die Vollbildfenster rendern direkt über ihr HWND. |
| Rendering Vollbild | D3D11-Swapchain (Flip-Model, VSync). Der Rahmen ist ein HLSL-Pixelshader, die UI (Cover, Text, Buttons) läuft über Direct2D/DirectWrite auf derselben DXGI-Surface. |
| Einstellungsfenster | `egui` 0.37 über `egui-winit` + `egui_glow` (glutin, OpenGL über das System-`opengl32.dll`) im gemeinsamen winit-Event-Loop. Das Fenster existiert nur, solange es offen ist. *Entscheidung Phase 0:* `eframe` hat einen eigenen Event-Loop, `egui-wgpu` ist zu schwer für das Exe-Größenziel, und die D3D11-Backends (`egui-directx11`, `egui-d3d11`) hängen der egui-Version hinterher. *Umsetzung Phase 7:* GL-Kontext direkt über glutin (nur WGL, nur hardwarebeschleunigte Pixelformate), ohne `glutin-winit`; Schrift Segoe UI aus `%WINDIR%\Fonts` statt eingebetteter egui-Fonts. |
| Tray | `tray-icon` |
| Globaler Hotkey | `global-hotkey` |
| FFT | `realfft` / `rustfft` |
| HTTP | schlanker Blocking-Client (z. B. `ureq`) in einem eigenen Thread, kein Tokio |
| Settings | `serde` + `serde_json` |
| Bild-Decoding | WIC (nativ, kein zusätzlicher Crate) |
| Ergebnis | eine einzelne `.exe` mit statischer CRT und ohne mitgelieferte DLLs. Dazu ein portables ZIP und ein Installer. |

**Leichtgewichtigkeit ist ein Hauptziel.** Zielwerte (Release-Build, 1440p):

- **Im Tray (Modus aus):** < 15 MB RAM, ~0 % CPU. Kein Rendering, kein Audio-Capture.
- **Modus aktiv:** < 60 MB RAM, < 3 % CPU auf einer aktuellen Desktop-CPU, Rendering mit Monitor-Refresh.
- **Schwarze Monitore:** werden einmal gezeichnet, dann läuft dort keine Render-Loop.
- **Exe-Größe:** < 10 MB.

## 4. Bedienung

### Tray-Icon
- **Rechtsklick → Menü:**
  - **Starten** (bzw. **Stoppen**, wenn der Modus läuft)
  - **Einstellungen**
  - Trennlinie
  - **Beenden**
- **Linksklick (Doppelklick):** startet den Modus.
- **Tooltip:** „Glowframe“ plus der aktuelle Titel, falls Spotify läuft.

### Hotkey
- **Standard:** `Strg+Alt+L`, in den Einstellungen änderbar (Tastenkombination aufnehmen).
- **Funktion:** schaltet den Modus um. Ist er aus, wird er gestartet; läuft er, wird er beendet.
- **Belegte Kombination:** Schlägt die Registrierung fehl, kommt eine Tray-Benachrichtigung, und in den Einstellungen erscheint ein Hinweis.

### Beenden des Modus
- **Esc zweimal** innerhalb von 600 ms.
- **Beim ersten Esc** erscheint unten mittig ein dezenter Hinweis „Nochmal Esc zum Beenden“ / „Press Esc again to exit“, der nach 1,5 s ausblendet.
- **Alternativen:** der Hotkey oder Tray → Stoppen.

### Einzelinstanz
- **Mechanismus:** benannter Mutex.
- **Zweiter Start:** Läuft das Programm schon, öffnet ein erneuter Start die Einstellungen der laufenden Instanz und beendet sich selbst.

## 5. Vollbild-Modus

### 5.1 Monitore
- **Fenster:** pro Monitor ein randloses Fenster (`WS_POPUP`, `WS_EX_TOPMOST`, `WS_EX_TOOLWINDOW`, also ohne Taskleisteneintrag), das exakt den Monitor abdeckt. DPI-Awareness: Per-Monitor v2. Stil und Monitorrechteck werden selbst gesetzt und bei jeder Größen-, Positions- oder DPI-Änderung wiederhergestellt; Fenster werden erst sichtbar, wenn sie schwarz gemalt bzw. gerendert sind.
- **Modus pro Monitor** (Einstellung):
  - **Animation:** Rahmen und Player/Cover
  - **Schwarz**
  - **Frei:** wird nicht abgedeckt
- **Standard:** Primärmonitor = Animation, alle anderen = Schwarz.
- **Mehrere Animations-Monitore:** Mehrere Monitore auf „Animation“ sind erlaubt. Jeder rendert dann eigenständig.
- **Monitor-Änderungen:** Werden im laufenden Modus Monitore an- oder abgesteckt, werden die Fenster neu aufgebaut.
- **Bildschirm wach halten:** Während der Modus aktiv ist, verhindern Bildschirmschoner und Energiesparen ein Abschalten (`SetThreadExecutionState`).
- **Mauszeiger:** in der Cover-Ansicht ausgeblendet, bei Bewegung sofort wieder da.

### 5.2 Ansichten und Wechsel
Es gibt drei Ansichten:

- **Player-Ansicht:** erscheint bei Mausbewegung.
- **Ruhe-Ansicht:** erscheint, wenn die Maus *N* Sekunden ruht (Einstellung, Standard 5 s). Sie ist entweder die **Cover-Ansicht** (§5.4) oder die **Lyrics-Fokus-Ansicht** (§5.4a, v1.2).
  - Jedes Mal, wenn die Ruhe-Ansicht beginnt, wird mit 50:50-Chance gewählt, ob die Cover-Ansicht oder die Lyrics-Fokus-Ansicht erscheint.
  - Bleibt die Maus länger ruhig, wechseln sich beide in zufälligen Abständen von 5 min ± 2 min ab (weiche Überblendung per Feder).
  - Ohne Lyrics bleibt es bei der Cover-Ansicht.

**Uhr (v1.2):** In allen Ansichten steht oben mittig eine Zeile im Stil des iOS-Sperrbildschirms: Uhrzeit fett, daneben das Datum normal und leicht gedimmt (DE: „16:46  Fr. 20. Nov.“, EN: „4:46  Fri, Nov 20“). Sie folgt der Sprache der Einstellungen und lässt sich in den Einstellungen ausschalten (§7); das Umschalten blendet weich ein bzw. aus.

Der Übergang ist ein weicher Crossfade mit leichtem Scale, etwa 400 ms, mit Easing.

**Bewegungsphysik (v1.1):** Alle Übergänge der Oberfläche laufen über gedämpfte Federn wie bei iOS (Parameter „response“ und „Dämpfung“): schneller Ansatz, weiches Ausschwingen, bei Skalierungen ein minimales Überschwingen. Das gilt für Ansichtswechsel, Layoutwechsel zentriert ↔ geteilt, Cover-Überblendung beim Trackwechsel (neues Cover skaliert leicht herein), Lyrics-Scrollen und Zeilenwechsel, Pause/Standby-Überblendung, Button-Feedback und den Start des Modus (Inhalt und Rahmen blenden mit leichtem Scale ein).

### 5.3 Player-Ansicht
Orientiert sich am zweiten Screenshot.

**Ohne Lyrics** (Lyrics aus, nicht gefunden oder nur unsynchronisiert):
- **Cover:** zentriert, abgerundete Ecken, weicher Schatten bzw. Glow in der Cover-Farbe.
- **Darunter:** Titel (fett, hell) und Interpret (kleiner, gedimmt).
- **Darunter die Steuerung:**
  - Shuffle, Zurück, **Play/Pause** (größer, hervorgehoben), Vor, Repeat
  - Runde, dunkle, halbtransparente Buttons
  - Hover-State, Klick-Feedback
  - Shuffle und Repeat zeigen ihren aktiven Zustand. Repeat hat drei Zustände: aus, alle, eins.
  - Bietet die Spotify-Session eine Funktion nicht an, ist der Button gedimmt und nicht klickbar.

**Mit Lyrics** (und „Lyrics in Player-Ansicht“ aktiv):
- **Links (~40 %):** Cover, Titel, Interpret und Steuerung.
- **Rechts (~60 %):** Lyrics im Apple-Music-Stil.
  - **Aktuelle Zeile:** groß, hell, voll deckend.
  - **Vergangene und kommende Zeilen:** kleiner, gedimmt, leicht weichgezeichnet.
  - **Scrollen:** weich, die aktuelle Zeile bleibt etwa auf 40 % der Höhe.
  - **Instrumentale Lücken** (über 4 s ohne Zeile): animierte Punkte „• • •“.

Der Wechsel zwischen den Layouts (zentriert ↔ geteilt) wird animiert und springt nicht.

### 5.4 Cover-Ansicht
Orientiert sich am ersten Screenshot.
- **Cover:** groß und zentriert, etwa 70 % der Bildschirmhöhe, mit Ambient-Glow dahinter in der Cover-Farbe.
- **Lyrics** (wenn aktiv und „Lyrics in Cover-Ansicht“ an):
  - unten mittig die aktuelle Zeile, darunter gedimmt die nächste
  - über einem dezenten Verlauf nach Schwarz, damit es lesbar bleibt
  - Zeilenwechsel mit weichem Fade/Slide

### 5.4a Lyrics-Fokus-Ansicht (v1.2)
- Aufbau wie die Player-Ansicht mit Lyrics (§5.3), aber **ohne Steuerungsbuttons**.
- **Links:** größeres Cover (füllt den frei gewordenen Platz), darunter Titel und Interpret.
- **Rechts:** Lyrics im gleichen Stil wie in der Player-Ansicht.
- Nur verfügbar, wenn synchronisierte Lyrics vorhanden und eingeschaltet sind.

### 5.5 Leuchtrahmen
- **Form:** Er folgt dem rechteckigen Bildschirmrand mit leicht abgerundeten Ecken und leuchtet weich nach innen aus (Glow-Falloff, keine harte Linie).
- **Rotation (v1.1, „Komet“):** Ein heller, kompakter **Kopf** (~12 % des Umfangs, weicher Vorlauf) zieht einen weich auslaufenden **Schweif** (~30 % des Umfangs) hinter sich her und **wandert im Uhrzeigersinn um den Monitor**: Start unten links, den linken Rand hoch, oben entlang, rechts hinunter, unten zurück.
  - **Geschwindigkeit:** über die Perimeter-Koordinate gleichmäßig, damit es an den Ecken nicht ruckelt. Beim Start des Modus beschleunigt der Komet per Feder aus dem Stand.
  - **Grundglühen:** Auf dem restlichen Umfang bleibt ein schwaches Glühen (etwa 5 %), damit der Rahmen nie ganz dunkel ist, die Bewegung aber auch bei einfarbigen Covern klar sichtbar bleibt.
  - *Vorher (v1.0):* Bogen über ~60 % mit 15 % Grundglühen; bei einfarbigen Covern war kaum Bewegung zu sehen.
- **Farben:**
  - **Farbmodus Cover:** 2–3 Farben aus dem Cover, als Verlauf vom Kopf zum Schweifende.
  - **Farbmodus Feste Palette:** Preset oder eigene Farben.
- **Beat:**
  - **Bei jedem erkannten Beat:** Helligkeit und Breite pulsieren kurz (Attack ~10 ms, Decay ~180 ms).
  - **Lautstärke-Energie:** Die geglättete Energie moduliert zusätzlich die Grundhelligkeit leicht.
  - **Stärke:** einstellbar.
- **Trackwechsel:** Die Farben werden in OKLab über ~1,5 s interpoliert, ohne harten Sprung.
- **Umsetzung:** Pixelshader mit den Uniforms Zeit, Rotationsphase, Palette (bis 3 Farben), Helligkeit, Breite, Beat-Puls, Energie und Bildschirmgröße.

### 5.6 Zustände

| Zustand | Verhalten |
|---|---|
| **Spielt** | normal wie oben beschrieben |
| **Pausiert** | Die Rotation bremst über ~1 s auf etwa 10 % Geschwindigkeit ab. Der Rahmen „atmet“ langsam (~0,2 Hz) zwischen 20 und 40 % Helligkeit. Das Cover wird leicht entsättigt und gedimmt, in der Player-Ansicht liegt ein dezentes Pause-Symbol darauf. Beat-Reaktion aus, die Lyrics bleiben auf der aktuellen Zeile stehen. Beim Fortsetzen läuft alles weich wieder hoch. |
| **Kein Spotify / Spotify geschlossen** | Standby-Bildschirm: Rahmen in der zuletzt genutzten Palette (oder Standard Pink/Lila), langsamer Bogen, atmend. Mittig steht „Warte auf Spotify …“ / „Waiting for Spotify …“ und klein darunter „Esc Esc zum Beenden“. Sobald eine Spotify-Session auftaucht, geht es automatisch weiter. |
| **Trackwechsel** | Das Cover blendet über (~500 ms), die Farben interpolieren, die Lyrics werden neu geladen. Während des Ladens (max. ~1,5 s) bleibt das aktuelle Layout stehen. Werden keine Lyrics gefunden, wechselt es animiert zum zentrierten Layout. |
| **Werbung (Spotify Free)** | Es wird angezeigt, was GSMTC liefert, ohne Lyrics. Kein Absturz und keine leeren Felder: Fehlt der Titel, steht dort „Spotify“. |
| **Leise Passagen / Stille** | Die Beat-Erkennung passt ihre Schwelle an. Kein Flackern durch Rauschen. |

## 6. Datenquellen

### 6.1 Mediensteuerung (GSMTC)
- **API:** `GlobalSystemMediaTransportControlsSessionManager`.
- **Session-Auswahl:** **Nur** Sessions, deren `SourceAppUserModelId` „Spotify“ enthält (ohne Groß-/Kleinschreibung; deckt die Desktop- und die Store-Version ab). Alle anderen Sessions werden ignoriert, auch wenn sie gerade spielen.
- **Events:** `SessionsChanged`, `MediaPropertiesChanged`, `PlaybackInfoChanged`, `TimelinePropertiesChanged`.
- **Daten:** Titel, Interpret, Album, Thumbnail, Wiedergabestatus, Shuffle und Repeat (inkl. ob verfügbar), Timeline.
- **Steuerung:** Play/Pause, Vor, Zurück, Shuffle und Repeat über die `Try…Async`-Methoden.
- **Position:**
  - **Berechnung:** `Position + (jetzt − LastUpdatedTime)`, solange gespielt wird.
  - **Hinweis:** Spotify aktualisiert die Timeline nur sporadisch, daher wird bei jedem Event neu synchronisiert.
  - **Offset:** Ein globaler Lyrics-Versatz in ms ist in den Einstellungen justierbar (erweitert).

### 6.2 Audio-Analyse (Beat)
- **Capture:**
  - **Bevorzugt:** WASAPI **Process Loopback** nur auf den Spotify-Prozess (inkl. Kindprozesse), damit Systemsounds, Discord usw. keine Beats auslösen.
  - **Fallback:** Wird Process Loopback nicht unterstützt oder schlägt es fehl, wird das Standard-Wiedergabegerät im Loopback-Modus mitgeschnitten.
- **Laufzeit:** nur während der Modus aktiv ist, in einem eigenen Thread.
- **Analyse:**
  - Mono-Mix, FFT 1024 bei 512 Hop.
  - Spectral Flux im Bassband (~40–150 Hz) mit adaptiver Schwelle (gleitender Median der letzten ~1 s × Faktor) und Mindestabstand zwischen Beats (~250 ms).
  - *Umsetzung (Phase 5):* Flux = positiver Log-Anstieg der Bassband-Energie (über 2 Hops gemittelt) gegenüber max(Mittel der letzten 3 Hops, Median der letzten ~1 s). Schwelle = Median + Faktor × MAD des Flux über ~1 s, mindestens ein Energieanstieg von ~3× bei 60 % Beat-Reaktion. Grund: Mit nur ~3 FFT-Bins im Bassband löste „Median × Faktor“ bei Rauschen zu viele Beats aus (Tests mit synthetischem Signal).
  - Zusätzlich eine geglättete RMS-Energie.
- **Ausgabe an den Render-Thread:** Beat-Puls und Energie (0..1), lock-frei über Atomics oder einen Ring-Buffer.
- **Einstellung „Beat-Reaktion“:** skaliert Schwellenfaktor und Puls-Amplitude. Bei 0 % ist der Rahmen ohne Beat.

### 6.3 Cover
- **Quelle:** das GSMTC-Thumbnail, über WIC dekodiert.
- **Bekanntes Problem:** Spotify liefert dort oft nur eine geringe Auflösung. Im Vollbild wäre das unscharf.
- **HD-Cover** (Einstellung, Standard an):
  - **Suche:** Album über die öffentliche iTunes Search API (`itunes.apple.com/search`, ohne Key), Artwork-URL auf ~1200×1200 skalieren.
  - **Übernahme:** nur, wenn Interpret und Album nach Normalisierung übereinstimmen (Kleinschreibung, ohne Zusätze wie „(Deluxe)“, „- Remastered“).
  - **Sonst:** GSMTC-Thumbnail mit hochwertiger Interpolation.
- **Cache:** `%LOCALAPPDATA%\Glowframe\cache\covers\`, Schlüssel = Hash aus Interpret und Album, max. ~200 MB (LRU).
- **Farbextraktion:**
  - Cover auf 64×64 skalieren, in OKLab umrechnen, k-means mit k=5.
  - Fast schwarze, fast weiße und sehr unbunte Cluster verwerfen, außer das Cover ist durchgehend monochrom.
  - Nach Chroma × Anteil sortieren, 2–3 Farben nehmen.
  - Helligkeit und Sättigung für die Darstellung auf Schwarz anheben.
- **Fallback-Palette:** `#FF2EA6`, `#B026FF`, `#5B3CFF`.

### 6.4 Lyrics (LRCLIB)
- **Endpunkt:** `GET https://lrclib.net/api/get?track_name=…&artist_name=…&album_name=…&duration=…` (Dauer in s, LRCLIB akzeptiert ±2 s).
- **Fallback:** `GET /api/search?track_name=…&artist_name=…`, bester Treffer nach Dauerdifferenz, nur bei ≤ 3 s Abweichung.
- **Header:** Der `User-Agent` ist Pflicht: `Glowframe/<version> (https://github.com/<user>/glowframe)`.
- **Ausgewertet wird nur `syncedLyrics`** (LRC: `[mm:ss.xx] Text`).
  - `instrumental: true` → keine Lyrics.
  - Nur `plainLyrics` → keine Lyrics, weil nicht synchronisierbar.
- **404 (`TrackNotFound`):** wird im Cache als „nicht gefunden“ vermerkt und nach 7 Tagen erneut versucht.
- **Rate-Limit (429):** `Retry-After` beachten, plus Jitter. Nie mehr als ein Request gleichzeitig.
- **Cache:** `%LOCALAPPDATA%\Glowframe\cache\lyrics\` als JSON pro Track.

## 7. Einstellungen

**Fenster:** normales Fenster außerhalb des Vollbilds, im gleichen dunklen Stil. Rechts oben ein Sprachumschalter **DE | EN**, der sofort wirkt.

**Übernehmen:** Änderungen gelten **sofort**, auch in einem laufenden Vollbild-Modus (Live-Vorschau) und werden automatisch gespeichert. Es gibt keinen „Speichern“-Button.

| Gruppe | Einstellung | Typ / Werte | Standard |
|---|---|---|---|
| Allgemein | Sprache | DE / EN | Systemsprache (DE wenn Deutsch, sonst EN) |
| Allgemein | Hotkey | Kombination aufnehmen | `Strg+Alt+L` |
| Monitore | Modus je Monitor | Animation / Schwarz / Frei (Liste mit Monitorname und Auflösung, Primär markiert) | Primär = Animation, Rest = Schwarz |
| Ansicht | Zeit bis Cover-Ansicht | 2–60 s | 5 s |
| Ansicht | Datum und Uhrzeit anzeigen (v1.2) | an/aus | an |
| Rahmen | Helligkeit | 0–100 % | 80 % |
| Rahmen | Breite | 0–100 % (relativ zur Bildschirmhöhe) | 50 % |
| Rahmen | Rotationsgeschwindigkeit | 0–100 % (0 = steht) | 40 % (≈ 1 Umlauf / 12 s) |
| Rahmen | Beat-Reaktion | 0–100 % | 60 % |
| Rahmen | Farbmodus | Cover / Feste Palette | Cover |
| Rahmen | Palette (bei „Feste Palette“) | Presets (Neon Pink, Ocean, Sunset, Mono) oder 3 eigene Farben | Neon Pink |
| Lyrics | Lyrics anzeigen | an/aus (Hauptschalter) | an |
| Lyrics | In Player-Ansicht | an/aus | an |
| Lyrics | In Cover-Ansicht | an/aus | an |
| Lyrics (erweitert) | Versatz | −2000 … +2000 ms | 0 |
| Cover | HD-Cover laden | an/aus | an |

**Speicherorte:**
- **Settings:** `%APPDATA%\Glowframe\settings.json`
- **Portabler Modus:** Liegt neben der `.exe` eine Datei `portable.txt`, landen Settings, Cache und Logs neben der `.exe` statt in `%APPDATA%`/`%LOCALAPPDATA%`.
- **Defekte oder alte Settings-Datei:** Unbekannte Felder werden ignoriert, fehlende mit Standardwerten ergänzt. Eine defekte Datei wird als `.bak` gesichert und neu angelegt.

## 8. Sprache / i18n
- **Texte:** Alle UI-Texte (Einstellungen, Tray-Menü, Hinweise im Vollbild) liegen in `locales/de.toml` und `locales/en.toml` und werden per `include_str!` eingebettet.
- **Fehlende Schlüssel:** Fallback auf Englisch.

## 9. Auslieferung
- **Portabel:** `Glowframe-<version>-portable.zip` mit `Glowframe.exe` und `portable.txt`.
- **Installer:** `Glowframe-<version>-setup.exe`, gebaut mit Inno Setup 6.
  - **Installation:** pro Benutzer ohne Admin-Rechte nach `%LOCALAPPDATA%\Programs\Glowframe`.
  - **Umfang:** Startmenü-Verknüpfung und Deinstallation. Kein Autostart.
  - **Deinstallation:** fragt optional, ob Settings und Cache gelöscht werden sollen.
- **Exe-Ausstattung:** App-Icon und Versionsinfo (aus `Cargo.toml`) sind eingebettet.
- **Build:** ein Skript `scripts/package.ps1` erzeugt beide Artefakte nach `dist/`.

## 10. Akzeptanzkriterien v1
1. Ein Rechtsklick aufs Tray-Icon zeigt Starten/Einstellungen/Beenden, und alle drei funktionieren.
2. `Strg+Alt+L` startet und beendet den Modus. Zweimal Esc beendet ihn ebenfalls.
3. Der Primärmonitor zeigt den Rahmen plus Player/Cover, alle anderen Monitore sind schwarz (Standard). Ein Umstellen in den Einstellungen wirkt sofort.
4. Mit laufendem Spotify passen Titel, Interpret und Cover. Steuerung (inkl. Shuffle und Repeat) funktioniert. Andere Medienquellen (z. B. YouTube im Browser) werden ignoriert.
5. Der Rahmen rotiert gleichmäßig, übernimmt die Cover-Farben und pulsiert hörbar passend zum Beat. Bei Pause und ohne Spotify verhält er sich wie in 5.6.
6. Nach 5 s ohne Maus wechselt die Ansicht zum Cover, bei Mausbewegung zurück.
7. Für einen gängigen Song mit LRCLIB-Eintrag laufen die Lyrics zeilengenau synchron. Gefühlt liegt die Abweichung unter 0,5 s.
8. Ohne Lyrics zeigt der Player das zentrierte Layout wie im Screenshot.
9. Einstellungen werden gespeichert und überleben einen Neustart, DE/EN schaltet sofort um.
10. Die Zielwerte aus Abschnitt 3 werden eingehalten (mit dem Task-Manager gemessen).
11. Portables ZIP und Installer funktionieren auf einem frischen Windows ohne zusätzliche Runtimes.
