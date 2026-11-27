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
- ~~Kein Autostart mit Windows.~~ Seit v1.5 als Option (Standard aus, §7).
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

### Zifferntasten 1–9 (v1.5, ersetzt Taste F aus v1.3)
- Wählen im laufenden Modus den Lyrics-Stil direkt: **1** Klassisch, **2** Nachrichten (§5.4b), **3** Sternbild (§5.4c), **4** Sternbild V2 (§5.4c2), **5** Code (§5.4e), **6** Ein Wort (§5.4f). Weitere Stile bekommen die nächsten Ziffern bis 9; unbelegte Ziffern tun nichts.
- Die Wahl wird gespeichert und ist auch in den Einstellungen änderbar (§7).

### Einzelinstanz
- **Mechanismus:** benannter Mutex.
- **Start (v1.7):** Ein manueller Start (Windows-Suche, Startmenü, Verknüpfung) öffnet sofort die Show. Läuft das Programm schon, startet ein erneuter Start die Show der laufenden Instanz (bzw. holt sie nach vorn) und beendet sich selbst. Die Einstellungen öffnet man über das Tray-Menü.
- **Still im Tray:** Autostart mit Windows und der Neustart nach einem Update starten mit `--tray` und öffnen keine Show; ein solcher Start bei schon laufender Instanz tut nichts.

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
- **Hintergrund (v1.5, nur Animations-Monitore):**
  - **Schwarz** (Standard).
  - **Windows-Hintergrundbild:** das aktuelle Desktop-Hintergrundbild wird als Hintergrund gezeichnet („cover“-skaliert).
    - **Live-Wallpaper (Wallpaper Engine, Lively u. ä.):** Hängen sichtbare Fenster unter dem Desktop (`WorkerW` unter `Progman` bzw. eigenes `WorkerW`), wird der Desktop per Windows Graphics Capture live mitgeschnitten (ohne gelben Rahmen, ohne Mauszeiger) und pro Monitor der passende Ausschnitt gezeichnet. Die Desktop-Symbole werden dafür während des Modus ausgeblendet und danach wieder eingeblendet (auch nach einem Absturz beim nächsten Start). Wallpaper Engine muss „Andere Anwendung im Vollbild“ auf „Weiter abspielen“ stehen haben, sonst steht das Bild still. Schlägt die Aufnahme fehl, wird das statische Hintergrundbild verwendet.
  - **Eigenes Bild:** vom Nutzer gewählt (PNG/JPEG/BMP/WebP über WIC), Mindestauflösung 2560×1440; kleinere Bilder werden mit Hinweis abgelehnt. Das Bild wird in die App-Daten kopiert (`…\Glowframe\background.*`).
  - **Durchsichtig:** Das Fenster ist echt transparent (DirectComposition, Swapchain mit Alpha); man sieht alles dahinter (Desktop, offene Fenster). Eingaben gehen weiter an Glowframe.
  - **Abdunkeln:** 0–90 %, Standard 50 %; legt Schwarz über Bild bzw. Durchsicht, damit Rahmen und Text lesbar bleiben.
  - **Taskleiste sichtbar lassen:** Das Vollbildfenster deckt dann nur den Arbeitsbereich ab (ohne Taskleiste).

### 5.2 Ansichten und Wechsel
Es gibt drei Ansichten:

- **Player-Ansicht:** erscheint bei Mausbewegung.
  - **Aufweck-Schwelle (v1.5):** Während die Ruhe-Ansicht steht, zählt nur eine deutliche Bewegung: Die Maus muss innerhalb von 0,6 s eine Mindeststrecke zurücklegen, die mit der Ruhedauer wächst (≈ 40 DIP beim Erscheinen der Ruhe-Ansicht bis ≈ 160 DIP nach einer Minute Ruhe). Kleines Anstoßen weckt nicht. Klicks wecken sofort.
- **Ruhe-Ansicht:** erscheint, wenn die Maus *N* Sekunden ruht (Einstellung, Standard 5 s). Sie ist entweder die **Cover-Ansicht** (§5.4) oder die **Lyrics-Fokus-Ansicht** (§5.4a, v1.2).
  - Jedes Mal, wenn die Ruhe-Ansicht beginnt, wird mit 50:50-Chance gewählt, ob die Cover-Ansicht oder die Lyrics-Fokus-Ansicht erscheint.
  - Bleibt die Maus länger ruhig, wechseln sich beide in zufälligen Abständen von 5 min ± 2 min ab (weiche Überblendung per Feder).
  - Ohne Lyrics bleibt es bei der Cover-Ansicht.
  - Im Lyrics-Stil **Nachrichten** (v1.3) ist die Ruhe-Ansicht stattdessen die Nachrichten-Ansicht (§5.4b); ohne Lyrics bleibt es auch dort bei der Cover-Ansicht.

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

### 5.4b Nachrichten-Ansicht (v1.3)
Lyrics als Chatverlauf im Stil von iMessage (dunkles Design). Ruhe-Ansicht im Lyrics-Stil „Nachrichten“, nur mit synchronisierten Lyrics.
- **Kopf:** oben mittig unter der Uhr das Cover als runder Avatar, darunter Titel und Interpret (wie der Kontaktkopf eines Chats).
- **Verlauf:** mittige Spalte. Jede Lyrics-Zeile ist eine Bubble; die neueste steht unten, ältere rücken nach oben und blenden zum oberen Rand hin aus.
  - Bubbles abwechselnd links (grau) und rechts (blau), gelegentlich zwei hintereinander auf derselben Seite; die Seite ist pro Zeile fest (deterministisch).
  - Die jeweils letzte Bubble einer Folge hat das typische Schwänzchen.
- **Einblenden:** neue Bubble poppt mit Feder (Skalierung aus der Schwänzchen-Ecke mit leichtem Überschwingen) herein, die älteren rücken per Feder nach oben.
- **Wort für Wort:** In der aktuellen Bubble werden die Wörter nacheinander hell; sie sind bis dahin gedimmt. Da LRCLIB nur Zeilenzeiten liefert, wird die Zeit bis zur nächsten Zeile nach Wortlänge auf die Wörter verteilt (Näherung).
- **Instrumentale Lücken:** Tipp-Indikator (Bubble mit drei pulsierenden Punkten) auf der Seite der nächsten Zeile.
- **Karten:** Nennt eine Zeile einen Ort (Liste bekannter Städte, Länder und Orte, DE/EN) oder Wörter wie Karte, Maps, Stadt, Straße, Brücke, erscheint sie als Karten-Bubble: oben eine selbst gezeichnete, stilisierte Mini-Karte (Straßen, Wasser, Pin; aus dem Text erzeugt, kein Kartendienst, kein Netz), darunter die Zeile fett und eine Ortszeile mit Pin-Symbol (genannter Ort, sonst ein Ort aus der Liste).

### 5.4c Sternbild-Ansicht (v1.5)
Lyrics als Sternbild: Ruhe-Ansicht im Lyrics-Stil „Sternbild“, nur mit synchronisierten Lyrics.
- Jedes Wort der aktuellen Zeile ist ein kleiner vierzackiger Stern mit dem Wort darunter. Die Sterne liegen an **zufälligen Punkten auf dem ganzen Bildschirm** (jede Zeile ein neues Muster; außerhalb der Uhr und der Ränder), mit Regeln für ein sauberes Bild: Mindestabstand zwischen Sternen, Verbindungslinien mit begrenzter Länge und **ohne Überkreuzungen**.
- Die Wörter erscheinen nacheinander (Zeit bis zur nächsten Zeile nach Wortlänge verteilt, wie §5.4b); jeder neue Stern poppt per Feder mit leichtem Überschwingen auf, die Linie vom vorherigen Stern wächst per Feder zu ihm hin.
- Das aktuelle Wort ist größer und heller mit leichtem Schein; ältere Sterne und Linien sind gedimmt. Farbe: erste Palettenfarbe (Akzent).
- Zeilenwechsel: das alte Sternbild blendet aus und driftet leicht, das neue baut sich auf. Instrumentale Lücken: pulsierende Punkte.

### 5.4c2 Sternbild V2 (v1.6)
Eigener Lyrics-Stil „Sternbild V2“ (Taste 4); V1 (§5.4c) bleibt unverändert. Gleiche Sterne, Linien, Federn und Farben wie V1, mit diesen Unterschieden:
- **Durchgehende Kette:** Die Wörter werden über Zeilenwechsel hinweg aneinandergereiht; das erste Wort einer Zeile verbindet sich mit dem letzten der vorigen. Bis zu **30 Wörter** (einstellbar 5–40, v1.7) sind gleichzeitig sichtbar; jedes weitere lässt das älteste ganz langsam (über ~6 s) ausblenden. Ältere Linien werden nach hinten hin dunkler.
- **Platzierung:** Jeder neue Stern landet an einem zufälligen freien Punkt des ganzen Bildschirms (Mindestabstand zu allen sichtbaren Sternen, begrenzte Linienlänge). **Kreuzungen der Linien sind erlaubt.**
- **Schlagworte** werden etwa 1,5× größer und fett dargestellt, ihr Stern etwas größer. Erkennung pro Song: Jedes Wort bekommt Punkte nach Häufigkeit im ganzen Song und Wortlänge; Füllwörter (DE/EN-Liste) und Wörter unter 3 Buchstaben zählen nie. Die besten ~15 % der verschiedenen Wörter sind Schlagworte.
- **Instrumentale Lücken:** Das Sternbild bleibt stehen, die Sterne funkeln leicht; danach geht die Kette weiter. Beim Songwechsel blendet die Kette aus. Beim Zurückspulen verschwinden die noch nicht gesungenen Wörter; nach einem großen Sprung nach vorn wird die Kette neu aufgebaut.

### 5.4e Code-Ansicht (v1.6)
Lyrics-Stil „Code“ (Taste 5): Die Lyrics werden wie Code in einem Editor getippt, synchron zum Song. Ruhe-Ansicht, nur mit synchronisierten Lyrics.
- **Editor:** dunkles Editorfenster, etwas eingerückt, damit der Leuchtrahmen sichtbar bleibt. Oben ein Tab mit dem Songtitel als Dateiname (`neon_horizon.rs`), links Zeilennummern, die aktuelle Zeile ist hinterlegt, blinkender Balken-Cursor (beim Tippen durchgehend sichtbar). Monospace-Schrift (Cascadia Mono, sonst Consolas). Syntaxfarben werden aus der Cover-Palette abgeleitet (aufgehellt, damit lesbar).
- **Code-Form:** oben ein Kommentar mit Titel und Interpret und `import { … } from "lyrics";` mit den Schlagworten des Songs (§5.4c2). Jeder Abschnitt ist eine Funktion (`fn verse_1() {`, `fn chorus() {`), jede Zeile `sing("…");`, instrumentale Pausen ein Kommentar `// ♪ instrumental`.
- **Tippen:** Wort für Wort, zeichenweise, ohne Tippfehler. Die Wortzeiten werden wie in §5.4b über die Zeile verteilt. Gerüst (Funktionskopf, `sing("`, `");`, `}`) wird kurz vor bzw. nach den Wörtern schnell getippt.
- **Abschnitte:** LRCLIB hat keine Marker. Ein Block von mindestens 2 Zeilen, der im Song mehrmals vorkommt, ist ein Refrain. Neue Abschnitte beginnen außerdem nach instrumentalen Pausen (über 4 s oder Leerzeile) und beim Wechsel zwischen Wiederholungs- und Strophenzeilen; zu lange Blöcke werden alle 4 Zeilen geteilt. Alles andere sind Strophen (`verse_1`, `verse_2` …).
- **Refrain-Wiederholung:** wird nicht neu getippt. Am Dateiende erscheint ein Aufruf `chorus();`, die Kamera springt zur vorhandenen Funktion und hebt die Zeilen beim Singen Wort für Wort hervor.
- **Kamera:** eng am Cursor (etwa 8 Zeilen sichtbar), folgt per Feder und neigt sich leicht in Tipprichtung. **3D:** pro Abschnitt gewürfelt schaut etwa jeder zweite Abschnitt von der Seite (40°, links oder rechts, manchmal zusätzlich leicht von unten/oben) perspektivisch auf die Code-Ebene; der Winkel wechselt per Feder mit dem Abschnitt. In der Schrägansicht wird nichts hart abgeschnitten: Die Code-Ebene blendet zu den Rändern des Editors weich aus, die ferne Seite wird zusätzlich abgedunkelt. Beim Abschnittswechsel: liegt das Ziel nah, ein schneller Schwenk; liegt es weit weg, zoomt sie heraus (ganze Datei sichtbar), fliegt hin und zoomt wieder ein (~1,5 s).
- Alles hängt an der Songposition: Pause hält das Tippen an, Spulen springt an die passende Stelle.

### 5.4f Ein-Wort-Ansicht (v1.6)
Lyrics-Stil „Ein Wort“ (Taste 6): Ruhe-Ansicht, nur mit synchronisierten Lyrics.
- Immer nur **ein Wort** groß und fett in der Mitte (Satzzeichen am Rand entfernt). Lange Wörter werden verkleinert, bis sie in die Breite passen. Die Wortzeiten werden wie in §5.4b über die Zeile verteilt.
- **Farben:** Jedes Wort bekommt die nächste Farbe aus einer Reihe, der **ganze Bildschirm** die invertierte Farbe (weißes Wort auf Schwarz, schwarzes auf Weiß, Rot auf Cyan …). Der Leuchtrahmen ist dabei verdeckt. Die Reihe besteht aus Weiß und den Cover-Farben; liefert das Cover wenige unterschiedliche Farben, kommen farbtonverschobene Varianten der Hauptfarbe dazu. Mittlere Töne werden aufgehellt oder abgedunkelt, damit Wort und Hintergrund immer deutlich kontrastieren.
- **Farbmodus (v1.7/v1.8):** drei Modi, in den Einstellungen wählbar und mit der **Tab-Taste** in der Ansicht reihum umschaltbar (gespeichert):
  1. **Cover-Farben:** Farbreihe wie oben, Hintergrund invertiert.
  2. **Eigenes Paar:** Farbe A und Farbe B aus den Einstellungen; die Wörter wechseln A auf B, B auf A.
  3. **Zwei Cover-Farben:** die zwei Farben der Cover-Palette mit dem stärksten Kontrast, ebenfalls abwechselnd A auf B, B auf A. Liegen sie zu nah beieinander, wird die hellere aufgehellt und die dunklere abgedunkelt.
- **Wechsel:** harter Schnitt bei jedem Wort, ohne Begrenzung der Wechselrate (Wunsch des Projektinhabers; Hinweis zu Lichtempfindlichkeit im README).
- Die Uhrzeile bleibt oben, in der Wortfarbe. Instrumentale Lücken: pulsierende Punkte in der aktuellen Wortfarbe.

### 5.4d Schallplatte (v1.6)
Abwechslung für Songs **ohne Lyrics** (keine gefunden, Werbung oder Lyrics ausgeschaltet).
- **Wurf:** einmal pro Song 50:50, ob statt des Covers die Schallplatte gezeigt wird. Kein Wechsel mitten im Song; nur wenn sich die Lyrics-Suche zu Songbeginn als erfolglos herausstellt, morpht das Cover einmal weich zur Platte. Keine Einstellung.
- **Optik:** schwarzes Vinyl mit feinen Rillen, zwei Rillen-Pausen und einem stehenden Lichtreflex. Das Albumcover ist rund ausgeschnitten das Label in der Mitte, mit Mittelloch und Ambient-Glow wie beim Cover.
- **Drehung:** 33⅓ U/min (eine Umdrehung in 1,8 s), sichtbar am Label. Bei Pause läuft die Platte über einige Sekunden langsam aus; bei Play läuft sie in unter einer Sekunde wieder an.
- **Player-Ansicht:** Die Platte ersetzt das Cover in gleicher Größe; alles andere bleibt.
- **Ruhe-Ansicht:** Die Platte steht groß in der Mitte (wie das Cover in §5.4), Titel und Interpret stehen links unten in der Ecke. Die Uhrzeile bleibt oben.

### 5.5 Leuchtrahmen
- **Form:** Er folgt dem rechteckigen Bildschirmrand mit leicht abgerundeten Ecken und leuchtet weich nach innen aus (Glow-Falloff, keine harte Linie).
- **Rotation (v1.1, „Komet“):** Ein heller, kompakter **Kopf** (~12 % des Umfangs, weicher Vorlauf) zieht einen weich auslaufenden **Schweif** (~30 % des Umfangs) hinter sich her und **wandert im Uhrzeigersinn um den Monitor**: Start unten links, den linken Rand hoch, oben entlang, rechts hinunter, unten zurück.
  - **Geschwindigkeit:** über die Perimeter-Koordinate gleichmäßig, damit es an den Ecken nicht ruckelt. Beim Start des Modus beschleunigt der Komet per Feder aus dem Stand.
  - **Grundglühen:** Auf dem restlichen Umfang bleibt ein schwaches Glühen (etwa 5 %), damit der Rahmen nie ganz dunkel ist, die Bewegung aber auch bei einfarbigen Covern klar sichtbar bleibt.
  - *Vorher (v1.0):* Bogen über ~60 % mit 15 % Grundglühen; bei einfarbigen Covern war kaum Bewegung zu sehen.
- **Farben:**
  - **Farbmodus Cover:** 2–3 Farben aus dem Cover, als Verlauf vom Kopf zum Schweifende.
  - **Mehrere Kometen (v1.4):** Hat das Cover mehrere **dominante** Farben (je mindestens ~10 % der Fläche und deutlich verschieden), läuft pro Farbe ein eigener Komet in dieser Farbe (2 oder 3), gleichmäßig über den Umfang verteilt; die Schweife sind dann etwas kürzer. Beim Trackwechsel blenden zusätzliche Kometen weich ein bzw. aus und die Abstände gleiten per Feder. Bei nur einer dominanten Farbe und bei fester Palette bleibt es bei einem Kometen mit Farbverlauf.
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

### 6.5 Updates (v1.5)
- **Quelle:** `GET https://api.github.com/repos/CallMeFriedrich/glowframe/releases/latest` (öffentlich, ohne Token, mit `User-Agent`).
- **Vergleich:** Tag `vX.Y.Z` gegen die eigene Version (SemVer); nur neuere Versionen werden angeboten.
- **Wann:** per Knopf „Nach Updates suchen“ in den Einstellungen und einmal still ~10 s nach dem Programmstart; bei neuer Version eine Tray-Benachrichtigung, im Einstellungsfenster „Version X verfügbar – Installieren“.
- **Installieren:**
  - **Installierte Version:** `Glowframe-<v>-setup.exe` herunterladen, SHA-256 gegen den `digest` aus der GitHub-API prüfen, mit `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART` starten, Glowframe beendet sich; der Installer startet es danach wieder.
  - **Portable Version:** `…-portable.zip` herunterladen und prüfen, die neue Exe neben die alte entpacken, die laufende Exe umbenennen (`.old`), neue an ihre Stelle setzen, neu starten; `.old` wird beim nächsten Start gelöscht.
  - Ohne passenden Digest oder bei Fehlern wird nichts installiert, Fehler werden angezeigt und geloggt.

## 7. Einstellungen

**Fenster:** normales Fenster außerhalb des Vollbilds, im gleichen dunklen Stil. Rechts oben ein Sprachumschalter **DE | EN**, der sofort wirkt.

**Übernehmen:** Änderungen gelten **sofort**, auch in einem laufenden Vollbild-Modus (Live-Vorschau) und werden automatisch gespeichert. Es gibt keinen „Speichern“-Button.

| Gruppe | Einstellung | Typ / Werte | Standard |
|---|---|---|---|
| Allgemein | Sprache | DE / EN | Systemsprache (DE wenn Deutsch, sonst EN) |
| Allgemein | Hotkey | Kombination aufnehmen | `Strg+Alt+L` |
| Allgemein | Mit Windows starten (v1.5) | an/aus (Registry `HKCU\…\Run`) | aus |
| Allgemein | Updates (v1.5) | Knopf „Nach Updates suchen“, Anzeige der Version, „Installieren“ | – |
| Hintergrund (v1.5) | Hintergrund | Schwarz / Windows-Hintergrundbild / Eigenes Bild / Durchsichtig | Schwarz |
| Hintergrund | Eigenes Bild | Datei wählen (≥ 2560×1440) | – |
| Hintergrund | Abdunkeln | 0–90 % | 50 % |
| Hintergrund | Taskleiste sichtbar lassen | an/aus | aus |
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
| Lyrics | Stil (v1.3/v1.5, auch per Zifferntaste 1–9) | Klassisch / Nachrichten / Sternbild / Sternbild V2 / Code / Ein Wort | Klassisch |
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
