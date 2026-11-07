<!--
===============================================================================
 Division By Zeros (DBZS) - Projektpapier
 Projekt: DBZS LiveTab Analyzer
 Dokument: UI_CONCEPT.md
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

# UI Konzept

## Ziel

Die UI soll als dunkle, modulare Musik-Analyse-Workstation im DBZS Neon Style aufgebaut werden.

## Hauptbereiche

- Top Navigation
- Audio Input Panel
- Song Overview
- Waveform
- Chord Timeline
- Tab Editor
- Lyrics Panel
- Lyrics Timeline
- Fretboard View
- Scales & Modes
- Playback Controls
- Tracks/Mixer
- Dev Observer
- Project Panel

## Referenzbild

Das ZIP enthält unter `assets/ui-concept.png` ein generiertes UI-Konzeptbild.

## UI-Grundsätze

- dunkler technischer Look
- Neon-Cyan, Neon-Blau, Violett als Akzente
- keine überladene Startansicht
- Panels einklappbar
- Splitter zwischen Hauptbereichen
- Live-Status immer sichtbar
- Confidence-Werte klar darstellen
- Dev Observer als eigenes Panel
- Tabs und Timeline müssen Vorrang haben

## Wichtig

Die UI darf keine Businesslogik enthalten. Jede Aktion ruft Commands oder Controller-Methoden auf.
