<!--
===============================================================================
 Division By Zeros (DBZS) - Projektpapier
 Projekt: DBZS LiveTab Analyzer
 Dokument: LYRICS_PIPELINE.md
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

# Lyrics Pipeline

## Ziel

Die Lyrics Pipeline transkribiert Liedtext, erzeugt Wort-Zeitstempel und ordnet Wörter den richtigen Takten und Beats zu.

## Pipeline

```text
Audio
→ Stem Separation
→ Vocals Stem
→ Speech-To-Text
→ Word Timestamp Alignment
→ BeatGrid Mapping
→ LyricsModel
→ Tab/Lyrics Renderer
```

## LyricsTranscriptionJob

### Zweck

Führt die Lyrics-Erkennung als separaten Job aus.

### Warum als Job?

Lyrics-Erkennung ist rechenintensiv und soll den Live-Modus nicht blockieren.

## Schritte

1. Prüfen, ob ein Vocals-Stem existiert.
2. Falls nicht, Stem-Separation starten.
3. Sprache erkennen oder aus Settings lesen.
4. STT-Modell ausführen.
5. Wort-Zeitstempel erzeugen.
6. Wörter auf Beatgrid mappen.
7. Lyrics-Zeilen bilden.
8. Unsichere Wörter markieren.
9. Ergebnis im SongModel speichern.

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

## LyricsLine

```json
{
  "text": "I walk through the fire again",
  "start": 0.42,
  "end": 3.80,
  "startBar": 1,
  "endBar": 2
}
```

## Korrektur-Workflow

Die UI muss ermöglichen:

- Text bearbeiten
- Wörter verschieben
- Zeilen trennen/zusammenführen
- unklare Wörter markieren
- Re-Align ausführen
- Sprache ändern
- Confidence anzeigen

## Export

Lyrics können exportiert werden als:

- `.lrc`
- `.srt`
- `.txt`
- `.pdf`
- Teil der `.dbztab` Datei

## Rechtlicher Hinweis

Die App ist als lokales Analyse- und Lernwerkzeug konzipiert. Beim Export vollständiger Songtexte aus geschützten Songs soll ein Warnhinweis angezeigt werden.
