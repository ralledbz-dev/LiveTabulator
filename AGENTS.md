<!--
===============================================================================
 Division By Zeros (DBZS) - Projektpapier
 Projekt: DBZS LiveTab Analyzer
 Dokument: AGENTS.md
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

# Agenten und Workflows

## Ziel

Die App nutzt fachlich getrennte Agents, die Analyse, Korrektur, Export und Playback kontrolliert durchführen.

## Agenten

| Agent | Aufgabe |
|---|---|
| AudioCaptureAgent | überwacht Audioeingang und Pegel |
| BeatAgent | erkennt BPM, Beats, Downbeats |
| ChordAgent | erkennt Akkorde |
| KeyAgent | erkennt Tonart und Skala |
| MelodyAgent | extrahiert Melodie/Pitch |
| LyricsAgent | transkribiert und aligned Lyrics |
| TabAgent | erzeugt Gitarren-Tabs |
| PredictionAgent | berechnet nächste Akkorde/Skalen |
| PlaybackAgent | steuert Wiedergabe |
| ExportAgent | erzeugt Exportdateien |
| CorrectionAgent | verarbeitet Nutzerkorrekturen |
| DevObserverAgent | sammelt Diagnoseinformationen |

## Workflow: Full Song Rebuild

```text
Import Audio
→ BeatAgent
→ ChordAgent
→ KeyAgent
→ LyricsAgent
→ MelodyAgent
→ TabAgent
→ PredictionAgent
→ ExportAgent
```

## Agent-Regeln

- Jeder Agent hat klar definierte Eingaben und Ausgaben.
- Jeder Agent schreibt strukturierte Logs.
- Jeder Agent meldet Fortschritt.
- Jeder Agent kann Fehler verständlich zurückgeben.
- Agents greifen nicht direkt auf UI-Komponenten zu.
- Agents arbeiten mit Core-Modellen und Events.

## Beispiel: LyricsAgent

### Zweck

Erzeugt Lyrics mit Wort-Zeitstempeln und Taktzuordnung.

### Eingaben

- Audioquelle
- optional Vocals-Stem
- Beatgrid
- Spracheinstellung
- Modellprofil

### Ausgaben

- LyricsWords
- LyricsLines
- Confidence-Werte
- unsichere Stellen
- Events für UI-Aktualisierung

## Beispiel: TabAgent

### Zweck

Erzeugt aus NoteEvents und ChordEvents spielbare TabTracks.

### Eingaben

- SongModel
- GuitarTuning
- TabMode
- Schwierigkeitsgrad
- bevorzugte Lage

### Ausgaben

- TabTrack
- TabBars
- TabNotes
- Mapping-Log
- Warnungen bei unsicheren Stellen
