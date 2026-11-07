<!--
===============================================================================
 Division By Zeros (DBZS) - Projektpapier
 Projekt: DBZS LiveTab Analyzer
 Dokument: PLAYBACK_ENGINE.md
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

# Playback Engine

## Ziel

Die Playback Engine spielt erkannte oder erzeugte Tabs wie ein einfacher Guitar-Pro-Player ab.

## Funktionen

- Play
- Pause
- Stop
- Loop
- Count-In
- Metronom
- Tempoänderung
- Speed Trainer
- Cursor-Synchronisation
- Lyrics-Highlighting
- Spur-Mixer

## Pipeline

```text
SongModel
→ PlaybackTimeline
→ Scheduler
→ MIDI Events
→ Audio/MIDI Output
→ Cursor Events
→ UI Update
```

## PlaybackScheduler

### Zweck

Plant musikalische Ereignisse zeitgenau.

### Muss leisten

- Events nach Zeit sortieren
- Tempoänderungen beachten
- Loopbereiche beachten
- Cursorposition berechnen
- UI nicht blockieren
- Latenz melden

## Cursor Sync

Der Cursor muss synchron anzeigen:

- aktuelle Zeit
- aktueller Takt
- aktueller Beat
- aktuelle Tab-Position
- aktuelles Lyrics-Wort

## Metronom

Eigenschaften:

- Count-In
- Betonung auf Zählzeit 1
- verschiedene Taktarten
- Lautstärke
- optional Soundauswahl

## Mixer

Spuren:

- Original
- Drums
- Bass
- Other
- Vocals
- Generated Guitar
- Metronome

## Events

- `PlaybackStartedEvent`
- `PlaybackPausedEvent`
- `PlaybackStoppedEvent`
- `PlaybackPositionChangedEvent`
- `LoopRegionChangedEvent`
- `LyricsWordHighlightedEvent`
