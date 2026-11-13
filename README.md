<!--
===============================================================================
 Division By Zeros (DBZS) - Projektpapier
 Projekt: DBZS LiveTab Analyzer
 Dokument: README.md
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

# DBZS LiveTab Analyzer - Projektübersicht

## Kurzbeschreibung

Der **DBZS LiveTab Analyzer** ist eine Windows-App, die den laufenden Stereo-Ausgang des Systems analysiert und daraus musikalische Informationen gewinnt:

- Tonart
- Akkorde
- Einzeltöne
- Melodien
- Bassbewegungen
- BPM, Beatgrid und Taktart
- Lyrics mit Zeitstempeln
- Gitarren-Tabs
- Songstruktur
- Vorschläge für nächste Akkorde, Skalen und Zielnoten
- Guitar-Pro-artiges Playback mit Cursor, Loop, Metronom und Lyrics-Highlighting

## Produktziel

Die App soll Musikerinnen und Musiker beim Raushören, Üben, Improvisieren, Komponieren und Dokumentieren von Songs unterstützen.

Der wichtigste Grundsatz:

> Die App muss nicht sofort perfekte Tabs aus jedem fertigen Mix erzeugen. Sie muss zuerst einen transparenten, korrigierbaren und musikalisch sinnvollen Analyse-Workflow liefern.

## Kernprinzip

```text
Windows Audio Input: WASAPI-Systemmix oder ASIO-Interfaceeingang
→ Live Capture
→ Audioanalyse
→ Musiktheorie
→ Songmodell
→ Tab-Erzeugung
→ Lyrics-Sync
→ Playback / Export
```

## Empfohlener Stack

- C# / .NET 8 oder .NET 9
- Avalonia UI
- NAudio / WASAPI Loopback
- Python Analysis Worker mit FastAPI
- librosa, numpy, scipy
- Demucs für Stem-Separation
- faster-whisper für Lyrics
- Basic Pitch und torchcrepe für Pitch/Melodie
- SQLite für interne Zustände
- `.dbztab` als natives Projektformat
- SkiaSharp für Timeline-, Tab- und Fretboard-Rendering
- DryWetMIDI für MIDI-Playback

## MVP-Ziel

Die erste Version soll können:

1. Windows-Stereo-Ausgang abhören.
2. Pegel, Spektrum und Waveform anzeigen.
3. BPM grob erkennen.
4. Akkorde per Chroma/Template erkennen.
5. Tonart schätzen.
6. Chord-Timeline anzeigen.
7. nächste Akkorde vorschlagen.
8. Analyse als Projekt speichern.

## Spätere Kernfeatures

- Offline Deep Analysis
- Stem-Separation
- Lyrics-Transkription
- Word-Level-Timestamps
- Gitarren-Tab-Generator
- Guitar-Pro-artiges Playback
- PDF, MIDI, MusicXML, TXT, LRC, SRT Export
