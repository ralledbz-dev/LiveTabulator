<!--
===============================================================================
 Division By Zeros (DBZS) - Projektpapier
 Projekt: DBZS LiveTab Analyzer
 Dokument: DEV_STACK.md
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

# Dev Stack

## Empfohlener Hauptstack

| Bereich | Technologie |
|---|---|
| Desktop App | C# / .NET 8 oder 9 |
| UI | Avalonia UI |
| Audio Capture | NAudio / WASAPI Loopback |
| Rendering | SkiaSharp |
| Playback | DryWetMIDI |
| Datenhaltung | SQLite + JSON |
| Analyse Worker | Python 3.11/3.12 + FastAPI |
| DSP | numpy, scipy, librosa |
| Lyrics | faster-whisper |
| Stem Separation | Demucs |
| Pitch/Melodie | Basic Pitch, torchcrepe |
| Packaging | portable ZIP, später MSIX/Inno Setup |

## Warum dieser Stack?

C# übernimmt:

- UI
- Windows-Audio
- Projektmodell
- Playback
- Rendering
- Settings
- Persistenz
- Dev Observer

Python übernimmt:

- DSP-Analyse
- ML-Modelle
- Stem-Separation
- Lyrics-Transkription
- Deep-Analyse-Jobs

## Kommunikationsmodell

MVP:

```text
C# App
→ HTTP REST
→ Python FastAPI Worker
→ JSON-Ergebnis
→ C# SongModel
```

Später:

```text
C# App
→ gRPC / MessagePack / Shared Memory
→ Python oder Native Engine
```

## Lokale Modellstruktur

```text
D:\Models\DBZS-LiveTab\
├─ asr\
├─ stem-separation\
├─ pitch\
├─ chords\
├─ beat\
└─ llm\
```

## Build-Ziele

- `Debug`: Entwicklerbetrieb mit Dev Observer
- `Release`: stabile App mit Logs
- `Portable`: ZIP-Paket mit Worker-Launcher
- `Installer`: später MSIX/Inno Setup

## Mindestanforderung

Die App muss auf einem Windows-System mit CPU-first Analyse laufen. GPU-Beschleunigung ist optional.
