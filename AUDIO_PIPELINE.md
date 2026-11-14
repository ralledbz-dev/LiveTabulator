<!--
===============================================================================
 Division By Zeros (DBZS) - Projektpapier
 Projekt: DBZS LiveTab Analyzer
 Dokument: AUDIO_PIPELINE.md
 Version: 0.1.0
 Stand: 2026-05-23
 Status: Konzept / Entwicklungsgrundlage
 Autor: Division By Zeros (DBZS) / Ralle
 Sprache: Deutsch
===============================================================================

Zweck:
Dieses Dokument beschreibt einen Teilbereich des Projekts „DBZS LiveTab Analyzer“.
Alle technischen Beschreibungen, Funktionen, Methoden, Prozeduren und Workflows
sind so zu dokumentieren, dass sie für Entwickler, KI-Agenten und spätere
Wartung nachvollziehbar bleiben.

Dokumentationspflicht:
- Jede Sourcecode-Datei erhält einen DBZS-Header.
- Jede öffentliche Klasse, Funktion, Methode, Prozedur und Pipeline wird auf Deutsch dokumentiert.
- Jede Funktion beschreibt Zweck, Eingaben, Ausgaben, Fehlerfälle und wichtige Abhängigkeiten.
- UI-Code enthält keine Businesslogik.
- Fachlogik liegt in Core-, Service-, Engine- oder Agent-Modulen.
===============================================================================
-->

# Audio Pipeline

## Ziel

Die Audio Pipeline führt lokale Eingänge zu stabilen Analyseframes zusammen. Unter Windows bleiben die Quellen getrennt: WASAPI-Loopback nimmt den Systemmix eines Render-Endpunkts auf, ASIO liest Eingänge eines registrierten Audiointerfaces. ASIO ist kein Ersatz für Systemton-Loopback. Android/iOS verwenden eine freigegebene Mikrofoneingabe.

## Kernpipeline

```text
WASAPI Systemmix oder ASIO-Interfaceeingang
→ AudioCaptureService
→ AudioRingBuffer
→ FrameSplitter
→ AudioPreprocessor
→ LiveAnalysisQueue
→ Recorder optional
```

## AudioCaptureService

### Zweck

Startet und stoppt die jeweils gewählte lokale Audioquelle. Unter Windows wird WASAPI-Loopback für den Systemmix oder ASIO für Interface-Eingänge verwendet.

### Verantwortlichkeiten

- WASAPI-Render-Endpunkte und ASIO-Treiber getrennt auflisten
- den gewählten Backendpfad initialisieren (WASAPI-Loopback oder ASIO-Interfaceeingang)
- Sample Rate und Kanalzahl erfassen
- Pegelwerte berechnen
- Frames an den Ringbuffer senden
- Fehler im Dev Observer melden

### Muss dokumentiert werden

Jede Methode beschreibt:

- welches Gerät genutzt wird,
- welche Sample Rate erwartet wird,
- welche Buffergröße verwendet wird,
- wie Fehler behandelt werden.

## AudioRingBuffer

### Zweck

Hält laufende Audioframes für Analyse und Anzeige bereit.

### Wichtige Regeln

- Der Buffer darf die UI nicht blockieren.
- Alte Frames dürfen überschrieben werden, wenn Analyse zu langsam ist.
- Deep-Analyse nutzt aufgezeichnete Daten oder Dateien, nicht den Live-Ringbuffer.

## Analysefenster

| Analyse | Fenstergröße |
|---|---|
| Pegel | 10-30 ms |
| Spektrum | 20-50 ms |
| Pitch | 50-100 ms |
| Akkorde | 0.5-2 s |
| Tonart | 5-30 s |
| BPM | 5-20 s |

## AudioPreprocessor

### Aufgaben

- Stereo zu Mono für Analyse ableiten
- Normalisierung
- DC Offset entfernen
- optional Hochpass/Tiefpass
- Frequenzbereiche für Bass/Harmonie/Melodie vorbereiten

## Events

- `AudioDeviceSelectedEvent`
- `AudioCaptureStartedEvent`
- `AudioCaptureStoppedEvent`
- `AudioFrameAvailableEvent`
- `AudioOverrunDetectedEvent`
- `AudioCaptureErrorEvent`

## WAV-/MP3-Datei- und Aufnahmeweg

- WAV bleibt unverändert unterstützt. `.mp3` wird lokal mit Symphonia 0.5.5 dekodiert und als dieselbe `WaveFile`-PCM-Struktur an Projektmodell, Deep-Analyse, Waveform und nativen Playback-Ausgang übergeben. Dateiendungen werden ohne Beachtung der Groß-/Kleinschreibung ausgewertet; MP3-Quelldateien und dekodierte Ausgaben sind jeweils auf 2 GiB begrenzt.
- Die Live-PCM-Aufnahme bleibt auf zehn Minuten begrenzt. Beim Speichern entscheidet die echte `.wav`- oder `.mp3`-Endung: WAV nutzt den bestehenden PCM-16-Bit-Writer; MP3 wird offline nach Ende der Aufnahme mit dem gebündelten LAME-Encoder als 192-kbit/s-CBR-Mono kodiert. Nicht unterstützte Encoder-Sampleraten führen zu einem sichtbaren Fehler und werden nicht als MP3/WAV falsch beschriftet.
- `.dbztab` bettet neue WAV- und MP3-Quellen unter ihrer passenden Endung ein; der Loader liest weiterhin ältere Archive mit `audio/source.wav`.
- Die Audiokarte zeigt Capturefortschritt aus tatsächlich gepufferten Samples relativ zum Zehn-Minuten-Limit. WAV-Import meldet gelesene Datei-Bytes und konvertierte PCM-Samples; MP3-Import meldet dekodierte Paketbytes. MP3-Aufnahme meldet LAME-Sampleblöcke und tatsächlich geschriebene Ausgabebytes; WAV meldet geschriebene PCM-Blöcke. Updates werden auf mindestens 0,5-Prozent-Schritte gedrosselt; nicht gemessene Analysejobs erhalten keinen Audioprozentbalken.
- Implementierungsquellen: [Symphonia 0.5.5 – MP3-Demuxer/Decoder und gapless Option](https://docs.rs/symphonia/0.5.5/symphonia/), [mp3lame-encoder 0.3.0 – LAME-Aufnahmecodierung](https://docs.rs/mp3lame-encoder/0.3.0/mp3lame_encoder/), [mp3lame-sys 0.2.0 – gebündeltes statisches LAME 4.0](https://crates.io/crates/mp3lame-sys/0.2.0).
- Lizenzhinweis für spätere Binärveröffentlichungen: LAME und `mp3lame-encoder` stehen unter LGPL-3.0. Eine ausgelieferte statisch gelinkte App muss die einschlägigen Lizenz-/Quellhinweise und die LGPL-Anforderungen an das Relinking mitliefern; diese Änderung erstellt oder veröffentlicht kein Installationspaket.
- Validierung (Windows x64/MSVC 19.51, Rust/Cargo 1.95; Artefakte unter `.work/mp3-validation-2026-11-14`):
  - `cargo +1.95-x86_64-pc-windows-msvc fmt --all -- --check` – bestanden.
  - `cargo +1.95-x86_64-pc-windows-msvc test --locked -p livetab-core` – 58 bestanden, 0 fehlgeschlagen.
  - `cargo +1.95-x86_64-pc-windows-msvc test --locked -p livetab-app audio_` – 6 bestanden, 10 gefiltert; enthält WAV-/MP3-Roundtrips, Encoder-/Decoderfortschritt und einen synthetischen Capture-Snapshottest.
  - `cargo +1.95-x86_64-pc-windows-msvc check --locked -p livetab-app` – erfolgreich, einschließlich Slint-Codegenerierung; zwei nicht-fatale `dead_code`-Warnungen für `CaptureSession::is_recording` und `PlaybackOutputSession::position_seconds`.
  - `cargo +1.95-x86_64-pc-windows-msvc build --locked -p livetab-app` – erfolgreich gelinkt; dieselben zwei Warnungen.
  - Synthetische Tests starteten weder die App noch Capture oder Playback.
