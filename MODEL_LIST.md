<!--
===============================================================================
 Division By Zeros (DBZS) - Projektpapier
 Projekt: DBZS LiveTab Analyzer
 Dokument: MODEL_LIST.md
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

# Modellliste

## Pflichtmodelle für MVP und Deep Analyse

## ASR / Lyrics

Empfohlen:

```text
D:\Models\DBZS-LiveTab\asr\
├─ faster-whisper-small
├─ faster-whisper-medium
└─ faster-distil-whisper-large-v3
```

Nutzung:

- Fast Mode: small
- Balanced Mode: medium
- Quality Mode: distil-large-v3

## Stem Separation

```text
D:\Models\DBZS-LiveTab\stem-separation\
├─ htdemucs
└─ htdemucs_ft
```

Nutzung:

- Vocals für Lyrics
- Bass für Root Notes
- Drums für Beatgrid
- Other für Akkorde

## Pitch / Melody

```text
D:\Models\DBZS-LiveTab\pitch\
├─ basic-pitch
├─ torchcrepe-tiny
└─ torchcrepe-full
```

Nutzung:

- Basic Pitch: Audio-to-MIDI
- torchcrepe tiny: schneller Pitch
- torchcrepe full: genaue Offline-Pitchanalyse

## Chords

```text
D:\Models\DBZS-LiveTab\chords\
├─ chordino
├─ qm-vamp-plugins
└─ btc-experimental
```

Nutzung:

- MVP: eigener Chroma/Template-Algorithmus
- Vergleich: Chordino
- Experiment: BTC

## Beat

```text
D:\Models\DBZS-LiveTab\beat\
└─ madmom-optional
```

## LLM optional

```text
D:\Models\DBZS-LiveTab\llm\
├─ qwen2.5-7b-instruct-q4_k_m.gguf
├─ mistral-7b-instruct-q4_k_m.gguf
└─ llama-3.1-8b-instruct-q4_k_m.gguf
```

Nutzung:

- Erklärungen
- Übehinweise
- Musiktheorie
- Songwriting-Vorschläge
