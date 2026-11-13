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
