<!--
===============================================================================
 Division By Zeros (DBZS) - Projektpapier
 Projekt: DBZS LiveTab Analyzer
 Dokument: ANALYSIS_PIPELINE.md
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

# Analyse Pipeline

## Ziel

Die Analyse Pipeline wandelt Audioframes in musikalische Ereignisse um.

## Pipeline

```text
AudioFrame
→ Feature Extraction
→ Chroma
→ Pitch Candidates
→ Onset Detection
→ Beat Tracking
→ Chord Detection
→ Key Detection
→ Event Smoothing
→ MusicalEventStream
```

## Live-Analyse

Live-Analyse muss schnell sein. Sie arbeitet mit:

- Chroma-Features
- einfachen Akkord-Templates
- Beat-Schätzung
- gleitender Tonart-Wahrscheinlichkeit
- Confidence-Werten

## Deep-Analyse

Deep-Analyse läuft als Job:

```text
Audio File
→ Normalize
→ Stem Separation optional
→ Beatgrid
→ Chord Pass 1
→ Chord Pass 2
→ Key/Scale Analysis
→ Melody/Pitch Extraction
→ SongModel Merge
```

## ChordDetectionService

### Zweck

Erkennt Akkorde aus Chroma-Features.

### Eingaben

- Audioframe oder Chroma-Vektor
- aktuelle Tonart-Wahrscheinlichkeit
- vorherige Akkorde
- Zeitposition

### Ausgaben

- Akkordname
- Root
- Qualität
- erkannte Töne
- Startzeit
- Endzeit
- Confidence

### Fehlerfälle

- zu leises Signal
- keine klare Harmonie
- Percussion dominiert
- Mehrdeutigkeit zwischen Dur/Moll/Powerchord

## KeyDetectionService

### Zweck

Ermittelt die wahrscheinlichste Tonart über ein Zeitfenster.

### Ausgaben

- Tonart
- Modus
- Confidence
- alternative Tonarten
- Skalentöne

## BeatTrackingService

### Zweck

Erkennt BPM, Beats und Takte.

### Muss liefern

- BPM
- Beat-Liste
- Downbeat-Liste
- Taktpositionen
- Taktart-Schätzung
- Confidence
