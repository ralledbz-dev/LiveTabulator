<!--
===============================================================================
 Division By Zeros (DBZS) - Projektpapier
 Projekt: DBZS LiveTab Analyzer
 Dokument: TAB_ENGINE.md
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

# Tab Engine

## Ziel

Die Tab Engine erzeugt aus Noten, Akkorden, Timing und Gitarrenstimmung spielbare Gitarren-Tabs.

## Kernmodell

```text
SongModel
→ ChordEvents
→ NoteEvents
→ BeatGrid
→ GuitarTuning
→ FingeringSolver
→ TabTrack
→ TabBars
→ TabNotes
```

## TabTrack

Ein TabTrack repräsentiert eine Gitarrenspur.

Eigenschaften:

- Name
- Stimmung
- Capo
- Instrumenttyp
- Bars
- Noten
- Techniken
- Playback-Parameter

## GuitarTuning

Standard:

```text
E2 A2 D3 G3 B3 E4
```

Später:

- Drop D
- D Standard
- Open G
- 7-Saiter
- Bass

## FingeringSolver

### Zweck

Entscheidet, auf welcher Saite und welchem Bund eine Note gespielt wird.

### Bewertungskriterien

- Lage
- Saitensprung
- Bundsprung
- Geschwindigkeit
- offene Saiten
- Akkordgriff-Kompatibilität
- Anfängerfreundlichkeit
- Nähe zur vorherigen Note

### Kostenmodell

```text
Kosten =
Lagenwechsel
+ Saitensprung
+ Stretch
+ Tempo-Strafe
+ Stil-Strafe
- Bonus für offene Saiten
- Bonus für bekannte Griffbilder
```

## Tab-Modi

| Modus | Ziel |
|---|---|
| Easy | einfache spielbare Tabs |
| Original-ish | möglichst nah am Original |
| Lead | Hauptmelodie |
| Rhythm | Akkordbegleitung |
| Powerchord | Rock/Metal |
| Bass | Basslinie |

## Techniken

Später zu unterstützen:

- Hammer-On
- Pull-Off
- Slide
- Bend
- Vibrato
- Palm Mute
- Let Ring
- Dead Notes
- Harmonics

## Dokumentationspflicht

Jede Methode der Tab Engine muss erklären:

- welche musikalische Entscheidung getroffen wird,
- welche Eingaben genutzt werden,
- welche Annahmen gelten,
- wie Mehrdeutigkeit aufgelöst wird.
