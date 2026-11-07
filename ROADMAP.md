<!--
===============================================================================
 Division By Zeros (DBZS) - Projektpapier
 Projekt: DBZS LiveTab Analyzer
 Dokument: ROADMAP.md
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

# Roadmap

## Phase 0 - Projektbasis

- Repo-Struktur anlegen
- DBZS Header-Regeln einbauen
- Coding Guidelines einführen
- Dev Observer Grundmodul anlegen
- Settings-System vorbereiten

## Phase 1 - Audio Capture MVP

Ziele:

- WASAPI Loopback Capture
- Audio Device Selector
- Pegelanzeige
- Waveform
- Spektrum
- WAV Recording

Akzeptanz:

- App kann Windows-Ausgang aufnehmen.
- Pegel reagiert live.
- Aufnahme kann als WAV gespeichert werden.

## Phase 2 - Live Chord/Key MVP

Ziele:

- Chroma Feature Extraction
- einfache Akkord-Templates
- Chord Timeline
- Tonart-Schätzung
- Confidence-Anzeige

Akzeptanz:

- App erkennt einfache Pop/Rock-Akkordfolgen grob.
- Akkorde erscheinen in Timeline.
- Confidence wird angezeigt.

## Phase 3 - Beatgrid

Ziele:

- BPM-Erkennung
- Beat-Liste
- Taktlinien
- Quantisierung

Akzeptanz:

- Timeline kann Takte anzeigen.
- Playback-Cursor kann auf Beatgrid laufen.

## Phase 4 - SongModel und Projektdatei

Ziele:

- SongModel
- `.dbztab` Container
- SQLite Cache
- Save/Load

Akzeptanz:

- Analyse kann gespeichert und geladen werden.
- Projektdatei enthält Akkorde, Beats und Settings.

## Phase 5 - Lyrics Job

Ziele:

- Demucs optional
- faster-whisper
- Word-Level-Timestamps
- Mapping auf Takte/Beats
- Lyrics Editor

Akzeptanz:

- Lyrics werden erkannt.
- Wörter haben Zeitstempel.
- Text erscheint an richtiger Stelle in Timeline/Tab.

## Phase 6 - Tab Engine

Ziele:

- Fretboard Mapping
- Fingering Solver
- Easy Tabs
- Chord Tabs
- Lead Tabs

Akzeptanz:

- erkannte Noten/Akkorde werden in Gitarren-Tabs umgewandelt.
- Nutzer kann Stimmung und Modus wählen.

## Phase 7 - Playback

Ziele:

- MIDI Playback
- Cursor
- Loop
- Metronom
- Speed Trainer
- Lyrics Highlighting

Akzeptanz:

- Tab läuft im Takt mit.
- Lyrics werden passend markiert.
- Loopbereich funktioniert.

## Phase 8 - Export

Ziele:

- TXT Tab
- PDF
- MIDI
- MusicXML
- LRC
- SRT
- JSON

Akzeptanz:

- Projekt kann in mindestens TXT, MIDI und LRC exportieren.

## Phase 9 - Prediction / Theory Assistant

Ziele:

- Stufenanalyse
- nächste Akkorde
- Skalenempfehlungen
- Erklärungstexte
- LLM optional

Akzeptanz:

- App zeigt musikalisch sinnvolle nächste Akkorde mit Begründung.
