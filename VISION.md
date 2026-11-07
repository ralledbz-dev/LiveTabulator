<!--
===============================================================================
 Division By Zeros (DBZS) - Projektpapier
 Projekt: DBZS LiveTab Analyzer
 Dokument: VISION.md
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

# Produktvision

## Vision

Der DBZS LiveTab Analyzer soll ein intelligenter Musik-Assistent werden, der aus laufender oder geladener Musik ein strukturiertes, editierbares und abspielbares Musikprojekt erzeugt.

## Leitsatz

> Aus Audio wird musikalisches Wissen.

## Zielgruppen

| Zielgruppe | Nutzen |
|---|---|
| Gitarristen | Songs raushören, Tabs erzeugen, üben |
| Produzenten | Akkordfolgen und Referenztracks analysieren |
| Songwriter | neue Harmonien und Melodien ableiten |
| Lernende | Musiktheorie am echten Song verstehen |
| Live-Musiker | Tonart, Skalen und Akkorde während des Hörens sehen |

## Produktversprechen

Die App erkennt nicht nur Daten, sondern bereitet sie musikalisch auf:

- „Dieser Akkord ist wahrscheinlich Em.“
- „Die Tonart ist vermutlich E-Moll.“
- „Die Progression entspricht i - VI - III - VII.“
- „Als nächster Akkord wäre B7 spannend, weil er nach Em zurückführt.“
- „Diese Melodie kann auf der Gitarre in Lage 5 gespielt werden.“
- „Dieses Wort liegt auf Takt 3, Beat 2.5.“

## Arbeitsmodus

Die App hat zwei Analyseebenen:

### Live-Modus

Schnelle, leicht unscharfe Echtzeitanalyse:

- aktueller Akkord
- Tonart-Schätzung
- BPM
- Skalen
- nächste Akkorde
- einfache Fretboard-Hilfen

### Deep-Analyse-Modus

Langsamere, genauere Analyse als Job:

- Stems trennen
- Vocals isolieren
- Lyrics transkribieren
- Melodie extrahieren
- Tabs erzeugen
- Korrektur ermöglichen
- Export vorbereiten
