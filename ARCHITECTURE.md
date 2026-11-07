<!--
===============================================================================
 Division By Zeros (DBZS) - Projektpapier
 Projekt: DBZS LiveTab Analyzer
 Dokument: ARCHITECTURE.md
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

# Systemarchitektur

## Architekturgrundsatz

Die App wird modular aufgebaut. Die UI enthält keine Fachlogik. Alle Entscheidungen laufen über Controller, Services, Engines oder Agents.

```text
UI
→ AppController
→ EventBus
→ Services / Engines / Agents
→ SongModel / ProjectStore
→ Rendering / Playback / Export
```

## Hauptmodule

```text
DBZS.LiveTab.App
DBZS.LiveTab.Core
DBZS.LiveTab.Audio
DBZS.LiveTab.AnalysisClient
DBZS.LiveTab.Playback
DBZS.LiveTab.Rendering
DBZS.LiveTab.Infrastructure
DBZS.LiveTab.Observability
analysis-worker
```

## Schichten

| Schicht | Verantwortung |
|---|---|
| UI Layer | Darstellung, Nutzerinteraktion, keine Businesslogik |
| Controller Layer | Orchestrierung von Aktionen |
| Core Layer | Songmodell, Tabmodell, Lyricsmodell, Musiktheorie |
| Audio Layer | WASAPI Capture, Buffer, Recorder |
| Analysis Layer | Kommunikation mit Python Worker |
| Engine Layer | Akkorde, Tonart, Tabs, Playback |
| Infrastructure | SQLite, Dateien, Einstellungen |
| Observability | Logs, Metriken, Dev Observer |
| Worker | DSP, ML, Stem-Separation, Lyrics |

## Datenfluss

```text
WASAPI Loopback
→ AudioRingBuffer
→ AnalysisFrame
→ LiveAnalysisService
→ MusicalEventStream
→ SongModel
→ TimelineRenderer
→ TabRenderer
→ PlaybackEngine
```

## AppController

Der AppController nimmt UI-Kommandos entgegen und leitet sie weiter.

Beispiele:

- `StartLiveAnalysisCommand`
- `StopLiveAnalysisCommand`
- `ImportAudioFileCommand`
- `RunLyricsJobCommand`
- `GenerateTabsCommand`
- `StartPlaybackCommand`
- `ExportProjectCommand`

## EventBus

Der EventBus verteilt Zustandsänderungen.

Beispiele:

- `AudioCaptureStartedEvent`
- `ChordDetectedEvent`
- `KeyChangedEvent`
- `BeatGridUpdatedEvent`
- `LyricsWordAlignedEvent`
- `TabGeneratedEvent`
- `PlaybackPositionChangedEvent`
- `AnalysisJobFailedEvent`

## Fehlerstrategie

Jeder Fehler wird:

1. technisch geloggt,
2. im Dev Observer angezeigt,
3. dem Nutzer verständlich erklärt,
4. mit Kontextdaten versehen,
5. optional als Diagnosepaket exportierbar gemacht.
