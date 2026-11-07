<!--
===============================================================================
 Division By Zeros (DBZS) - Projektpapier
 Projekt: DBZS LiveTab Analyzer
 Dokument: OBSERVABILITY.md
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

# Observability und Dev Observer

## Ziel

Die App darf keine Blackbox sein. Jeder relevante Prozess muss sichtbar, loggbar und nachvollziehbar sein.

## Logging-Kategorien

```text
Audio
DSP
BeatTracking
ChordDetection
KeyDetection
StemSeparation
Lyrics
Pitch
TabGeneration
Prediction
Playback
Export
UI
Jobs
Settings
Models
Errors
```

## Dev Observer Panel

Das Dev Observer Panel zeigt:

- aktuelle Audioquelle
- Worker-Status
- geladene Modelle
- aktive Jobs
- letzte Fehler
- Chord/Key Confidence
- Beatgrid-Status
- CPU/RAM/GPU-Auslastung
- Logstream
- Diagnoseexport

## Beispiel-Logs

```text
[Audio] WASAPI Loopback gestartet: Lautsprecher Realtek, 48 kHz, Stereo
[ChordDetection] Kandidat: Em, Confidence=0.82
[KeyDetection] Tonart: E minor, Probability=0.76
[Lyrics] Wort aligned: "fire", Bar=2, Beat=3.25
[TabGeneration] Note E4 → Saite=1, Bund=0
[Prediction] Nächste Akkorde: C, Am, B7
```

## Fehlerbehandlung

Ein Fehler muss enthalten:

- Kategorie
- technische Nachricht
- Nutzer-Nachricht
- Kontextdaten
- Stacktrace im Debug-Modus
- Lösungshinweis, falls möglich

## Diagnosepaket

Die App soll ein Diagnosepaket exportieren können:

```text
diagnostics.zip
├─ logs/
├─ settings.json
├─ worker-health.json
├─ model-index.json
├─ recent-errors.json
└─ system-info.json
```

## Pflicht für Entwickler

Jede neue Pipeline muss Observability-Hooks enthalten:

- Start
- Fortschritt
- Ergebnis
- Fehler
- Dauer
- relevante Parameter
