<!--
===============================================================================
 Division By Zeros (DBZS) - Projektpapier
 Projekt: DBZS LiveTab Analyzer
 Dokument: DATA_MODEL.md
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

# Datenmodell

## Ziel

Das Datenmodell hält alle musikalischen, technischen und editorischen Informationen eines Songs.

## Projektformat

Natives Projektformat:

```text
.dbztab
```

Technisch empfohlen:

```text
ZIP Container
├─ project.json
├─ audio/
├─ stems/
├─ analysis/
├─ exports/
└─ logs/
```

## Project

```json
{
  "version": "0.1.0",
  "title": "Unknown Song",
  "artist": "Unknown Artist",
  "bpm": 120,
  "timeSignature": "4/4",
  "key": "E minor"
}
```

## ChordEvent

```json
{
  "name": "Em",
  "root": "E",
  "quality": "minor",
  "notes": ["E", "G", "B"],
  "start": 0.0,
  "end": 2.0,
  "bar": 1,
  "beat": 1.0,
  "confidence": 0.88
}
```

## NoteEvent

```json
{
  "pitch": "E4",
  "midi": 64,
  "start": 1.25,
  "duration": 0.5,
  "velocity": 92,
  "confidence": 0.74
}
```

## TabNote

```json
{
  "string": 1,
  "fret": 0,
  "start": 1.25,
  "duration": 0.5,
  "note": "E4",
  "technique": null
}
```

## LyricsWord

```json
{
  "text": "fire",
  "start": 2.13,
  "end": 2.52,
  "bar": 1,
  "beat": 4.25,
  "confidence": 0.84
}
```

## AnalysisJob

```json
{
  "jobId": "lyrics_001",
  "type": "lyrics_transcription",
  "state": "running",
  "progress": 0.62,
  "message": "Aligning words to beat grid",
  "errors": []
}
```

## SQLite Tabellen

Empfohlen:

```text
projects
audio_sources
analysis_jobs
beat_events
bar_events
chord_events
note_events
lyrics_words
lyrics_lines
tab_tracks
tab_bars
tab_notes
exports
settings
logs
models
```
