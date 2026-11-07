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

Die Audio Pipeline nimmt den Windows-Stereo-Ausgang auf, erzeugt stabile Analyseframes und stellt diese Live-Analyse, Deep-Analyse und Aufnahmefunktion bereit.

## Kernpipeline

```text
WASAPI Loopback
→ AudioCaptureService
→ AudioRingBuffer
→ FrameSplitter
→ AudioPreprocessor
→ LiveAnalysisQueue
→ Recorder optional
```

## AudioCaptureService

### Zweck

Startet und stoppt die Aufnahme des Windows-Systemausgangs.

### Verantwortlichkeiten

- Audioausgabegeräte auflisten
- WASAPI Loopback initialisieren
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
